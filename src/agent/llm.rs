//! LLM client with multi-provider support (OpenRouter, NVIDIA NIM)

use anyhow::{Result, Context, bail};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const NVIDIA_NIM_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";

// ============ Provider Configuration ============

/// Configuration for an LLM API provider
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Base URL for the API (e.g., "https://openrouter.ai/api/v1")
    pub base_url: String,
    /// API key for authentication
    pub api_key: String,
    /// Extra headers to include in requests (e.g., X-Title, HTTP-Referer)
    pub extra_headers: Vec<(String, String)>,
    /// Whether to include `transforms: []` in requests (OpenRouter-specific)
    pub include_transforms: bool,
}

impl ProviderConfig {
    /// Create an OpenRouter provider configuration
    pub fn openrouter(api_key: String) -> Self {
        Self {
            base_url: OPENROUTER_BASE_URL.to_string(),
            api_key,
            extra_headers: vec![
                ("HTTP-Referer".to_string(), "https://github.com/secure-agent".to_string()),
                ("X-Title".to_string(), "Secure Agent".to_string()),
            ],
            include_transforms: true,
        }
    }

    /// Create an NVIDIA NIM provider configuration
    pub fn nvidia_nim(api_key: String) -> Self {
        Self {
            base_url: NVIDIA_NIM_BASE_URL.to_string(),
            api_key,
            extra_headers: Vec::new(),
            include_transforms: false,
        }
    }

    /// Create an NVIDIA NIM provider with a custom base URL
    pub fn nvidia_nim_with_url(api_key: String, base_url: String) -> Self {
        Self {
            base_url,
            api_key,
            extra_headers: Vec::new(),
            include_transforms: false,
        }
    }
}

// ============ Multimodal Content Support ============

/// Content part for multimodal messages (text + images)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

/// Image URL for multimodal messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    /// URL or data URI (e.g., "data:image/png;base64,...")
    pub url: String,
    /// Detail level: "low", "high", or "auto"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ContentPart {
    /// Create a text content part
    pub fn text(text: impl Into<String>) -> Self {
        ContentPart::Text { text: text.into() }
    }

    /// Create an image content part from base64 data
    pub fn image_base64(base64_data: &str, media_type: &str) -> Self {
        ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: format!("data:{};base64,{}", media_type, base64_data),
                detail: None,
            },
        }
    }

    /// Create an image content part from a URL
    pub fn image_url(url: impl Into<String>) -> Self {
        ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: url.into(),
                detail: None,
            },
        }
    }
}

/// LLM API client (supports OpenRouter, NVIDIA NIM, and other OpenAI-compatible providers)
#[derive(Clone)]
pub struct OpenRouterClient {
    client: Arc<Client>,
    provider: ProviderConfig,
}

/// Type alias for future migration convenience
pub type LLMClient = OpenRouterClient;

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transforms: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    #[serde(default)]
    pub role: Option<serde_json::Value>,
    #[serde(default)]
    pub content: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_details: Option<ReasoningDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Reasoning field that some models (like Kimi K2.5, xAI) return
    /// Can be either a string or an object, so we use Value for flexibility
    #[serde(default)]
    pub reasoning: Option<serde_json::Value>,
    /// Refusal field (xai and others)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningDetails {
    pub reasoning: String,
    pub confidence: Option<f32>,
    pub steps: Vec<String>,
}

/// Tool definition for OpenAI-compatible function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub r#type: String,
    pub function: FunctionDefinition,
}

/// Function definition for tools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Tool call from LLM response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub index: Option<i32>,  // xai adds this
    pub function: FunctionCall,
}

/// Function call details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    #[serde(default)]
    pub name: String,
    /// Arguments can arrive as either a JSON string or a raw JSON object
    /// depending on the model. We normalize to a string for downstream use.
    #[serde(default, deserialize_with = "deserialize_arguments")]
    pub arguments: String,
}

/// Deserialize arguments that may be a JSON string or a JSON object/map.
/// Some models (e.g. z-ai/glm-5) return arguments as a raw object instead
/// of a stringified JSON object.
fn deserialize_arguments<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Null => Ok(String::new()),
        other => Ok(other.to_string()),
    }
}

