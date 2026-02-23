//! Cloud secrets management
//!
//! Manages secrets for local and cloud deployment with encryption support.

use anyhow::{Result, Context, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::env;

/// Secret source (where the secret comes from)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecretSource {
    /// Environment variable
    Environment(String),
    /// File path
    File(String),
    /// OS keyring
    Keyring(String),
    /// Encrypted storage
    Encrypted,
    /// Inline value (for development only)
    Inline(String),
}

/// A managed secret
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Secret {
    /// Secret name/key
    pub name: String,
    /// Source of the secret
    pub source: SecretSource,
    /// Optional description
    pub description: Option<String>,
    /// Whether this secret is required
    pub required: bool,
}

/// Secrets manager configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsConfig {
    /// Secrets file path
    pub secrets_file: PathBuf,
    /// Whether to use encryption
    pub use_encryption: bool,
    /// Encryption key source
    pub encryption_key_source: SecretSource,
    /// Default secret source for new secrets
    pub default_source: SecretSource,
}

impl Default for SecretsConfig {
    fn default() -> Self {
        let config_dir = directories::ProjectDirs::from("com", "my-agent", "my-agent")
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        Self {
            secrets_file: config_dir.join("secrets.enc"),
            use_encryption: true,
            encryption_key_source: SecretSource::Environment("MY_AGENT_SECRET_KEY".to_string()),
            default_source: SecretSource::Keyring("default".to_string()),
        }
    }
}

/// Secrets manager for cloud and local deployment
pub struct SecretsManager {
    config: SecretsConfig,
    /// Cached secrets (name -> value)
    cache: HashMap<String, String>,
    /// Secret definitions
    secrets: HashMap<String, Secret>,
}

