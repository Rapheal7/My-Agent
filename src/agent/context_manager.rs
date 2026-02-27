//! Context management for long conversations
//!
//! Provides token-aware message management to prevent context overflow.

use anyhow::Result;
use crate::agent::llm::ChatMessage;

/// Configuration for context management
#[derive(Debug, Clone)]
pub struct ContextConfig {
    /// Maximum context window in tokens
    pub model_context_limit: usize,
    /// Maximum tokens to use (leaving room for response)
    pub max_context_tokens: usize,
    /// Maximum number of messages
    pub max_messages: usize,
    /// Token count at which to warn
    pub warning_threshold: usize,
    /// Reserve tokens for response
    pub reserve_tokens: usize,
    /// Token threshold at which to flush memories before compaction
    pub memory_flush_threshold: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        let max_context_tokens = 100000;
        Self {
            model_context_limit: 120000,
            max_context_tokens,
            max_messages: 50,
            warning_threshold: 80000,
            reserve_tokens: 4096,
            memory_flush_threshold: max_context_tokens.saturating_sub(10000),
        }
    }
}

/// Get context config appropriate for a model
pub fn context_config_for_model(model: &str) -> ContextConfig {
    let model_lower = model.to_lowercase();
    let model_context_limit = if model_lower.contains("claude-3.5")
        || model_lower.contains("claude-4")
        || model_lower.contains("claude-3-opus")
    {
        // Claude 3.5/4 models have 200K context
        200000
    } else if model_lower.contains("gpt-4") || model_lower.contains("claude") {
        128000
    } else if model_lower.contains("gpt-3.5") {
        16000
    } else {
        120000
    };

    // Use 88% of limit (was 85%) for more headroom
    let max_context_tokens = (model_context_limit as f64 * 0.88) as usize;

    ContextConfig {
        model_context_limit,
        max_context_tokens,
        warning_threshold: (model_context_limit as f64 * 0.7) as usize,
        memory_flush_threshold: max_context_tokens.saturating_sub(10000),
        ..Default::default()
    }
}

/// Summary statistics
#[derive(Debug, Clone)]
pub struct SummaryStats {
    pub messages_compressed: usize,
    pub original_tokens: usize,
    pub summary_tokens: usize,
}

/// Result of context management
pub struct ManagedContext {
    pub messages: Vec<ChatMessage>,
    pub estimated_tokens: usize,
    pub max_tokens: usize,
    /// Warning about context usage
    pub warning: Option<String>,
    /// Whether messages were trimmed
    pub was_trimmed: bool,
}

/// Context manager that tracks token usage and manages messages
#[derive(Debug, Clone)]
pub struct ContextManager {
    pub config: ContextConfig,
    estimated_tokens: usize,
    summary_stats: Option<SummaryStats>,
}

impl ContextManager {
    /// Create a new context manager
    pub fn new(config: ContextConfig) -> Self {
        Self {
            config,
            estimated_tokens: 0,
            summary_stats: None,
        }
    }

    /// Estimate tokens in a string (~4 chars per token)
    fn estimate_str_tokens(text: &str) -> usize {
        text.len() / 4
    }

    /// Estimate tokens in a set of ChatMessages
    pub fn estimate_message_tokens(messages: &[ChatMessage]) -> usize {
        messages.iter()
            .map(|m| {
                // Handle content as string, array, or JSON value
                let content_tokens = m.content.as_ref()
                    .map(|c| match c {
                        serde_json::Value::String(s) => Self::estimate_str_tokens(s),
                        other => Self::estimate_str_tokens(&other.to_string()),
                    })
                    .unwrap_or(0);
                let tool_tokens = m.tool_calls.as_ref()
                    .map(|tcs| tcs.iter().map(|tc| Self::estimate_str_tokens(&tc.function.arguments)).sum::<usize>())
                    .unwrap_or(0);
                content_tokens + tool_tokens + 4 // 4 tokens overhead per message
            })
            .sum()
    }

