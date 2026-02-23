//! Signal messaging integration
//!
//! Signal provides end-to-end encrypted messaging with excellent privacy guarantees.
//! This integration uses signal-cli for programmatic access.
//!
//! # Setup
//!
//! 1. Install signal-cli: https://github.com/AsamK/signal-cli
//! 2. Register/link your device: `signal-cli link`
//! 3. Configure in my-agent config
//!
//! # Security
//!
//! - All messages are end-to-end encrypted
//! - No message content is stored on servers
//! - Perfect forward secrecy
//! - Sealed sender support

use anyhow::{Result, Context, bail};
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::path::PathBuf;
use tracing::{info, warn, error};

use crate::messaging::{Message, MessagingPlatform, Priority};
use crate::config::Config;

/// Signal client configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalConfig {
    /// Path to signal-cli binary
    pub cli_path: PathBuf,
    /// Signal phone number (with country code, e.g., +1234567890)
    pub phone_number: String,
    /// Signal-cli data directory (contains keys and messages)
    pub data_dir: Option<PathBuf>,
    /// Whether to use native libsignal-client (faster)
    pub use_native: bool,
}

impl SignalConfig {
    /// Create default config
    pub fn new(phone_number: impl Into<String>) -> Self {
        Self {
            cli_path: PathBuf::from("signal-cli"),
            phone_number: phone_number.into(),
            data_dir: None,
            use_native: true,
        }
    }

    /// Load from main config
    pub fn from_config(_config: &Config) -> Result<Self> {
        // Try to load from config file or environment
        let phone_number = std::env::var("SIGNAL_PHONE_NUMBER")
            .ok()
            .or_else(|| {
                // Try to read from config
                let config_path = dirs::config_dir()?.join("my-agent/config.toml");
                let contents = std::fs::read_to_string(config_path).ok()?;
                let value: toml::Value = toml::from_str(&contents).ok()?;
                value.get("signal")?.get("phone_number")?.as_str().map(String::from)
            })
            .context("Signal phone number not configured. Set SIGNAL_PHONE_NUMBER or add to config.")?;

        let cli_path = std::env::var("SIGNAL_CLI_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("signal-cli"));

        Ok(Self {
            cli_path,
            phone_number,
            data_dir: std::env::var("SIGNAL_DATA_DIR").ok().map(PathBuf::from),
            use_native: true,
        })
    }

    /// Check if Signal is properly configured
    pub fn is_configured(&self) -> bool {
        if self.phone_number.is_empty() {
            return false;
        }

        // Check if signal-cli is available
        match Command::new(&self.cli_path).arg("--version").output() {
            Ok(output) => output.status.success(),
            Err(_) => {
                warn!("signal-cli not found at {:?}", self.cli_path);
                false
            }
        }
    }
}

/// Signal client for sending messages
#[derive(Debug, Clone)]
pub struct SignalClient {
    config: SignalConfig,
}

impl SignalClient {
    /// Create a new Signal client
    pub fn new(config: SignalConfig) -> Self {
        Self { config }
    }

    /// Check if signal-cli is available and configured
    pub fn check_setup(&self) -> Result<()> {
        let output = Command::new(&self.config.cli_path)
            .arg("--version")
            .output()
            .context("Failed to run signal-cli. Is it installed?")?;

        if !output.status.success() {
            bail!("signal-cli is not working properly");
        }

        info!("signal-cli version: {}", String::from_utf8_lossy(&output.stdout));

        // Check if account is registered
        let output = Command::new(&self.config.cli_path)
            .args(["--config", self.config.data_dir.as_ref()
                .map(|p| p.to_str().unwrap_or("/var/lib/signal-cli"))
                .unwrap_or("/var/lib/signal-cli")])
            .arg("listAccounts")
            .output()
            .context("Failed to check Signal accounts")?;

        let accounts = String::from_utf8_lossy(&output.stdout);
        if !accounts.contains(&self.config.phone_number) {
            bail!(
                "Signal account {} not found. Run: signal-cli link",
                self.config.phone_number
            );
        }

        Ok(())
    }

    /// Send a text message to a recipient
    pub async fn send_text(&self, recipient: &str, message: &str) -> Result<()> {
        self.send_message_internal(recipient, message, None).await
    }

    /// Send a message with attachment
    pub async fn send_with_attachment(
        &self,
        recipient: &str,
        message: &str,
        attachment_path: &std::path::Path,
    ) -> Result<()> {
        self.send_message_internal(recipient, message, Some(attachment_path)).await
    }

