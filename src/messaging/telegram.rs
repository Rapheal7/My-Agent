//! Telegram messaging integration
//!
//! Telegram provides fast, reliable messaging with bot API support.
//! This integration uses the Telegram Bot API for programmatic access.
//!
//! # Setup
//!
//! 1. Create a bot via @BotFather on Telegram
//! 2. Get your bot token
//! 3. Configure in my-agent config
//!
//! # Security
//!
//! - HTTPS-only API communication
//! - Bot token should be kept secure
//! - Supports secret chats (client-side only)
//! - Message editing and deletion capabilities

use anyhow::{Result, Context, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn, error, debug};

use crate::messaging::{Message, MessagingPlatform, Attachment};
use crate::config::Config;

/// Telegram API base URL
const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// Telegram client configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Bot token from @BotFather (format: 123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11)
    pub bot_token: String,
    /// Default chat ID for notifications (can be user ID or channel ID)
    pub default_chat_id: Option<String>,
    /// Parse mode for messages (HTML, Markdown, MarkdownV2, or None)
    pub parse_mode: Option<String>,
    /// Disable link previews
    pub disable_web_page_preview: bool,
    /// API base URL (for self-hosted bot API servers)
    pub api_base: String,
}

impl TelegramConfig {
    /// Create a new config with bot token
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            default_chat_id: None,
            parse_mode: Some("HTML".to_string()),
            disable_web_page_preview: false,
            api_base: TELEGRAM_API_BASE.to_string(),
        }
    }

    /// Load from main config
    pub fn from_config(_config: &Config) -> Result<Self> {
        // Try to load from environment or config file
        let bot_token = std::env::var("TELEGRAM_BOT_TOKEN")
            .ok()
            .or_else(|| {
                let config_path = dirs::config_dir()?.join("my-agent/config.toml");
                let contents = std::fs::read_to_string(config_path).ok()?;
                let value: toml::Value = toml::from_str(&contents).ok()?;
                value.get("telegram")?.get("bot_token")?.as_str().map(String::from)
            })
            .context("Telegram bot token not configured. Set TELEGRAM_BOT_TOKEN or add to config.")?;

        let default_chat_id = std::env::var("TELEGRAM_CHAT_ID")
            .ok()
            .or_else(|| {
                let config_path = dirs::config_dir()?.join("my-agent/config.toml");
                let contents = std::fs::read_to_string(config_path).ok()?;
                let value: toml::Value = toml::from_str(&contents).ok()?;
                value.get("telegram")?.get("chat_id")?.as_str().map(String::from)
            });

        let parse_mode = std::env::var("TELEGRAM_PARSE_MODE")
            .ok()
            .or_else(|| Some("HTML".to_string()));

        Ok(Self {
            bot_token,
            default_chat_id,
            parse_mode,
            disable_web_page_preview: false,
            api_base: TELEGRAM_API_BASE.to_string(),
        })
    }

    /// Check if Telegram is properly configured
    pub fn is_configured(&self) -> bool {
        !self.bot_token.is_empty() && self.bot_token.contains(':')
    }

    /// Get API URL for a method
    fn api_url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.api_base, self.bot_token, method)
    }
}

/// Telegram client for sending and receiving messages
#[derive(Debug, Clone)]
pub struct TelegramClient {
    config: TelegramConfig,
    http_client: reqwest::Client,
}

/// Telegram message response
#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
    error_code: Option<i32>,
}

/// Telegram message info
#[derive(Debug, Deserialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub date: i64,
    pub text: Option<String>,
    pub chat: TelegramChat,
    pub from: Option<TelegramUser>,
}

/// Telegram chat info
#[derive(Debug, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
    pub title: Option<String>,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

/// Telegram user info
#[derive(Debug, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub username: Option<String>,
}

/// Telegram update (incoming message/event)
#[derive(Debug, Deserialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
    pub edited_message: Option<TelegramMessage>,
    pub channel_post: Option<TelegramMessage>,
    pub edited_channel_post: Option<TelegramMessage>,
}