/// Extended ChatMessage with tool support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Reasoning field that some models (like Kimi K2.5) return
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Some(serde_json::json!("user")),
            content: Some(serde_json::json!(content.into())),
            reasoning_details: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Some(serde_json::json!("assistant")),
            content: Some(serde_json::json!(content.into())),
            reasoning_details: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Some(serde_json::json!("system")),
            content: Some(serde_json::json!(content.into())),
            reasoning_details: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        }
    }

    pub fn with_reasoning(content: impl Into<String>, reasoning: impl Into<String>) -> Self {
        Self {
            role: Some(serde_json::json!("assistant")),
            content: Some(serde_json::json!(content.into())),
            reasoning_details: Some(ReasoningDetails {
                reasoning: reasoning.into(),
                confidence: None,
                steps: Vec::new(),
            }),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        }
    }

    /// Create a tool result message
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Some(serde_json::json!("tool")),
            content: Some(serde_json::json!(content.into())),
            reasoning_details: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
            reasoning: None,
            refusal: None,
        }
    }

    /// Create an assistant message with tool calls
    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Some(serde_json::json!("assistant")),
            content: Some(serde_json::json!(content.into())),
            reasoning_details: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        }
    }

    /// Check if message has tool calls
    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls.as_ref().map(|c| !c.is_empty()).unwrap_or(false)
    }

    /// Extract content as plain text, handling both string and array-of-content-parts formats.
    /// Some models return content as `"hello"`, others as `[{"type":"text","text":"hello"}]`.
    pub fn content_as_text(&self) -> Option<String> {
        self.content.as_ref().and_then(|c| match c {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Array(parts) => {
                let texts: Vec<String> = parts.iter().filter_map(|part| {
                    if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                        part.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                    } else {
                        None
                    }
                }).collect();
                if texts.is_empty() { None } else { Some(texts.join("")) }
            }
            serde_json::Value::Null => None,
            // Some models return content as a number or other type — stringify it
            other => Some(other.to_string()),
        })
    }

    /// Create a user message with an image (multimodal)
    pub fn user_with_image(text: impl Into<String>, image_base64: &str, media_type: &str) -> Self {
        Self {
            role: Some(serde_json::json!("user")),
            content: Some(serde_json::json!([
                { "type": "text", "text": text.into() },
                { "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", media_type, image_base64) } }
            ])),
            reasoning_details: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        }
    }

    /// Create a user message with multiple content parts
    pub fn user_multimodal(parts: Vec<ContentPart>) -> Self {
        let content_array: Vec<serde_json::Value> = parts.iter()
            .map(|p| serde_json::to_value(p).unwrap_or_default())
            .collect();
        Self {
            role: Some(serde_json::json!("user")),
            content: Some(serde_json::json!(content_array)),
            reasoning_details: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        }
    }
}