impl SecretsManager {
    /// Create a new secrets manager
    pub fn new(config: SecretsConfig) -> Self {
        Self {
            config,
            cache: HashMap::new(),
            secrets: HashMap::new(),
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(SecretsConfig::default())
    }

    /// Register a secret definition
    pub fn register(&mut self, secret: Secret) {
        self.secrets.insert(secret.name.clone(), secret);
    }

    /// Register multiple secrets
    pub fn register_many(&mut self, secrets: Vec<Secret>) {
        for secret in secrets {
            self.register(secret);
        }
    }

    /// Get a secret value
    pub fn get(&mut self, name: &str) -> Result<String> {
        // Check cache first
        if let Some(value) = self.cache.get(name) {
            return Ok(value.clone());
        }

        // Get secret definition
        let secret = self.secrets.get(name).cloned().unwrap_or_else(|| Secret {
            name: name.to_string(),
            source: self.config.default_source.clone(),
            description: None,
            required: false,
        });

        // Retrieve value based on source
        let value = self.retrieve_secret(&secret)?;

        // Cache the value
        self.cache.insert(name.to_string(), value.clone());

        Ok(value)
    }

    /// Get a secret, returning None if not found
    pub fn get_optional(&mut self, name: &str) -> Result<Option<String>> {
        match self.get(name) {
            Ok(value) => Ok(Some(value)),
            Err(_) => Ok(None),
        }
    }

    /// Set a secret value
    pub fn set(&mut self, name: &str, value: &str) -> Result<()> {
        // Store in cache
        self.cache.insert(name.to_string(), value.to_string());

        // Store based on source if secret is registered
        if let Some(secret) = self.secrets.get(name) {
            match &secret.source {
                SecretSource::Keyring(key) => {
                    let entry = keyring::Entry::new("my-agent", key)
                        .context("Failed to create keyring entry")?;
                    entry.set_password(value)
                        .context("Failed to store secret in keyring")?;
                }
                SecretSource::Environment(var) => {
                    // Can't set env vars persistently, just cache
                    tracing::warn!("Cannot persistently set environment variable {}", var);
                }
                _ => {
                    // For other sources, we'll need to save to file
                    self.save_to_file()?;
                }
            }
        }

        tracing::info!("Secret '{}' stored successfully", name);
        Ok(())
    }

    /// Delete a secret
    pub fn delete(&mut self, name: &str) -> Result<()> {
        // Remove from cache
        self.cache.remove(name);

        // Remove from keyring if applicable
        if let Some(secret) = self.secrets.get(name) {
            if let SecretSource::Keyring(key) = &secret.source {
                let entry = keyring::Entry::new("my-agent", key)
                    .context("Failed to create keyring entry")?;
                let _ = entry.delete_credential();
            }
        }

        tracing::info!("Secret '{}' deleted", name);
        Ok(())
    }

    /// Check if a secret exists
    pub fn exists(&mut self, name: &str) -> bool {
        if self.cache.contains_key(name) {
            return true;
        }

        if let Some(secret) = self.secrets.get(name) {
            self.check_source_exists(&secret.source)
        } else {
            false
        }
    }

    /// List all registered secrets
    pub fn list(&self) -> Vec<&Secret> {
        self.secrets.values().collect()
    }

    /// Validate all required secrets are available
    pub fn validate(&mut self) -> Result<Vec<String>> {
        let mut missing = Vec::new();

        // Collect required secret names first to avoid borrow issues
        let required_names: Vec<String> = self.secrets
            .iter()
            .filter(|(_, secret)| secret.required)
            .map(|(name, _)| name.clone())
            .collect();

        for name in required_names {
            if !self.exists(&name) {
                missing.push(name);
            }
        }

        Ok(missing)
    }

    /// Retrieve a secret from its source
    fn retrieve_secret(&self, secret: &Secret) -> Result<String> {
        match &secret.source {
            SecretSource::Environment(var) => {
                env::var(var)
                    .with_context(|| format!("Environment variable {} not set", var))
            }
            SecretSource::File(path) => {
                std::fs::read_to_string(path)
                    .with_context(|| format!("Failed to read secret file: {}", path))
                    .map(|s| s.trim().to_string())
            }
            SecretSource::Keyring(key) => {
                let entry = keyring::Entry::new("my-agent", key)
                    .context("Failed to create keyring entry")?;
                entry.get_password()
                    .with_context(|| format!("Failed to get secret '{}' from keyring", secret.name))
            }
            SecretSource::Encrypted => {
                self.retrieve_encrypted(&secret.name)
            }
            SecretSource::Inline(value) => {
                Ok(value.clone())
            }
        }
    }

    /// Check if a source exists
    fn check_source_exists(&self, source: &SecretSource) -> bool {
        match source {
            SecretSource::Environment(var) => env::var(var).is_ok(),
            SecretSource::File(path) => Path::new(path).exists(),
            SecretSource::Keyring(key) => {
                keyring::Entry::new("my-agent", key)
                    .and_then(|e| e.get_password())
                    .is_ok()
            }
            SecretSource::Encrypted => {
                // Check if encrypted store exists and has the secret
                self.config.secrets_file.exists()
            }
            SecretSource::Inline(_) => true,
        }
    }

    /// Retrieve an encrypted secret
    fn retrieve_encrypted(&self, name: &str) -> Result<String> {
        if !self.config.secrets_file.exists() {
            bail!("Encrypted secrets file not found");
        }

        // Get encryption key
        let key = self.get_encryption_key()?;

        // Read encrypted file
        let encrypted = std::fs::read(&self.config.secrets_file)
            .context("Failed to read encrypted secrets file")?;

        // Decrypt and parse
        let decrypted = decrypt(&encrypted, &key)
            .context("Failed to decrypt secrets")?;

        let store: HashMap<String, String> = serde_json::from_str(&decrypted)
            .context("Failed to parse secrets")?;

        store.get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Secret '{}' not found in encrypted store", name))
    }

    /// Save secrets to encrypted file
    fn save_to_file(&self) -> Result<()> {
        let key = self.get_encryption_key()?;

        // Serialize current cache
        let json = serde_json::to_string(&self.cache)
            .context("Failed to serialize secrets")?;

        // Encrypt
        let encrypted = encrypt(json.as_bytes(), &key)
            .context("Failed to encrypt secrets")?;

        // Ensure parent directory exists
        if let Some(parent) = self.config.secrets_file.parent() {
            std::fs::create_dir_all(parent)
                .context("Failed to create secrets directory")?;
        }

        // Write to file with restrictive permissions
        std::fs::write(&self.config.secrets_file, encrypted)
            .context("Failed to write secrets file")?;

        // Set file permissions (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.config.secrets_file, std::fs::Permissions::from_mode(0o600))
                .ok();
        }

        Ok(())
    }

