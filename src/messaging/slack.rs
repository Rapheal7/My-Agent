//! Slack messaging integration
//!
//! Slack integration for team notifications and agent interactions.
//! Supports both incoming webhooks and Slack API (Socket Mode for real-time).
//!
//! # Setup
//!
//! ## Option 1: Incoming Webhooks (Simple, read-only)
//! 1. Create a Slack app at https://api.slack.com/apps
//! 2. Enable "Incoming Webhooks"
//! 3. Copy the webhook URL
//!
//! ## Option 2: Slack API with Socket Mode (Full functionality)
//! 1. Create a Slack app
//! 2. Enable "Bots" and add scopes: `chat:write`, `im:write`, `users:read`
//! 3. Enable "Socket Mode"
//! 4. Generate app-level token with `connections:write` scope
//! 5. Install app to workspace
//!
//! # Security
//!
//! - Store tokens securely (use keyring)
//! - Use Socket Mode instead of exposing public URLs
//! - Validate Slack signatures on incoming webhooks

use anyhow::{Result, Context, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use tokio_tungstenite::tungstenite::Utf8Bytes;
use tracing::{info, warn, error};

use crate::messaging::{Message, MessagingPlatform};
use crate::config::Config;

/// Slack client configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackConfig {
    /// Bot User OAuth Token (xoxb-...)
    pub bot_token: Option<String>,
    /// App-Level Token for Socket Mode (xapp-...)
    pub app_token: Option<String>,
    /// Incoming Webhook URL (for simple notifications)
    pub webhook_url: Option<String>,
    /// Default channel to post to
    pub default_channel: Option<String>,
    /// Signing secret for webhook verification
    pub signing_secret: Option<String>,
}

impl SlackConfig {
    /// Create a new config with bot token
    pub fn with_bot_token(token: impl Into<String>) -> Self {
        Self {
            bot_token: Some(token.into()),
            app_token: None,
            webhook_url: None,
            default_channel: None,
            signing_secret: None,
        }
    }

    /// Create a new config with webhook URL
    pub fn with_webhook(url: impl Into<String>) -> Self {
        Self {
            bot_token: None,
            app_token: None,
            webhook_url: Some(url.into()),
            default_channel: None,
            signing_secret: None,
        }
    }

    /// Load from main config
    pub fn from_config(_config: &Config) -> Result<Self> {
        // Try environment variables first
        let bot_token = std::env::var("SLACK_BOT_TOKEN").ok();
        let app_token = std::env::var("SLACK_APP_TOKEN").ok();
        let webhook_url = std::env::var("SLACK_WEBHOOK_URL").ok();
        let default_channel = std::env::var("SLACK_DEFAULT_CHANNEL").ok();
        let signing_secret = std::env::var("SLACK_SIGNING_SECRET").ok();

        // Try config file
        let config_path = dirs::config_dir()
            .map(|d| d.join("my-agent/config.toml"));

        if let Some(ref path) = config_path {
            if let Ok(contents) = std::fs::read_to_string(path) {
                if let Ok(value) = toml::from_str::<toml::Value>(&contents) {
                    let slack_config = value.get("slack");

                    return Ok(Self {
                        bot_token: bot_token.or_else(|| {
                            slack_config?.get("bot_token")?.as_str().map(String::from)
                        }),
                        app_token: app_token.or_else(|| {
                            slack_config?.get("app_token")?.as_str().map(String::from)
                        }),
                        webhook_url: webhook_url.or_else(|| {
                            slack_config?.get("webhook_url")?.as_str().map(String::from)
                        }),
                        default_channel: default_channel.or_else(|| {
                            slack_config?.get("default_channel")?.as_str().map(String::from)
                        }),
                        signing_secret: signing_secret.or_else(|| {
                            slack_config?.get("signing_secret")?.as_str().map(String::from)
                        }),
                    });
                }
            }
        }

        Ok(Self {
            bot_token,
            app_token,
            webhook_url,
            default_channel,
            signing_secret,
        })
    }

    /// Check if Slack is configured
    pub fn is_configured(&self) -> bool {
        self.bot_token.is_some() || self.webhook_url.is_some()
    }

