//! Messaging integrations for agent notifications and interactions
//!
//! Supported platforms:
//! - Signal: End-to-end encrypted, privacy-focused (recommended)
//! - Slack: Team collaboration platform

pub mod signal;
pub mod slack;
pub mod telegram;

use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Generic message structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Message content
    pub content: String,
    /// Optional media attachments
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<Attachment>>,
    /// Message metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

/// Attachment (file, image, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// File name
    pub filename: String,
    /// MIME type
    pub content_type: String,
    /// File data (base64 encoded)
    pub data: String,
}

/// Message priority level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Normal,
    High,
    Urgent,
}

/// Common trait for messaging platforms
#[async_trait::async_trait]
pub trait MessagingPlatform: Send + Sync {
    /// Send a message
    async fn send(&self, to: &str, message: &Message) -> Result<()>;

    /// Check if the platform is configured and ready
    fn is_configured(&self) -> bool;

    /// Get platform name
    fn name(&self) -> &'static str;
}

/// Unified messaging manager
pub struct MessagingManager {
    platforms: HashMap<String, Box<dyn MessagingPlatform>>,
    default_platform: Option<String>,
}

impl MessagingManager {
    /// Create a new messaging manager
    pub fn new() -> Self {
        Self {
            platforms: HashMap::new(),
            default_platform: None,
        }
    }

    /// Register a messaging platform
    pub fn register(&mut self, name: &str, platform: Box<dyn MessagingPlatform>) {
        self.platforms.insert(name.to_string(), platform);
        if self.default_platform.is_none() {
            self.default_platform = Some(name.to_string());
        }
    }

    /// Set the default platform
    pub fn set_default(&mut self, name: &str) -> Result<()> {
        if self.platforms.contains_key(name) {
            self.default_platform = Some(name.to_string());
            Ok(())
        } else {
            anyhow::bail!("Platform '{}' not registered", name)
        }
    }

    /// Send a message using the default platform
    pub async fn send(&self, to: &str, message: &Message) -> Result<()> {
        let platform_name = self.default_platform.as_ref()
            .context("No default messaging platform configured")?;
        self.send_via(platform_name, to, message).await
    }

    /// Send a message via a specific platform
    pub async fn send_via(&self, platform: &str, to: &str, message: &Message) -> Result<()> {
        let platform = self.platforms.get(platform)
            .context(format!("Platform '{}' not found", platform))?;
        platform.send(to, message).await
    }

    /// Get list of available platforms
    pub fn available_platforms(&self) -> Vec<&str> {
        self.platforms.keys()
            .map(|s| s.as_str())
            .collect()
    }
}

impl Default for MessagingManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a messaging manager from configuration
pub fn create_from_config(config: &crate::config::Config) -> MessagingManager {
    let mut manager = MessagingManager::new();

    // Add Signal if configured
    if let Ok(signal_config) = signal::SignalConfig::from_config(config) {
        if signal_config.is_configured() {
            manager.register("signal", Box::new(signal::SignalClient::new(signal_config)));
        }
    }

    // Add Slack if configured
    if let Ok(slack_config) = slack::SlackConfig::from_config(config) {
        if slack_config.is_configured() {
            manager.register("slack", Box::new(slack::SlackClient::new(slack_config)));
        }
    }

    // Add Telegram if configured
    if let Ok(telegram_config) = telegram::TelegramConfig::from_config(config) {
        if telegram_config.is_configured() {
            manager.register("telegram", Box::new(telegram::TelegramClient::new(telegram_config)));
        }
    }

    manager
}