/// Reasoning response data returned by some models
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ReasoningResponse {
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub steps: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct StreamResponse {
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: Delta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
}

impl OpenRouterClient {
    /// Create a new OpenRouter client (backward compatible)
    pub fn new(api_key: String) -> Self {
        let provider = ProviderConfig::openrouter(api_key);
        Self {
            client: Arc::new(Client::new()),
            provider,
        }
    }

    /// Create a client with a specific provider configuration
    pub fn with_provider(config: ProviderConfig) -> Self {
        Self {
            client: Arc::new(Client::new()),
            provider: config,
        }
    }

    /// Create client from keyring (uses OpenRouter by default)
    pub fn from_keyring() -> Result<Self> {
        let api_key = crate::security::keyring::get_api_key()?;
        Ok(Self::new(api_key))
    }

    /// Create client from config (uses keyring for API key)
    pub fn from_config(_config: &crate::config::Config) -> Result<Self> {
        Self::from_keyring()
    }

    /// Get the provider configuration
    pub fn provider(&self) -> &ProviderConfig {
        &self.provider
    }

    /// Simple single-turn chat with default model
    pub async fn chat_simple(&self, message: &str) -> Result<String> {
        let messages = vec![
            ChatMessage::system("You are a helpful AI assistant."),
            ChatMessage::user(message),
        ];
        self.complete(TEXT_CHAT_MODEL, messages, Some(1024)).await
    }

    /// Simple single-turn chat with specified model
    pub async fn chat_with_model(&self, model: &str, message: &str) -> Result<String> {
        let system_prompt = r#"You are "My Agent", a helpful AI assistant with access to powerful tools.

Your capabilities include:
- File system operations (read, write, list files)
- Shell command execution
- Web search and browsing
- Multi-agent orchestration (you can spawn specialized sub-agents for complex tasks)
- Code execution and analysis

When asked what you can do or who you are, always mention that you are "My Agent" and describe your tool capabilities. You are not just a language model - you have access to the user's system and can perform actions on their behalf.

Be helpful, truthful, and concise in your responses."#;

        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(message),
        ];
        self.complete(model, messages, Some(1024)).await
    }

    /// Send a chat completion request
    pub async fn complete(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        max_tokens: Option<u32>,
    ) -> Result<String> {
        let request = ChatRequest {
            model: model.to_string(),
            messages,
            max_tokens,
            stream: None,
            transforms: if self.provider.include_transforms { Some(vec![]) } else { None },
            tools: None,
            tool_choice: None,
        };

        let mut req_builder = self.client
            .post(format!("{}/chat/completions", self.provider.base_url))
            .header("Authorization", format!("Bearer {}", self.provider.api_key));
        for (key, value) in &self.provider.extra_headers {
            req_builder = req_builder.header(key.as_str(), value.as_str());
        }
        let response = req_builder
            .json(&request)
            .send()
            .await
            .context("Failed to send request to LLM provider")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("LLM API error ({}): {}", status, body);
        }

        // Parse response with better error handling
        let body = response.text().await.context("Failed to read response body")?;

        // Try to log the problematic response for debugging
        if std::env::var("DEBUG_LLM_RESPONSES").is_ok() {
            eprintln!("DEBUG LLM Response:\n{}", crate::truncate_safe(&body, 2000));
        }

        // Parse as raw Value first for maximum flexibility
        let raw_response: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| {
                anyhow::anyhow!("Failed to parse JSON response: {} (body: {})",
                    e, crate::truncate_safe(&body, 500))
            })?;

        // Extract content from the response using path navigation
        // Handle both string content and array-of-content-parts formats
        let content_value = raw_response
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|msg| msg.get("content"));

        let content = match content_value {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(parts)) => {
                parts.iter().filter_map(|part| {
                    if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                        part.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                    } else {
                        None
                    }
                }).collect::<Vec<_>>().join("")
            }
            _ => String::new(),
        };

        Ok(content)
    }

    /// Stream a chat completion request with callback for each chunk
    ///
    /// This enables real-time display of LLM responses as they're generated.
    /// The callback is called for each text chunk received.
    pub async fn stream_complete(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        max_tokens: Option<u32>,
        mut on_chunk: impl FnMut(&str) + Send,
    ) -> Result<String> {
        let request = ChatRequest {
            model: model.to_string(),
            messages,
            max_tokens,
            stream: Some(true),
            transforms: if self.provider.include_transforms { Some(vec![]) } else { None },
            tools: None,
            tool_choice: None,
        };

        let mut req_builder = self.client
            .post(format!("{}/chat/completions", self.provider.base_url))
            .header("Authorization", format!("Bearer {}", self.provider.api_key));
        for (key, value) in &self.provider.extra_headers {
            req_builder = req_builder.header(key.as_str(), value.as_str());
        }
        let response = req_builder
            .json(&request)
            .send()
            .await
            .context("Failed to send streaming request to LLM provider")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("LLM streaming API error ({}): {}", status, body);
        }

        // Use bytes_stream for SSE parsing
        let mut stream = response.bytes_stream();
        let mut full_content = String::new();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Failed to read stream chunk")?;
            let chunk_str = String::from_utf8_lossy(&chunk);
            buffer.push_str(&chunk_str);

            // Parse SSE events
            while let Some(pos) = buffer.find("\n\n") {
                let event_str = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                // Handle data lines
                for line in event_str.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            continue;
                        }

                        if let Ok(stream_resp) = serde_json::from_str::<StreamResponse>(data) {
                            if let Some(choice) = stream_resp.choices.first() {
                                if let Some(content) = &choice.delta.content {
                                    on_chunk(content);
                                    full_content.push_str(content);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(full_content)
    }

    /// Send a chat completion request and return both content and reasoning
    pub async fn complete_with_reasoning(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        max_tokens: Option<u32>,
    ) -> Result<(String, Option<ReasoningResponse>)> {
        let request = ChatRequest {
            model: model.to_string(),
            messages,
            max_tokens,
            stream: None,
            transforms: if self.provider.include_transforms { Some(vec![]) } else { None },
            tools: None,
            tool_choice: None,
        };

        let mut req_builder = self.client
            .post(format!("{}/chat/completions", self.provider.base_url))
            .header("Authorization", format!("Bearer {}", self.provider.api_key));
        for (key, value) in &self.provider.extra_headers {
            req_builder = req_builder.header(key.as_str(), value.as_str());
        }
        let response = req_builder
            .json(&request)
            .send()
            .await
            .context("Failed to send request to LLM provider")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("LLM API error ({}): {}", status, body);
        }

        // Parse as raw Value first for maximum flexibility
        let body = response.text().await.context("Failed to read response body")?;
        let raw_response: serde_json::Value = serde_json::from_str(&body)
            .context("Failed to parse JSON response")?;

        // Extract content from the response using path navigation
        // Handle both string and array-of-content-parts formats
        let content_value = raw_response
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|msg| msg.get("content"));

        let content = match content_value {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(parts)) => {
                parts.iter().filter_map(|part| {
                    if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                        part.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                    } else {
                        None
                    }
                }).collect::<Vec<_>>().join("")
            }
            _ => String::new(),
        };

        Ok((content, None))
    }

    /// Create a new message with preserved reasoning from previous response
    pub fn create_message_with_reasoning(
        &self,
        content: impl Into<String>,
        previous_reasoning: Option<ReasoningResponse>,
    ) -> ChatMessage {
        let mut message = ChatMessage::assistant(content);

        if let Some(prev_reasoning) = previous_reasoning {
            message.reasoning_details = Some(ReasoningDetails {
                reasoning: prev_reasoning.reasoning,
                confidence: prev_reasoning.confidence,
                steps: prev_reasoning.steps,
            });
        }

        message
    }

    /// Send a chat completion request with tool support
    pub async fn complete_with_tools(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
        max_tokens: Option<u32>,
    ) -> Result<ChatMessage> {
        // Estimate token count for debugging
        let msg_tokens: usize = messages.iter()
            .filter_map(|m| m.content.as_ref())
            .map(|c| match c {
                serde_json::Value::String(s) => s.len() / 4,
                _ => 0
            })
            .sum();
        let tool_tokens: usize = tools.iter()
            .map(|t| (t.function.description.len() + t.function.name.len()) / 4)
            .sum();

        tracing::debug!("LLM request: {} messages (~{} tokens), {} tools (~{} tokens)",
                       messages.len(), msg_tokens, tools.len(), tool_tokens);

        // Warn if request is very large
        if msg_tokens > 50000 {
            tracing::warn!("Large request: ~{} tokens in messages, may hit context limit", msg_tokens);
        }

        let request = ChatRequest {
            model: model.to_string(),
            messages,
            max_tokens,
            stream: None,
            transforms: if self.provider.include_transforms { Some(vec![]) } else { None },
            tools: Some(tools),
            tool_choice: Some("auto".to_string()),
        };

        let mut req_builder = self.client
            .post(format!("{}/chat/completions", self.provider.base_url))
            .header("Authorization", format!("Bearer {}", self.provider.api_key));
        for (key, value) in &self.provider.extra_headers {
            req_builder = req_builder.header(key.as_str(), value.as_str());
        }
        let response = req_builder
            .json(&request)
            .send()
            .await
            .context("Failed to send request to LLM provider")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("LLM API error ({}): {}", status, body);
        }

        // Parse as raw Value first for maximum provider compatibility.
        // Strict struct deserialization breaks on models that return non-standard
        // field types (objects where strings are expected, etc.).
        let body = response.text().await
            .context("Failed to get response text")?;

        let raw: serde_json::Value = serde_json::from_str(body.trim())
            .context("Failed to parse JSON response")?;

        let message = raw
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("message"))
            .ok_or_else(|| anyhow::anyhow!("No message in response"))?;

        // Extract content (string or array-of-parts)
        let content = message.get("content").cloned();

        // Extract tool_calls array if present
        let tool_calls: Option<Vec<ToolCall>> = message
            .get("tool_calls")
            .and_then(|tc| tc.as_array())
            .map(|arr| {
                arr.iter().filter_map(|tc| {
                    let id = tc.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let tc_type = tc.get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("function")
                        .to_string();
                    let func = tc.get("function")?;
                    let name = func.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    // arguments: accept both string and raw object
                    let arguments = match func.get("arguments") {
                        Some(serde_json::Value::String(s)) => s.clone(),
                        Some(serde_json::Value::Null) | None => String::new(),
                        Some(other) => other.to_string(),
                    };
                    Some(ToolCall {
                        id,
                        r#type: tc_type,
                        index: tc.get("index").and_then(|v| v.as_i64()).map(|i| i as i32),
                        function: FunctionCall { name, arguments },
                    })
                }).collect()
            });

        // Extract role
        let role = message.get("role").cloned()
            .or(Some(serde_json::json!("assistant")));

        Ok(ChatMessage {
            role,
            content,
            reasoning_details: None,
            tool_calls,
            tool_call_id: message.get("tool_call_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            name: message.get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            reasoning: message.get("reasoning").cloned(),
            refusal: message.get("refusal").cloned(),
        })
    }

    /// Check if a model supports reasoning preservation
    pub fn model_supports_reasoning(&self, model: &str) -> bool {
        // Models that are known to support reasoning_details
        OpenRouterClient::reasoning_models().contains(&model)
    }

    /// Models that support reasoning preservation
    fn reasoning_models() -> Vec<&'static str> {
        vec![
            "deepseek/deepseek-r1",
            "anthropic/claude-3.5-sonnet",
            "anthropic/claude-3-opus",
            "anthropic/claude-3-haiku",
            "google/gemini-pro-1.5",
            "google/gemini-flash-1.5",
            "x-ai/grok-4.1-fast",
            "moonshotai/kimi-k2.5",
        ]
    }

    /// Check if a model supports vision/images
    pub fn model_supports_vision(model: &str) -> bool {
        supports_vision(model)
    }

    /// List available models
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let mut req_builder = self.client
            .get(format!("{}/models", self.provider.base_url))
            .header("Authorization", format!("Bearer {}", self.provider.api_key));
        for (key, value) in &self.provider.extra_headers {
            req_builder = req_builder.header(key.as_str(), value.as_str());
        }
        let response = req_builder
            .send()
            .await
            .context("Failed to fetch models")?;

        #[derive(Deserialize)]
        struct ModelsResponse {
            data: Vec<ModelInfo>,
        }

        let models: ModelsResponse = response
            .json()
            .await
            .context("Failed to parse models response")?;

        Ok(models.data)
    }
}