    /// Check if Socket Mode is available
    pub fn socket_mode_available(&self) -> bool {
        self.app_token.is_some() && self.bot_token.is_some()
    }
}

/// Slack API client
#[derive(Debug, Clone)]
pub struct SlackClient {
    config: SlackConfig,
    http: Client,
}

/// Slack message block for rich formatting
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum Block {
    Section {
        text: TextObject,
        #[serde(skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<TextObject>>,
    },
    Divider,
    Image {
        image_url: String,
        alt_text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<TextObject>,
    },
    Context {
        elements: Vec<ContextElement>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ContextElement {
    Image { image_url: String, alt_text: String },
    Mrkdwn { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum TextObject {
    PlainText { text: String, #[serde(skip_serializing_if = "Option::is_none")] emoji: Option<bool> },
    Mrkdwn { text: String },
}

impl SlackClient {
    /// Create a new Slack client
    pub fn new(config: SlackConfig) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self { config, http }
    }

    /// Send a simple text message via webhook
    pub async fn send_webhook_message(&self, text: &str) -> Result<()> {
        let webhook_url = self.config.webhook_url.as_ref()
            .context("Slack webhook URL not configured")?;

        let payload = json!({
            "text": text,
        });

        let response = self.http
            .post(webhook_url)
            .json(&payload)
            .send()
            .await
            .context("Failed to send Slack webhook request")?;

        if response.status().is_success() {
            info!("Slack webhook message sent successfully");
            Ok(())
        } else {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("Slack webhook failed: {} - {}", status, body)
        }
    }

    /// Send a rich message via API
    pub async fn send_api_message(
        &self,
        channel: &str,
        text: &str,
        blocks: Option<Vec<Block>>,
    ) -> Result<()> {
        let token = self.config.bot_token.as_ref()
            .context("Slack bot token not configured")?;

        let mut payload = json!({
            "channel": channel,
            "text": text,
            "unfurl_links": false,
        });

        if let Some(blocks) = blocks {
            payload["blocks"] = serde_json::to_value(blocks)?;
        }

        let response = self.http
            .post("https://slack.com/api/chat.postMessage")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .context("Failed to send Slack API request")?;

        let result: serde_json::Value = response.json().await?;

        if result["ok"].as_bool() == Some(true) {
            info!("Slack API message sent successfully to {}", channel);
            Ok(())
        } else {
            let error = result["error"].as_str().unwrap_or("unknown_error");
            bail!("Slack API error: {}", error)
        }
    }

    /// Send a direct message to a user
    pub async fn send_direct_message(&self, user_id: &str, text: &str) -> Result<()> {
        let token = self.config.bot_token.as_ref()
            .context("Slack bot token not configured")?;

        // First, open a conversation with the user
        let open_response = self.http
            .post("https://slack.com/api/conversations.open")
            .header("Authorization", format!("Bearer {}", token))
            .form(&[("users", user_id)])
            .send()
            .await
            .context("Failed to open Slack conversation")?;

        let open_result: serde_json::Value = open_response.json().await?;

        if open_result["ok"].as_bool() != Some(true) {
            let error = open_result["error"].as_str().unwrap_or("unknown_error");
            bail!("Failed to open Slack conversation: {}", error);
        }

        let channel_id = open_result["channel"]["id"]
            .as_str()
            .context("No channel ID in response")?;

        // Send the message
        self.send_api_message(channel_id, text, None).await
    }

    /// Get user info by email
    pub async fn get_user_by_email(&self, email: &str) -> Result<SlackUser> {
        let token = self.config.bot_token.as_ref()
            .context("Slack bot token not configured")?;

        let response = self.http
            .get("https://slack.com/api/users.lookupByEmail")
            .header("Authorization", format!("Bearer {}", token))
            .query(&[("email", email)])
            .send()
            .await
            .context("Failed to lookup Slack user")?;

        let result: serde_json::Value = response.json().await?;

        if result["ok"].as_bool() == Some(true) {
            let user = serde_json::from_value(result["user"].clone())?;
            Ok(user)
        } else {
            let error = result["error"].as_str().unwrap_or("unknown_error");
            bail!("Slack user lookup error: {}", error)
        }
    }

    /// List channels the bot is in
    pub async fn list_channels(&self) -> Result<Vec<SlackChannel>> {
        let token = self.config.bot_token.as_ref()
            .context("Slack bot token not configured")?;

        let response = self.http
            .get("https://slack.com/api/conversations.list")
            .header("Authorization", format!("Bearer {}", token))
            .query(&[("types", "public_channel,private_channel")])
            .send()
            .await
            .context("Failed to list Slack channels")?;

        let result: serde_json::Value = response.json().await?;

        if result["ok"].as_bool() == Some(true) {
            let channels: Vec<SlackChannel> = serde_json::from_value(
                result["channels"].clone()
            )?;
            Ok(channels)
        } else {
            let error = result["error"].as_str().unwrap_or("unknown_error");
            bail!("Slack channels list error: {}", error)
        }
    }

    /// Upload a file to a channel
    pub async fn upload_file(
        &self,
        channel: &str,
        filename: &str,
        content: &[u8],
        title: Option<&str>,
    ) -> Result<()> {
        let token = self.config.bot_token.as_ref()
            .context("Slack bot token not configured")?;

        let response = self.http
            .post("https://slack.com/api/files.upload")
            .header("Authorization", format!("Bearer {}", token))
            .multipart(
                reqwest::multipart::Form::new()
                    .text("channels", channel.to_string())
                    .text("filename", filename.to_string())
                    .text("title", title.unwrap_or(filename).to_string())
                    .part("file", reqwest::multipart::Part::bytes(content.to_vec())
                        .file_name(filename.to_string())),
            )
            .send()
            .await
            .context("Failed to upload file to Slack")?;

        let result: serde_json::Value = response.json().await?;

        if result["ok"].as_bool() == Some(true) {
            info!("File uploaded successfully to {}", channel);
            Ok(())
        } else {
            let error = result["error"].as_str().unwrap_or("unknown_error");
            bail!("Slack file upload error: {}", error)
        }
    }

    /// Create a notification message with blocks
    pub fn create_notification_blocks(
        title: &str,
        message: &str,
        priority: &str,
    ) -> Vec<Block> {
        let emoji = match priority.to_lowercase().as_str() {
            "urgent" | "high" => "🚨",
            "warning" | "medium" => "⚠️",
            _ => "ℹ️",
        };

        vec![
            Block::Section {
                text: TextObject::Mrkdwn {
                    text: format!("{} *{}*", emoji, title),
                },
                fields: None,
            },
            Block::Divider,
            Block::Section {
                text: TextObject::PlainText {
                    text: message.to_string(),
                    emoji: Some(true),
                },
                fields: None,
            },
            Block::Context {
                elements: vec![
                    ContextElement::Mrkdwn {
                        text: format!("Sent by My Agent at {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")),
                    },
                ],
            },
        ]
    }

    /// Verify Slack webhook signature
    pub fn verify_signature(&self, body: &str, timestamp: &str, signature: &str) -> Result<bool> {
        let secret = self.config.signing_secret.as_ref()
            .context("Slack signing secret not configured")?;

        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        type HmacSha256 = Hmac<Sha256>;

        let basestring = format!("v0:{}:{}", timestamp, body);

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())?;
        mac.update(basestring.as_bytes());
        let result = mac.finalize();
        let code_bytes = result.into_bytes();

        let expected_signature = format!("v0={}", hex::encode(code_bytes));

        Ok(signature == expected_signature)
    }
}

#[async_trait::async_trait]
impl MessagingPlatform for SlackClient {
    async fn send(&self, to: &str, message: &Message) -> Result<()> {
        let channel = if to.is_empty() {
            self.config.default_channel.as_deref()
                .context("No recipient specified and no default channel configured")?
        } else {
            to
        };

        // If webhook is configured and no bot token, use webhook
        if self.config.webhook_url.is_some() && self.config.bot_token.is_none() {
            return self.send_webhook_message(&message.content).await;
        }

        // Use API for rich messages
        let blocks = if message.metadata.as_ref().map(|m| m.contains_key("rich")).unwrap_or(false) {
            Some(Self::create_notification_blocks(
                "Agent Notification",
                &message.content,
                message.metadata.as_ref()
                    .and_then(|m| m.get("priority"))
                    .map(|s| s.as_str())
                    .unwrap_or("normal"),
            ))
        } else {
            None
        };

        self.send_api_message(channel, &message.content, blocks).await
    }

    fn is_configured(&self) -> bool {
        self.config.is_configured()
    }

    fn name(&self) -> &'static str {
        "Slack"
    }
}

/// Slack user representation
#[derive(Debug, Clone, Deserialize)]
pub struct SlackUser {
    pub id: String,
    pub name: String,
    pub real_name: Option<String>,
    pub profile: SlackUserProfile,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SlackUserProfile {
    pub email: Option<String>,
    pub display_name: Option<String>,
}

/// Slack channel representation
#[derive(Debug, Clone, Deserialize)]
pub struct SlackChannel {
    pub id: String,
    pub name: String,
    #[serde(rename = "is_private")]
    pub is_private: bool,
}

/// Socket Mode client for real-time messaging
pub mod socket_mode {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

    /// Start Socket Mode connection for real-time events
    pub async fn start_socket_mode(
        app_token: &str,
        bot_token: &str,
    ) -> Result<()> {
        // Step 1: Get WebSocket URL
        let client = Client::new();
        let response = client
            .get("https://slack.com/apps.connections.open")
            .header("Authorization", format!("Bearer {}", app_token))
            .send()
            .await?;

        let result: serde_json::Value = response.json().await?;
        let ws_url = result["url"]
            .as_str()
            .context("No WebSocket URL in response")?;

        // Step 2: Connect to WebSocket
        let (mut ws_stream, _) = connect_async(ws_url).await?;

        info!("Connected to Slack Socket Mode");

        // Handle messages
        while let Some(msg) = ws_stream.next().await {
            match msg {
                Ok(WsMessage::Text(text)) => {
                    let event: serde_json::Value = serde_json::from_str(&text)?;

                    // Handle hello message
                    if event["type"] == "hello" {
                        info!("Slack Socket Mode connection established");
                        continue;
                    }

                    // Handle events
                    if let Some(envelope_id) = event["envelope_id"].as_str() {
                        // Acknowledge the event
                        let ack = json!({"envelope_id": envelope_id});
                        ws_stream.send(WsMessage::Text(Utf8Bytes::from(ack.to_string()))).await?;

                        // Process the event
                        if let Some(payload) = event.get("payload") {
                            handle_event(payload, bot_token).await?;
                        }
                    }
                }
                Ok(WsMessage::Close(_)) => {
                    info!("Slack Socket Mode connection closed");
                    break;
                }
                Err(e) => {
                    error!("Socket Mode error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn handle_event(event: &serde_json::Value, _bot_token: &str) -> Result<()> {
        let event_type = event["event"]["type"].as_str();

        match event_type {
            Some("app_mention") => {
                info!("Bot mentioned in channel");
                // Handle mention
            }
            Some("message") => {
                // Handle direct message
                if event["event"]["channel_type"] == "im" {
                    info!("Direct message received");
                }
            }
            _ => {}
        }

        Ok(())
    }
}

/// Send a simple notification
pub async fn notify(message: &str, channel: Option<&str>) -> Result<()> {
    let config = SlackConfig::from_config(&Config::default())?;
    let client = SlackClient::new(config);

    let recipient = channel.or_else(|| client.config.default_channel.as_deref())
        .unwrap_or("#general");

    let msg = Message {
        content: message.to_string(),
        attachments: None,
        metadata: None,
    };

    client.send(recipient, &msg).await
}

/// Send an alert with rich formatting
pub async fn alert(title: &str, message: &str, channel: &str) -> Result<()> {
    let config = SlackConfig::from_config(&Config::default())?;
    let client = SlackClient::new(config);

    let blocks = SlackClient::create_notification_blocks(title, message, "high");

    client.send_api_message(channel, message, Some(blocks)).await
}