    /// Internal method to send messages
    async fn send_message_internal(
        &self,
        recipient: &str,
        message: &str,
        attachment: Option<&std::path::Path>,
    ) -> Result<()> {
        let recipient = self.normalize_recipient(recipient);

        let mut cmd = Command::new(&self.config.cli_path);

        // Add config directory if specified
        if let Some(ref data_dir) = self.config.data_dir {
            cmd.arg("--config").arg(data_dir);
        }

        // Build command
        cmd.arg("-a").arg(&self.config.phone_number);
        cmd.arg("send");
        cmd.arg("-m").arg(message);

        // Add attachment if provided
        if let Some(attachment_path) = attachment {
            if attachment_path.exists() {
                cmd.arg("-a").arg(attachment_path);
            } else {
                bail!("Attachment not found: {}", attachment_path.display());
            }
        }

        // Add recipient
        cmd.arg(&recipient);

        info!("Sending Signal message to {}", recipient);

        let output = cmd.output()
            .context("Failed to execute signal-cli send command")?;

        if output.status.success() {
            info!("Signal message sent successfully to {}", recipient);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Signal send failed: {}", stderr);
            bail!("Failed to send Signal message: {}", stderr)
        }
    }

    /// Send a message to a group
    pub async fn send_to_group(&self, group_id: &str, message: &str) -> Result<()> {
        let mut cmd = Command::new(&self.config.cli_path);

        if let Some(ref data_dir) = self.config.data_dir {
            cmd.arg("--config").arg(data_dir);
        }

        cmd.arg("-a").arg(&self.config.phone_number);
        cmd.arg("send");
        cmd.arg("-m").arg(message);
        cmd.arg("-g").arg(group_id);

        info!("Sending Signal message to group {}", group_id);

        let output = cmd.output()
            .context("Failed to execute signal-cli group send command")?;

        if output.status.success() {
            info!("Signal group message sent successfully");
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to send Signal group message: {}", stderr)
        }
    }

    /// List linked devices
    pub fn list_devices(&self) -> Result<Vec<String>> {
        let output = Command::new(&self.config.cli_path)
            .args([
                "-a", &self.config.phone_number,
                "listDevices"
            ])
            .output()
            .context("Failed to list Signal devices")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(stdout.lines().map(|s| s.to_string()).collect())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to list devices: {}", stderr)
        }
    }

    /// Normalize recipient phone number
    fn normalize_recipient(&self, recipient: &str) -> String {
        // Remove spaces and dashes
        let normalized: String = recipient
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '+')
            .collect();

        // Ensure it starts with +
        if normalized.starts_with('+') {
            normalized
        } else {
            format!("+{}", normalized)
        }
    }

    /// Receive messages (daemon mode)
    /// This should be run in a separate task to receive incoming messages
    pub async fn receive_messages(&self) -> Result<()> {
        let output = Command::new(&self.config.cli_path)
            .args([
                "-a", &self.config.phone_number,
                "receive"
            ])
            .output()
            .context("Failed to receive Signal messages")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if !stdout.is_empty() {
                info!("Received messages: {}", stdout);
            }
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to receive messages: {}", stderr)
        }
    }

    /// Start daemon mode for continuous message receiving
    pub fn start_daemon(&self) -> Result<std::process::Child> {
        info!("Starting Signal daemon for receiving messages");

        let mut cmd = Command::new(&self.config.cli_path);

        if let Some(ref data_dir) = self.config.data_dir {
            cmd.arg("--config").arg(data_dir);
        }

        cmd.args(["-a", &self.config.phone_number, "daemon", "--system"])
            .spawn()
            .context("Failed to start signal-cli daemon")
    }
}

#[async_trait::async_trait]
impl MessagingPlatform for SignalClient {
    async fn send(&self, to: &str, message: &Message) -> Result<()> {
        // Handle attachments if present
        if let Some(ref attachments) = message.attachments {
            for attachment in attachments {
                // Decode base64 and save to temp file
                let temp_dir = std::env::temp_dir();
                let temp_path = temp_dir.join(&attachment.filename);

                let data = base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    &attachment.data
                )?;

                tokio::fs::write(&temp_path, data).await?;

                // Send with attachment
                self.send_with_attachment(to, &message.content, &temp_path).await?;

                // Clean up temp file
                let _ = tokio::fs::remove_file(&temp_path).await;
            }
            Ok(())
        } else {
            self.send_text(to, &message.content).await
        }
    }

    fn is_configured(&self) -> bool {
        self.config.is_configured()
    }

    fn name(&self) -> &'static str {
        "Signal"
    }
}

/// Send a notification message
pub async fn notify(message: &str, recipient: &str) -> Result<()> {
    let config = SignalConfig::from_config(&Config::default())?;
    let client = SignalClient::new(config);

    let msg = Message {
        content: message.to_string(),
        attachments: None,
        metadata: None,
    };

    client.send(recipient, &msg).await
}

/// Send a high-priority alert
pub async fn alert(title: &str, message: &str, recipient: &str) -> Result<()> {
    let formatted = format!("🚨 {}\n\n{}", title, message);
    notify(&formatted, recipient).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_recipient() {
        let config = SignalConfig::new("+1234567890");
        let client = SignalClient::new(config);

        assert_eq!(client.normalize_recipient("+1 234 567 8900"), "+12345678900");
        assert_eq!(client.normalize_recipient("123-456-7890"), "+1234567890");
        assert_eq!(client.normalize_recipient("+1234567890"), "+1234567890");
    }

    #[test]
    fn test_config_serialization() {
        let config = SignalConfig::new("+1234567890");
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SignalConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.phone_number, deserialized.phone_number);
    }
}
