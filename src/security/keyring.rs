//! Keyring integration for secure API key storage
//! Falls back to file storage if keyring is unavailable

use anyhow::{Result, Context};
use std::path::PathBuf;
use std::fs;

const SERVICE_NAME: &str = "my-agent";
const API_KEY_USERNAME: &str = "openrouter-api-key";
const HF_API_KEY_USERNAME: &str = "huggingface-api-key";
const API_KEY_FILE: &str = "api_key.txt";
const HF_API_KEY_FILE: &str = "hf_api_key.txt";

/// Get the path for the fallback API key file
fn api_key_file_path() -> Result<PathBuf> {
    let base = directories::ProjectDirs::from("com", "my-agent", "my-agent")
        .context("Failed to get project directories")?;
    let dir = base.config_dir();
    fs::create_dir_all(dir).context("Failed to create config directory")?;
    Ok(dir.join(API_KEY_FILE))
}

/// Set API key - tries keyring first, falls back to file
pub fn set_api_key(key: &str) -> Result<()> {
    // Try keyring first
    match keyring::Entry::new(SERVICE_NAME, API_KEY_USERNAME) {
        Ok(entry) => {
            if entry.set_password(key).is_ok() {
                // Also save to file as backup in case keyring retrieval fails
                let _ = save_to_file(key);
                return Ok(());
            }
        }
        Err(_) => {}
    }

    // Fallback to file storage
    save_to_file(key)?;
    println!("Note: Using file-based storage (keyring unavailable)");
    Ok(())
}

fn save_to_file(key: &str) -> Result<()> {
    let path = api_key_file_path()?;
    fs::write(&path, key).context("Failed to write API key file")?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .context("Failed to set file permissions")?;
    }

    Ok(())
}

/// Get API key - tries keyring first, falls back to file
pub fn get_api_key() -> Result<String> {
    // Try keyring first
    if let Ok(entry) = keyring::Entry::new(SERVICE_NAME, API_KEY_USERNAME) {
        if let Ok(key) = entry.get_password() {
            return Ok(key);
        }
    }

    // Fallback to file
    let path = api_key_file_path()?;
    let key = fs::read_to_string(&path)
        .context("Failed to read API key. Run 'my-agent config --set-api-key YOUR_KEY' first.")?;
    Ok(key.trim().to_string())
}

/// Delete API key from both keyring and file
pub fn delete_api_key() -> Result<()> {
    // Try to delete from keyring
    if let Ok(entry) = keyring::Entry::new(SERVICE_NAME, API_KEY_USERNAME) {
        let _ = entry.delete_credential();
    }

    // Delete file
    let path = api_key_file_path()?;
    if path.exists() {
        fs::remove_file(&path).context("Failed to delete API key file")?;
    }

    Ok(())
}

/// Check if API key is set (in either keyring or file)
pub fn has_api_key() -> bool {
    // Check keyring
    if let Ok(entry) = keyring::Entry::new(SERVICE_NAME, API_KEY_USERNAME) {
        if entry.get_password().is_ok() {
            return true;
        }
    }

    // Check file
    if let Ok(path) = api_key_file_path() {
        if path.exists() {
            return true;
        }
    }

    false
}

// ============ Hugging Face API Key Functions ============

/// Get the path for the HF API key file
fn hf_api_key_file_path() -> Result<PathBuf> {
    let base = directories::ProjectDirs::from("com", "my-agent", "my-agent")
        .context("Failed to get project directories")?;
    let dir = base.config_dir();
    fs::create_dir_all(dir).context("Failed to create config directory")?;
    Ok(dir.join(HF_API_KEY_FILE))
}

/// Set Hugging Face API key
pub fn set_hf_api_key(key: &str) -> Result<()> {
    // Try keyring first
    match keyring::Entry::new(SERVICE_NAME, HF_API_KEY_USERNAME) {
        Ok(entry) => {
            if entry.set_password(key).is_ok() {
                return Ok(());
            }
        }
        Err(_) => {}
    }

    // Fallback to file storage
    let path = hf_api_key_file_path()?;
    fs::write(&path, key).context("Failed to write HF API key file")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .context("Failed to set file permissions")?;
    }

    Ok(())
}