    /// Manage context: combine messages with optional system prompt and memory
    pub async fn manage_context(
        &mut self,
        messages: Vec<ChatMessage>,
        system_prompt: Option<impl Into<String>>,
        memory_context: Option<String>,
    ) -> Result<ManagedContext> {
        let mut result_messages = Vec::new();

        // Add system prompt if provided
        if let Some(prompt) = system_prompt {
            let mut full_prompt = prompt.into();
            if let Some(memory) = memory_context {
                if !memory.is_empty() {
                    full_prompt = format!("{}\n\n---\n\n## Relevant Memory\n\n{}", full_prompt, memory);
                }
            }
            result_messages.push(ChatMessage::system(full_prompt));
        } else if let Some(memory) = memory_context {
            if !memory.is_empty() {
                result_messages.push(ChatMessage::system(format!("## Relevant Memory\n\n{}", memory)));
            }
        }

        // Add conversation messages
        result_messages.extend(messages);

        // Trim if exceeds limits
        let mut total_tokens = Self::estimate_message_tokens(&result_messages);

        if total_tokens > self.config.max_context_tokens && result_messages.len() > 2 {
            // Keep system prompt (first) and trim oldest non-system messages
            let original_count = result_messages.len();
            while total_tokens > self.config.max_context_tokens && result_messages.len() > 2 {
                result_messages.remove(1); // Remove oldest after system prompt
                total_tokens = Self::estimate_message_tokens(&result_messages);
            }
            let removed = original_count - result_messages.len();
            if removed > 0 {
                tracing::warn!("Trimmed {} messages to fit context window", removed);
                self.summary_stats = Some(SummaryStats {
                    messages_compressed: removed,
                    original_tokens: total_tokens + (removed * 100), // rough estimate
                    summary_tokens: total_tokens,
                });
            }
        }

        self.estimated_tokens = total_tokens;

        let was_trimmed = self.summary_stats.is_some();
        let warning = if total_tokens > self.config.warning_threshold {
            Some(format!(
                "Context usage high: {} tokens ({:.0}% of {})",
                total_tokens,
                total_tokens as f64 / self.config.model_context_limit as f64 * 100.0,
                self.config.model_context_limit
            ))
        } else {
            None
        };

        Ok(ManagedContext {
            messages: result_messages,
            estimated_tokens: total_tokens,
            max_tokens: self.config.max_context_tokens,
            warning,
            was_trimmed,
        })
    }

    /// Clear any cached summaries
    pub async fn clear_cache(&mut self) {
        self.summary_stats = None;
        self.estimated_tokens = 0;
    }

    /// Get summary statistics if available
    pub async fn get_summary_stats(&self) -> Option<SummaryStats> {
        self.summary_stats.clone()
    }

    /// Get estimated token count
    pub fn estimated_tokens(&self) -> usize {
        self.estimated_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_context_config() {
        let config = ContextConfig::default();
        assert_eq!(config.model_context_limit, 120000);
        assert_eq!(config.max_context_tokens, 100000);
        assert_eq!(config.max_messages, 50);
        assert_eq!(config.memory_flush_threshold, 90000); // 100000 - 10000
    }

    #[test]
    fn test_context_config_for_gpt4() {
        let config = context_config_for_model("gpt-4-turbo");
        assert_eq!(config.model_context_limit, 128000);
        // 88% of 128000 = 112640
        assert_eq!(config.max_context_tokens, 112640);
        assert_eq!(config.memory_flush_threshold, 102640); // 112640 - 10000
    }

    #[test]
    fn test_context_config_for_claude_35() {
        let config = context_config_for_model("claude-3.5-sonnet");
        assert_eq!(config.model_context_limit, 200000);
        // 88% of 200000 = 176000
        assert_eq!(config.max_context_tokens, 176000);
        assert_eq!(config.memory_flush_threshold, 166000); // 176000 - 10000
    }

    #[test]
    fn test_context_config_for_claude_4() {
        let config = context_config_for_model("claude-4-opus");
        assert_eq!(config.model_context_limit, 200000);
        assert_eq!(config.max_context_tokens, 176000);
    }

    #[test]
    fn test_context_config_for_gpt35() {
        let config = context_config_for_model("gpt-3.5-turbo");
        assert_eq!(config.model_context_limit, 16000);
        // 88% of 16000 = 14080
        assert_eq!(config.max_context_tokens, 14080);
    }

    #[test]
    fn test_context_config_for_unknown_model() {
        let config = context_config_for_model("some-random-model");
        assert_eq!(config.model_context_limit, 120000);
    }

    #[test]
    fn test_memory_flush_threshold_below_max() {
        // Flush threshold should always be below max context tokens
        for model in &["gpt-4", "claude-3.5-sonnet", "claude-4-opus", "gpt-3.5-turbo", "unknown"] {
            let config = context_config_for_model(model);
            assert!(config.memory_flush_threshold < config.max_context_tokens,
                "For model '{}': flush ({}) should be < max ({})",
                model, config.memory_flush_threshold, config.max_context_tokens);
        }
    }

    #[test]
    fn test_estimate_str_tokens() {
        // ~4 chars per token
        assert_eq!(ContextManager::estimate_str_tokens(""), 0);
        assert_eq!(ContextManager::estimate_str_tokens("test"), 1); // 4 chars = 1 token
        assert_eq!(ContextManager::estimate_str_tokens("hello world!!"), 3); // 13 chars ~ 3 tokens
    }
}
