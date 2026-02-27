//! Model Failover Chain - automatic model fallback on errors
//!
//! If the primary model is down, rate-limited, or fails, automatically
//! falls back to alternative models in a configured chain.

use anyhow::Result;
use std::collections::HashMap;
use tracing::{info, warn};

use crate::agent::llm::{self, OpenRouterClient, ChatMessage, ToolDefinition};

/// Error classification for failover decisions
#[derive(Debug, Clone, PartialEq)]
pub enum FailoverError {
    RateLimit,
    ModelDown,
    AuthError,
    ContextOverflow,
    Unknown(String),
}

impl std::fmt::Display for FailoverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FailoverError::RateLimit => write!(f, "rate_limit"),
            FailoverError::ModelDown => write!(f, "model_down"),
            FailoverError::AuthError => write!(f, "auth_error"),
            FailoverError::ContextOverflow => write!(f, "context_overflow"),
            FailoverError::Unknown(msg) => write!(f, "unknown: {}", msg),
        }
    }
}

/// Configuration for a single model in the chain
#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub model_id: String,
    pub max_tokens: u32,
    pub timeout_secs: u64,
}

impl ModelConfig {
    pub fn new(model_id: impl Into<String>) -> Self {
        Self {
            model_id: model_id.into(),
            max_tokens: 2048,
            timeout_secs: 60,
        }
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}

/// Failover configuration from config.toml
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct FailoverConfig {
    /// Maps role name -> list of fallback model IDs (first is primary)
    #[serde(default)]
    pub chains: HashMap<String, Vec<String>>,
}

/// Failover-aware client wrapper
pub struct FailoverClient {
    client: OpenRouterClient,
    chains: HashMap<String, Vec<ModelConfig>>,
}

impl FailoverClient {
    /// Create a new failover client with model chains
    pub fn new(client: OpenRouterClient) -> Self {
        Self {
            client,
            chains: HashMap::new(),
        }
    }

    /// Create from config
    pub fn from_config(client: OpenRouterClient, config: &FailoverConfig) -> Self {
        let mut chains = HashMap::new();
        for (role, models) in &config.chains {
            let chain: Vec<ModelConfig> = models.iter()
                .map(ModelConfig::new)
                .collect();
            if !chain.is_empty() {
                chains.insert(role.clone(), chain);
            }
        }
        Self { client, chains }
    }

    /// Set a failover chain for a role
    pub fn set_chain(&mut self, role: &str, models: Vec<ModelConfig>) {
        self.chains.insert(role.to_string(), models);
    }

    /// Build default chains from the current config
    pub fn with_default_chains(mut self) -> Self {
        if let Ok(config) = crate::config::Config::load() {
            // Build failover chains for each role using configured models
            let chat_chain = vec![
                ModelConfig::new(&config.models.chat),
                ModelConfig::new(&config.models.utility),
                ModelConfig::new(&config.models.research),
            ];
            self.chains.insert("chat".to_string(), chat_chain);

            let code_chain = vec![
                ModelConfig::new(&config.models.code),
                ModelConfig::new(&config.models.chat),
                ModelConfig::new(&config.models.utility),
            ];
            self.chains.insert("code".to_string(), code_chain);

            let utility_chain = vec![
                ModelConfig::new(&config.models.utility),
                ModelConfig::new(&config.models.chat),
                ModelConfig::new(&config.models.research),
            ];
            self.chains.insert("utility".to_string(), utility_chain);
        }
        self
    }