/// Resolve the correct LLM client for a given model ID.
///
/// Checks the configured NVIDIA NIM model prefixes (from config.toml `[nvidia]`)
/// and returns a NIM-configured client if the model matches. Otherwise returns
/// an OpenRouter client.
pub fn client_for_model(model: &str) -> Result<OpenRouterClient> {
    let config = crate::config::Config::load()?;

    // Check if the model matches any NVIDIA NIM prefix
    let is_nim = config.nvidia.model_prefixes.iter().any(|prefix| model.starts_with(prefix.as_str()));

    if is_nim {
        let api_key = crate::security::keyring::get_nvidia_api_key()
            .context("NVIDIA NIM API key not set. Run 'my-agent config --set-nvidia-key YOUR_KEY' first.")?;
        let provider = ProviderConfig::nvidia_nim_with_url(api_key, config.nvidia.base_url);
        Ok(OpenRouterClient::with_provider(provider))
    } else {
        OpenRouterClient::from_keyring()
    }
}

/// Models that support vision/images
pub const VISION_MODELS: &[&str] = &[
    "bytedance-seed/seed-1.6-flash",
    "google/gemini-flash-1.5",
    "google/gemini-pro-1.5",
    "openai/gpt-4o",
    "openai/gpt-4o-mini",
    "openai/gpt-4-turbo",
    "anthropic/claude-3.5-sonnet",
    "anthropic/claude-3-opus",
    "anthropic/claude-3-haiku",
    "meta-llama/llama-3.2-11b-vision-instruct",
    "meta-llama/llama-3.2-90b-vision-instruct",
];