/// Get Hugging Face API key
pub fn get_hf_api_key() -> Result<String> {
    // Try keyring first
    if let Ok(entry) = keyring::Entry::new(SERVICE_NAME, HF_API_KEY_USERNAME) {
        if let Ok(key) = entry.get_password() {
            return Ok(key);
        }
    }

    // Fallback to file
    let path = hf_api_key_file_path()?;
    let key = fs::read_to_string(&path)
        .context("Failed to read HF API key. Run 'my-agent config --set-hf-api-key YOUR_KEY' first.")?;
    Ok(key.trim().to_string())
}

/// Delete Hugging Face API key
pub fn delete_hf_api_key() -> Result<()> {
    if let Ok(entry) = keyring::Entry::new(SERVICE_NAME, HF_API_KEY_USERNAME) {
        let _ = entry.delete_credential();
    }

    let path = hf_api_key_file_path()?;
    if path.exists() {
        fs::remove_file(&path).context("Failed to delete HF API key file")?;
    }

    Ok(())
}

/// Check if HF API key is set
pub fn has_hf_api_key() -> bool {
    if let Ok(entry) = keyring::Entry::new(SERVICE_NAME, HF_API_KEY_USERNAME) {
        if entry.get_password().is_ok() {
            return true;
        }
    }

    if let Ok(path) = hf_api_key_file_path() {
        if path.exists() {
            return true;
        }
    }

    false
}

// ============ Server Password Functions ============

const SERVER_PASSWORD_USERNAME: &str = "server-password-hash";
const SERVER_PASSWORD_FILE: &str = "server_password_hash.txt";

/// Get the path for the server password hash file
fn server_password_file_path() -> Result<PathBuf> {
    let base = directories::ProjectDirs::from("com", "my-agent", "my-agent")
        .context("Failed to get project directories")?;
    let dir = base.config_dir();
    fs::create_dir_all(dir).context("Failed to create config directory")?;
    Ok(dir.join(SERVER_PASSWORD_FILE))
}

/// Store a hashed server password
pub fn set_server_password(password: &str) -> Result<()> {
    let hash = crate::server::auth::hash_password(password)?;

    // Try keyring first
    match keyring::Entry::new(SERVICE_NAME, SERVER_PASSWORD_USERNAME) {
        Ok(entry) => {
            if entry.set_password(&hash).is_ok() {
                // Also save to file as backup
                let _ = save_server_password_to_file(&hash);
                return Ok(());
            }
        }
        Err(_) => {}
    }

    // Fallback to file storage
    save_server_password_to_file(&hash)?;
    println!("Note: Using file-based storage for password hash (keyring unavailable)");
    Ok(())
}

fn save_server_password_to_file(hash: &str) -> Result<()> {
    let path = server_password_file_path()?;
    fs::write(&path, hash).context("Failed to write server password file")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .context("Failed to set file permissions")?;
    }

    Ok(())
}

/// Get the stored server password hash
pub fn get_server_password_hash() -> Result<String> {
    // Try keyring first
    if let Ok(entry) = keyring::Entry::new(SERVICE_NAME, SERVER_PASSWORD_USERNAME) {
        if let Ok(hash) = entry.get_password() {
            return Ok(hash);
        }
    }

    // Fallback to file
    let path = server_password_file_path()?;
    let hash = fs::read_to_string(&path)
        .context("Server password not configured. Run 'my-agent config --set-password' first.")?;
    Ok(hash.trim().to_string())
}

/// Check if a server password has been configured
pub fn has_server_password() -> bool {
    if let Ok(entry) = keyring::Entry::new(SERVICE_NAME, SERVER_PASSWORD_USERNAME) {
        if entry.get_password().is_ok() {
            return true;
        }
    }

    if let Ok(path) = server_password_file_path() {
        if path.exists() {
            return true;
        }
    }

    false
}