    /// Complete with tools, trying failover models on failure
    pub async fn complete_with_failover(
        &self,
        role: &str,
        primary_model: &str,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
        max_tokens: Option<u32>,
    ) -> Result<ChatMessage> {
        // Build the chain: primary model first, then any configured fallbacks
        let mut model_chain: Vec<&str> = vec![primary_model];

        if let Some(chain) = self.chains.get(role) {
            for config in chain {
                if config.model_id != primary_model {
                    model_chain.push(&config.model_id);
                }
            }
        }

        // Deduplicate while preserving order
        let mut seen = std::collections::HashSet::new();
        model_chain.retain(|m| seen.insert(*m));

        let mut last_error = None;

        for (i, model) in model_chain.iter().enumerate() {
            let tokens = max_tokens.or_else(|| {
                self.chains.get(role)
                    .and_then(|c| c.iter().find(|m| m.model_id == **model))
                    .map(|m| m.max_tokens)
            });

            // Resolve the correct provider client for this model
            let client = llm::client_for_model(model).unwrap_or_else(|_| self.client.clone());

            match client.complete_with_tools(
                model,
                messages.clone(),
                tools.clone(),
                tokens,
            ).await {
                Ok(response) => {
                    if i > 0 {
                        info!("Failover succeeded: {} -> {} (attempt {})", primary_model, model, i + 1);
                    }
                    return Ok(response);
                }
                Err(e) => {
                    let error_str = e.to_string();
                    let classified = classify_error(&error_str);

                    if should_failover(&classified) && i < model_chain.len() - 1 {
                        warn!(
                            "Model {} failed ({}), failing over to {}",
                            model,
                            classified,
                            model_chain[i + 1]
                        );
                        last_error = Some(e);
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("All models in failover chain failed")))
    }

    /// Simple complete (no tools) with failover
    pub async fn complete_with_failover_simple(
        &self,
        role: &str,
        primary_model: &str,
        messages: Vec<ChatMessage>,
        max_tokens: Option<u32>,
    ) -> Result<String> {
        let mut model_chain: Vec<&str> = vec![primary_model];

        if let Some(chain) = self.chains.get(role) {
            for config in chain {
                if config.model_id != primary_model {
                    model_chain.push(&config.model_id);
                }
            }
        }

        let mut seen = std::collections::HashSet::new();
        model_chain.retain(|m| seen.insert(*m));

        let mut last_error = None;

        for (i, model) in model_chain.iter().enumerate() {
            // Resolve the correct provider client for this model
            let client = llm::client_for_model(model).unwrap_or_else(|_| self.client.clone());

            match client.complete(model, messages.clone(), max_tokens).await {
                Ok(response) => {
                    if i > 0 {
                        info!("Failover succeeded: {} -> {}", primary_model, model);
                    }
                    return Ok(response);
                }
                Err(e) => {
                    let classified = classify_error(&e.to_string());
                    if should_failover(&classified) && i < model_chain.len() - 1 {
                        warn!("Model {} failed ({}), trying next", model, classified);
                        last_error = Some(e);
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("All models failed")))
    }

    /// Get the underlying client
    pub fn client(&self) -> &OpenRouterClient {
        &self.client
    }
}

/// Classify an error from the API response
pub fn classify_error(error_str: &str) -> FailoverError {
    let lower = error_str.to_lowercase();

    if lower.contains("rate limit") || lower.contains("429") || lower.contains("too many requests") {
        FailoverError::RateLimit
    } else if lower.contains("model not available")
        || lower.contains("503")
        || lower.contains("502")
        || lower.contains("model is currently overloaded")
        || lower.contains("service unavailable")
    {
        FailoverError::ModelDown
    } else if lower.contains("401") || lower.contains("403")
        || lower.contains("unauthorized") || lower.contains("invalid api key")
    {
        FailoverError::AuthError
    } else if lower.contains("context length")
        || lower.contains("max tokens")
        || lower.contains("too long")
        || lower.contains("context_length_exceeded")
    {
        FailoverError::ContextOverflow
    } else {
        FailoverError::Unknown(error_str.to_string())
    }
}

/// Determine if we should try the next model in the chain
pub fn should_failover(error: &FailoverError) -> bool {
    matches!(error, FailoverError::RateLimit | FailoverError::ModelDown | FailoverError::ContextOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_error() {
        assert_eq!(classify_error("429 Too Many Requests"), FailoverError::RateLimit);
        assert_eq!(classify_error("503 Service Unavailable"), FailoverError::ModelDown);
        assert_eq!(classify_error("401 Unauthorized"), FailoverError::AuthError);
        assert_eq!(classify_error("context_length_exceeded"), FailoverError::ContextOverflow);
    }

    #[test]
    fn test_should_failover() {
        assert!(should_failover(&FailoverError::RateLimit));
        assert!(should_failover(&FailoverError::ModelDown));
        assert!(should_failover(&FailoverError::ContextOverflow));
        assert!(!should_failover(&FailoverError::AuthError));
        assert!(!should_failover(&FailoverError::Unknown("test".into())));
    }
}