/// Send message request
#[derive(Debug, Serialize)]
struct SendMessageRequest {
    chat_id: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable_web_page_preview: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_markup: Option<serde_json::Value>,
}

/// Send photo request
#[derive(Debug, Serialize)]
struct SendPhotoRequest {
    chat_id: String,
    caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<String>,
}

/// Send document request
#[derive(Debug, Serialize)]
struct SendDocumentRequest {
    chat_id: String,
    caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<String>,
}

impl TelegramClient {
    /// Create a new Telegram client
    pub fn new(config: TelegramConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self { config, http_client }
    }

    /// Test the bot token and get bot info
    pub async fn get_me(&self) -> Result<TelegramUser> {
        let url = self.config.api_url("getMe");

        let response: TelegramResponse<TelegramUser> = self.http_client
            .get(&url)
            .send()
            .await
            .context("Failed to connect to Telegram API")?
            .json()
            .await
            .context("Failed to parse Telegram response")?;

        if response.ok {
            response.result.context("No result in response")
        } else {
            bail!("Telegram API error: {}",
                response.description.unwrap_or_else(|| "Unknown error".to_string()))
        }
    }

    /// Send a text message
    pub async fn send_message(&self, chat_id: &str, text: &str) -> Result<TelegramMessage> {
        let url = self.config.api_url("sendMessage");

        let request = SendMessageRequest {
            chat_id: chat_id.to_string(),
            text: text.to_string(),
            parse_mode: self.config.parse_mode.clone(),
            disable_web_page_preview: if self.config.disable_web_page_preview { Some(true) } else { None },
            reply_markup: None,
        };

        debug!("Sending Telegram message to {}", chat_id);

        let response: TelegramResponse<TelegramMessage> = self.http_client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to send Telegram message")?
            .json()
            .await
            .context("Failed to parse Telegram response")?;

        if response.ok {
            info!("Telegram message sent successfully to {}", chat_id);
            response.result.context("No result in response")
        } else {
            let error_msg = response.description.unwrap_or_else(|| "Unknown error".to_string());
            error!("Telegram API error: {} (code: {:?})", error_msg, response.error_code);
            bail!("Telegram API error: {}", error_msg)
        }
    }

    /// Send a photo
    pub async fn send_photo(&self, chat_id: &str, photo_data: Vec<u8>, caption: Option<&str>) -> Result<TelegramMessage> {
        let url = self.config.api_url("sendPhoto");

        let form = reqwest::multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part("photo", reqwest::multipart::Part::bytes(photo_data)
                .file_name("image.png")
                .mime_str("image/png")?);

        let form = if let Some(cap) = caption {
            form.text("caption", cap.to_string())
                .text("parse_mode", self.config.parse_mode.clone().unwrap_or_default())
        } else {
            form
        };

        let response: TelegramResponse<TelegramMessage> = self.http_client
            .post(&url)
            .multipart(form)
            .send()
            .await
            .context("Failed to send Telegram photo")?
            .json()
            .await
            .context("Failed to parse Telegram response")?;

        if response.ok {
            info!("Telegram photo sent successfully to {}", chat_id);
            response.result.context("No result in response")
        } else {
            let error_msg = response.description.unwrap_or_else(|| "Unknown error".to_string());
            bail!("Telegram API error: {}", error_msg)
        }
    }

    /// Send a document/file
    pub async fn send_document(&self, chat_id: &str, document_data: Vec<u8>, filename: &str, caption: Option<&str>) -> Result<TelegramMessage> {
        let url = self.config.api_url("sendDocument");

        let content_type = guess_mime_type(filename);

        let form = reqwest::multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part("document", reqwest::multipart::Part::bytes(document_data)
                .file_name(filename.to_string())
                .mime_str(&content_type)?);

        let form = if let Some(cap) = caption {
            form.text("caption", cap.to_string())
                .text("parse_mode", self.config.parse_mode.clone().unwrap_or_default())
        } else {
            form
        };

        let response: TelegramResponse<TelegramMessage> = self.http_client
            .post(&url)
            .multipart(form)
            .send()
            .await
            .context("Failed to send Telegram document")?
            .json()
            .await
            .context("Failed to parse Telegram response")?;

        if response.ok {
            info!("Telegram document sent successfully to {}", chat_id);
            response.result.context("No result in response")
        } else {
            let error_msg = response.description.unwrap_or_else(|| "Unknown error".to_string());
            bail!("Telegram API error: {}", error_msg)
        }
    }