    /// Get the encryption key
    fn get_encryption_key(&self) -> Result<String> {
        match &self.config.encryption_key_source {
            SecretSource::Environment(var) => {
                env::var(var)
                    .context(format!("Encryption key env var {} not set", var))
            }
            SecretSource::Keyring(key) => {
                let entry = keyring::Entry::new("my-agent", key)
                    .context("Failed to create keyring entry for encryption key")?;
                entry.get_password()
                    .context("Failed to get encryption key from keyring")
            }
            _ => bail!("Unsupported encryption key source"),
        }
    }

    /// Generate a new encryption key
    pub fn generate_encryption_key() -> String {
        use rand::Rng;
        let mut rng = rand::rng();
        let key: [u8; 32] = rng.random();
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &key)
    }
}

impl Default for SecretsManager {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Simple XOR encryption (for basic protection, not cryptographically secure)
/// For production, consider using age or other proper encryption libraries
fn encrypt(data: &[u8], key: &str) -> Result<Vec<u8>> {
    let key_bytes = key.as_bytes();
    let mut result = Vec::with_capacity(data.len());

    // Add a simple header for identification
    result.extend_from_slice(b"MAENC1");

    // XOR with key
    for (i, byte) in data.iter().enumerate() {
        result.push(byte ^ key_bytes[i % key_bytes.len()]);
    }

    Ok(result)
}

/// Decrypt data encrypted with encrypt()
fn decrypt(data: &[u8], key: &str) -> Result<String> {
    // Check header
    if data.len() < 6 || &data[..6] != b"MAENC1" {
        bail!("Invalid encrypted data format");
    }

    let encrypted = &data[6..];
    let key_bytes = key.as_bytes();
    let mut result = Vec::with_capacity(encrypted.len());

    for (i, byte) in encrypted.iter().enumerate() {
        result.push(byte ^ key_bytes[i % key_bytes.len()]);
    }

    String::from_utf8(result)
        .context("Decrypted data is not valid UTF-8")
}

/// Standard secrets used by my-agent
pub fn standard_secrets() -> Vec<Secret> {
    vec![
        Secret {
            name: "openrouter_api_key".to_string(),
            source: SecretSource::Keyring("openrouter-api-key".to_string()),
            description: Some("OpenRouter API key for cloud LLM access".to_string()),
            required: true,
        },
        Secret {
            name: "encryption_key".to_string(),
            source: SecretSource::Environment("MY_AGENT_SECRET_KEY".to_string()),
            description: Some("Master encryption key for secrets storage".to_string()),
            required: false,
        },
        Secret {
            name: "fly_api_token".to_string(),
            source: SecretSource::Environment("FLY_API_TOKEN".to_string()),
            description: Some("Fly.io API token for deployment".to_string()),
            required: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt() {
        let key = "test-key-12345";
        let data = "secret data here";

        let encrypted = encrypt(data.as_bytes(), key).unwrap();
        let decrypted = decrypt(&encrypted, key).unwrap();

        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_encrypt_decrypt_with_unicode() {
        let key = "unicode-key-🔐";
        let data = "Hello 世界! 🎉";

        let encrypted = encrypt(data.as_bytes(), key).unwrap();
        let decrypted = decrypt(&encrypted, key).unwrap();

        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_secrets_manager_registration() {
        let mut manager = SecretsManager::with_defaults();

        manager.register(Secret {
            name: "test".to_string(),
            source: SecretSource::Inline("value".to_string()),
            description: None,
            required: false,
        });

        let value = manager.get("test").unwrap();
        assert_eq!(value, "value");
    }

    #[test]
    fn test_generate_encryption_key() {
        let key1 = SecretsManager::generate_encryption_key();
        let key2 = SecretsManager::generate_encryption_key();

        // Keys should be different
        assert_ne!(key1, key2);
        // Keys should be valid base64
        assert!(base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &key1).is_ok());
    }
}