/// Check if a model supports vision (standalone function)
pub fn supports_vision(model: &str) -> bool {
    VISION_MODELS.iter().any(|m| model.contains(m))
}

/// Model information from OpenRouter
#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub context_length: u32,
    pub pricing: ModelPricing,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelPricing {
    pub prompt: String,
    pub completion: String,
}

// ============ Model Registry for Cost-Optimized Routing ============

/// Free models available via OpenRouter
pub const FREE_MODELS: &[(&str, &str)] = &[
    // Coding
    ("openrouter/pony-alpha", "Excellent free coding model"),
    ("qwen/qwen-2.5-coder-32b-instruct", "Alternative free coding model"),
    // Research
    ("perplexity/sonar", "Free with web search"),
    // Reasoning
    ("deepseek/deepseek-r1", "Free reasoning model"),
    // General
    ("google/gemma-2-9b-it", "Free, good reasoning"),
    ("meta-llama/llama-3.1-8b-instruct", "Free, versatile"),
    ("mistralai/mistral-7b-instruct", "Free, very fast"),
];

/// Text chat model (Grok 4.1 Fast - cheaper tier)
pub const TEXT_CHAT_MODEL: &str = "x-ai/grok-4.1-fast";

/// Get the recommended free model for a task type
pub fn get_free_model_for_task(task: TaskType) -> &'static str {
    match task {
        TaskType::Code => "openrouter/pony-alpha",
        TaskType::CodeComplex => "qwen/qwen-2.5-coder-32b-instruct",
        TaskType::Research => "perplexity/sonar",
        TaskType::Reasoning => "deepseek/deepseek-r1",
        TaskType::Analysis => "google/gemma-2-9b-it",
        TaskType::Quick => "mistralai/mistral-7b-instruct",
        TaskType::General => "meta-llama/llama-3.1-8b-instruct",
    }
}