    /// Edit a previously sent message
    pub async fn edit_message(&self, chat_id: &str, message_id: i64, new_text: &str) -> Result<TelegramMessage> {
        let url = self.config.api_url("editMessageText");

        #[derive(Serialize)]
        struct EditRequest {
            chat_id: String,
            message_id: i64,
            text: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            parse_mode: Option<String>,
        }

        let request = EditRequest {
            chat_id: chat_id.to_string(),
            message_id,
            text: new_text.to_string(),
            parse_mode: self.config.parse_mode.clone(),
        };

        let response: TelegramResponse<TelegramMessage> = self.http_client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to edit Telegram message")?
            .json()
            .await
            .context("Failed to parse Telegram response")?;

        if response.ok {
            response.result.context("No result in response")
        } else {
            bail!("Telegram API error: {}",
                response.description.unwrap_or_else(|| "Unknown error".to_string()))
        }
    }

    /// Delete a message
    pub async fn delete_message(&self, chat_id: &str, message_id: i64) -> Result<bool> {
        let url = self.config.api_url("deleteMessage");

        #[derive(Serialize)]
        struct DeleteRequest {
            chat_id: String,
            message_id: i64,
        }

        let request = DeleteRequest {
            chat_id: chat_id.to_string(),
            message_id,
        };

        let response: TelegramResponse<bool> = self.http_client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to delete Telegram message")?
            .json()
            .await
            .context("Failed to parse Telegram response")?;

        if response.ok {
            Ok(response.result.unwrap_or(false))
        } else {
            bail!("Telegram API error: {}",
                response.description.unwrap_or_else(|| "Unknown error".to_string()))
        }
    }

    /// Get updates (incoming messages) - for bot mode
    pub async fn get_updates(&self, offset: Option<i64>, limit: Option<i32>) -> Result<Vec<TelegramUpdate>> {
        let url = self.config.api_url("getUpdates");

        #[derive(Serialize)]
        struct GetUpdatesRequest {
            #[serde(skip_serializing_if = "Option::is_none")]
            offset: Option<i64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            limit: Option<i32>,
            #[serde(skip_serializing_if = "Option::is_none")]
            timeout: Option<i32>,
        }

        let request = GetUpdatesRequest {
            offset,
            limit: limit.or(Some(100)),
            timeout: Some(30),
        };

        #[derive(Deserialize)]
        struct UpdatesResponse {
            ok: bool,
            result: Option<Vec<TelegramUpdate>>,
            description: Option<String>,
        }

        let response: UpdatesResponse = self.http_client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to get Telegram updates")?
            .json()
            .await
            .context("Failed to parse Telegram response")?;

        if response.ok {
            Ok(response.result.unwrap_or_default())
        } else {
            bail!("Telegram API error: {}",
                response.description.unwrap_or_else(|| "Unknown error".to_string()))
        }
    }

    /// Send typing indicator
    pub async fn send_typing(&self, chat_id: &str) -> Result<bool> {
        let url = self.config.api_url("sendChatAction");

        #[derive(Serialize)]
        struct TypingRequest {
            chat_id: String,
            action: String,
        }

        let request = TypingRequest {
            chat_id: chat_id.to_string(),
            action: "typing".to_string(),
        };

        let response: TelegramResponse<bool> = self.http_client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to send typing indicator")?
            .json()
            .await
            .context("Failed to parse Telegram response")?;

        if response.ok {
            Ok(response.result.unwrap_or(false))
        } else {
            bail!("Telegram API error: {}",
                response.description.unwrap_or_else(|| "Unknown error".to_string()))
        }
    }

    /// Escape HTML characters for Telegram HTML parse mode
    pub fn escape_html(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    /// Format message with optional HTML formatting
    pub fn format_message(content: &str, metadata: &Option<HashMap<String, String>>) -> String {
        let mut message = content.to_string();

        // Add priority indicator if present
        if let Some(meta) = metadata {
            if let Some(priority) = meta.get("priority") {
                let emoji = match priority.as_str() {
                    "urgent" => "🚨",
                    "high" => "⚠️",
                    "low" => "ℹ️",
                    _ => "📌",
                };
                message = format!("{} <b>{}</b>\n\n{}", emoji, priority.to_uppercase(), message);
            }

            // Add timestamp if requested
            if meta.get("include_timestamp").map(|v| v == "true").unwrap_or(false) {
                let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                message = format!("{}\n\n<i>{}</i>", message, timestamp);
            }
        }

        message
    }
}

/// Guess MIME type from filename
fn guess_mime_type(filename: &str) -> String {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }.to_string()
}

#[async_trait::async_trait]
impl MessagingPlatform for TelegramClient {
    async fn send(&self, to: &str, message: &Message) -> Result<()> {
        let chat_id = if to.is_empty() {
            self.config.default_chat_id.as_ref()
                .context("No chat ID specified and no default configured")?
                .clone()
        } else {
            to.to_string()
        };

        // Send typing indicator first
        let _ = self.send_typing(&chat_id).await;

        let formatted_text = Self::format_message(&message.content, &message.metadata);

        // Handle attachments
        if let Some(attachments) = &message.attachments {
            for attachment in attachments {
                let data = base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    &attachment.data
                ).context("Failed to decode attachment data")?;

                if attachment.content_type.starts_with("image/") {
                    self.send_photo(&chat_id, data, Some(&formatted_text)).await?;
                } else {
                    self.send_document(&chat_id, data, &attachment.filename, Some(&formatted_text)).await?;
                }
                return Ok(());
            }
        }

        // Send text message
        self.send_message(&chat_id, &formatted_text).await?;
        Ok(())
    }

    fn is_configured(&self) -> bool {
        self.config.is_configured()
    }

    fn name(&self) -> &'static str {
        "telegram"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation() {
        let config = TelegramConfig::new("123456:valid_token_format");
        assert!(config.is_configured());

        let config = TelegramConfig::new("invalid_token");
        assert!(!config.is_configured());

        let config = TelegramConfig::new("");
        assert!(!config.is_configured());
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(TelegramClient::escape_html("<script>"), "&lt;script&gt;");
        assert_eq!(TelegramClient::escape_html("test & test"), "test &amp; test");
        assert_eq!(TelegramClient::escape_html("\"quoted\""), "&quot;quoted&quot;");
    }

    #[test]
    fn test_format_message() {
        let mut meta = HashMap::new();
        meta.insert("priority".to_string(), "urgent".to_string());

        let formatted = TelegramClient::format_message("Test message", &Some(meta));
        assert!(formatted.contains("🚨"));
        assert!(formatted.contains("URGENT"));
        assert!(formatted.contains("Test message"));
    }

    #[test]
    fn test_mime_type_guessing() {
        assert_eq!(guess_mime_type("file.png"), "image/png");
        assert_eq!(guess_mime_type("file.jpg"), "image/jpeg");
        assert_eq!(guess_mime_type("file.pdf"), "application/pdf");
        assert_eq!(guess_mime_type("file.unknown"), "application/octet-stream");
    }

    #[test]
    fn test_api_url_generation() {
        let config = TelegramConfig::new("123456:token");
        assert_eq!(config.api_url("sendMessage"), "https://api.telegram.org/bot123456:token/sendMessage");
        assert_eq!(config.api_url("getMe"), "https://api.telegram.org/bot123456:token/getMe");
    }
}