/// Task types for model routing
#[derive(Debug, Clone, Copy)]
pub enum TaskType {
    Code,
    CodeComplex,
    Research,
    Reasoning,
    Analysis,
    Quick,
    General,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_message_creation() {
        let user_msg = ChatMessage::user("Hello");
        assert_eq!(user_msg.role, Some(serde_json::json!("user")));
        assert_eq!(user_msg.content, Some(serde_json::json!("Hello")));

        let sys_msg = ChatMessage::system("You are helpful");
        assert_eq!(sys_msg.role, Some(serde_json::json!("system")));
    }

    #[test]
    fn test_function_call_arguments_string() {
        // Standard format: arguments as a JSON string
        let json = r#"{"name":"read_file","arguments":"{\"path\":\"/tmp/test\"}"}"#;
        let fc: FunctionCall = serde_json::from_str(json).unwrap();
        assert_eq!(fc.name, "read_file");
        assert_eq!(fc.arguments, r#"{"path":"/tmp/test"}"#);
    }

    #[test]
    fn test_function_call_arguments_object() {
        // Non-standard format (z-ai/glm-5): arguments as a raw object
        let json = r#"{"name":"read_file","arguments":{"path":"/tmp/test"}}"#;
        let fc: FunctionCall = serde_json::from_str(json).unwrap();
        assert_eq!(fc.name, "read_file");
        // Should be serialized to a string
        let parsed: serde_json::Value = serde_json::from_str(&fc.arguments).unwrap();
        assert_eq!(parsed["path"], "/tmp/test");
    }

    #[test]
    fn test_function_call_arguments_null() {
        let json = r#"{"name":"list_skills","arguments":null}"#;
        let fc: FunctionCall = serde_json::from_str(json).unwrap();
        assert_eq!(fc.name, "list_skills");
        assert_eq!(fc.arguments, "");
    }

    #[test]
    fn test_content_as_text_string() {
        let msg = ChatMessage::assistant("Hello world");
        assert_eq!(msg.content_as_text(), Some("Hello world".to_string()));
    }

    #[test]
    fn test_content_as_text_array() {
        // Content as array of content parts (some models do this)
        let msg = ChatMessage {
            role: Some(serde_json::json!("assistant")),
            content: Some(serde_json::json!([
                {"type": "text", "text": "Hello "},
                {"type": "text", "text": "world"}
            ])),
            reasoning_details: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        };
        assert_eq!(msg.content_as_text(), Some("Hello world".to_string()));
    }

    #[test]
    fn test_content_as_text_null() {
        let msg = ChatMessage {
            role: Some(serde_json::json!("assistant")),
            content: None,
            reasoning_details: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        };
        assert_eq!(msg.content_as_text(), None);
    }
}
