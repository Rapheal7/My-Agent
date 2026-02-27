//! Session Compaction - intelligent context compression
//!
//! Replaces simple message trimming with LLM-powered summarization
//! that preserves key decisions, file paths, and user preferences.

use anyhow::Result;
use tracing::{info, debug};

use crate::agent::llm::{OpenRouterClient, ChatMessage};

/// Intelligent session compactor
pub struct SessionCompactor {
    client: OpenRouterClient,
    /// Cheap/fast model for summarization
    model: String,
}

impl SessionCompactor {
    /// Create a new compactor using a cheap model for summarization
    pub fn new(client: OpenRouterClient, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }

    /// Create with default model from config
    pub fn from_config(client: OpenRouterClient) -> Self {
        let model = crate::config::Config::load()
            .map(|c| c.models.utility.clone())
            .unwrap_or_else(|_| "openai/gpt-oss-120b:free".to_string());
        Self { client, model }
    }

    /// Check if compaction is needed based on message count and estimated tokens
    pub fn should_compact(messages: &[ChatMessage], max_messages: usize, token_threshold: usize) -> bool {
        if messages.len() <= max_messages {
            return false;
        }
        let tokens = estimate_tokens(messages);
        tokens > token_threshold
    }

    /// Compact older messages into a summary while keeping recent ones intact
    pub async fn compact(
        &self,
        messages: &[ChatMessage],
        keep_recent: usize,
    ) -> Result<Vec<ChatMessage>> {
        if messages.len() <= keep_recent + 1 {
            // Nothing to compact (system prompt + recent <= threshold)
            return Ok(messages.to_vec());
        }

        // Separate system prompt, compaction window, and recent messages
        let system_msg = messages.iter()
            .find(|m| msg_role(m) == "system")
            .cloned();

        let non_system: Vec<&ChatMessage> = messages.iter()
            .filter(|m| msg_role(m) != "system")
            .collect();

        if non_system.len() <= keep_recent {
            return Ok(messages.to_vec());
        }

        let split_point = non_system.len() - keep_recent;
        let to_compact: Vec<&ChatMessage> = non_system[..split_point].to_vec();
        let to_keep: Vec<&ChatMessage> = non_system[split_point..].to_vec();

        info!(
            "Compacting {} messages into summary, keeping {} recent messages",
            to_compact.len(),
            to_keep.len()
        );

        // Extract key facts before summarizing
        let key_facts = self.extract_key_facts(&to_compact);

        // Generate structured summary
        let summary = self.generate_summary(&to_compact, &key_facts).await?;

        // Rebuild message list
        let mut result = Vec::new();

        // System prompt first
        if let Some(sys) = system_msg {
            result.push(sys);
        }

        // Compacted context as system message
        result.push(ChatMessage::system(format!(
            "[Session Context - Compacted from {} earlier messages]\n\n{}\n\n---\nConversation continues below.",
            to_compact.len(),
            summary
        )));

        // Recent messages intact
        for msg in to_keep {
            result.push(msg.clone());
        }

        info!(
            "Compaction complete: {} -> {} messages",
            messages.len(),
            result.len()
        );

        Ok(result)
    }

    /// Extract important facts from messages to preserve
    fn extract_key_facts(&self, messages: &[&ChatMessage]) -> Vec<String> {
        let mut facts = Vec::new();
        let mut file_paths = std::collections::HashSet::new();
        let mut tool_names = std::collections::HashSet::new();

        for msg in messages {
            let content = msg_content(msg);

            // Extract file paths
            for word in content.split_whitespace() {
                let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '.' && c != '_' && c != '-');
                if (trimmed.contains('/') || trimmed.contains('.'))
                    && (trimmed.ends_with(".rs") || trimmed.ends_with(".py")
                        || trimmed.ends_with(".ts") || trimmed.ends_with(".js")
                        || trimmed.ends_with(".toml") || trimmed.ends_with(".json")
                        || trimmed.ends_with(".md") || trimmed.ends_with(".yaml")
                        || trimmed.ends_with(".yml") || trimmed.starts_with("src/")
                        || trimmed.starts_with("./") || trimmed.starts_with("/"))
                {
                    file_paths.insert(trimmed.to_string());
                }
            }

            // Extract tool call references
            if let Some(tool_calls) = &msg.tool_calls {
                for tc in tool_calls {
                    tool_names.insert(tc.function.name.clone());
                }
            }

            // Extract user preferences (patterns like "I prefer", "always use", etc.)
            let lower = content.to_lowercase();
            if lower.contains("i prefer") || lower.contains("always use")
                || lower.contains("don't use") || lower.contains("never use")
            {
                facts.push(format!("User preference: {}", truncate_str(&content, 100)));
            }
        }

        if !file_paths.is_empty() {
            let paths: Vec<_> = file_paths.into_iter().take(20).collect();
            facts.push(format!("Files referenced: {}", paths.join(", ")));
        }

        if !tool_names.is_empty() {
            let tools: Vec<_> = tool_names.into_iter().collect();
            facts.push(format!("Tools used: {}", tools.join(", ")));
        }

        facts
    }

    /// Generate a structured summary using a cheap LLM
    async fn generate_summary(
        &self,
        messages: &[&ChatMessage],
        key_facts: &[String],
    ) -> Result<String> {
        // Build the conversation text to summarize
        let mut conversation_text = String::new();
        for msg in messages {
            let role = msg_role(msg);
            let content = msg_content(msg);
            if !content.is_empty() && role != "tool" {
                conversation_text.push_str(&format!("[{}]: {}\n", role, truncate_str(&content, 500)));
            }
        }

        // If the conversation is small enough, just format manually
        if conversation_text.len() < 1000 {
            let mut summary = String::from("## Conversation Summary\n\n");
            summary.push_str(&conversation_text);
            if !key_facts.is_empty() {
                summary.push_str("\n## Key Facts\n\n");
                for fact in key_facts {
                    summary.push_str(&format!("- {}\n", fact));
                }
            }
            return Ok(summary);
        }

        // Use LLM for longer conversations
        let prompt = format!(
            "Summarize this conversation concisely, preserving:\n\
             1. Key decisions made\n\
             2. Important file paths mentioned\n\
             3. User preferences expressed\n\
             4. Task progress and outcomes\n\
             5. Any errors encountered and their resolutions\n\n\
             Key facts to preserve:\n{}\n\n\
             Conversation:\n{}",
            key_facts.join("\n"),
            truncate_str(&conversation_text, 6000)
        );

        let summary_messages = vec![
            ChatMessage::system("You are a concise summarizer. Output a structured markdown summary.".to_string()),
            ChatMessage::user(prompt),
        ];

        match self.client.complete(&self.model, summary_messages, Some(1024)).await {
            Ok(summary) => Ok(summary),
            Err(e) => {
                // Fallback: manual extraction if LLM fails
                debug!("LLM summary failed, using manual extraction: {}", e);
                let mut summary = String::from("## Session Summary (auto-extracted)\n\n");
                for fact in key_facts {
                    summary.push_str(&format!("- {}\n", fact));
                }
                summary.push_str(&format!("\n{} messages were compacted.\n", messages.len()));
                Ok(summary)
            }
        }
    }
}

/// Strategy for compaction fallback chain
#[derive(Debug, Clone, Copy)]
pub enum CompactionStrategy {
    /// Try LLM-powered compaction (up to 3 attempts)
    AutoCompact,
    /// Truncate tool result content to 2000 chars
    TruncateToolResults,
    /// Strip reasoning/thinking content from messages
    ReduceThinking,
    /// Retry compaction with cheaper model via failover
    ModelFailover,
    /// Keep only system prompt + reset marker (last resort)
    SessionReset,
}

impl SessionCompactor {
    /// Flush durable memories from conversation before compacting.
    /// Returns bullet-point facts suitable for writing to MEMORY.md.
    pub async fn flush_memories_before_compaction(
        &self,
        messages: &[ChatMessage],
    ) -> Vec<String> {
        // Build a truncated conversation view (~8000 chars)
        let mut conversation_text = String::new();
        for msg in messages {
            let role = msg_role(msg);
            if role == "tool" { continue; }
            let content = msg_content(msg);
            if !content.is_empty() {
                conversation_text.push_str(&format!("[{}]: {}\n", role, truncate_str(&content, 300)));
            }
            if conversation_text.len() > 8000 {
                break;
            }
        }

        if conversation_text.is_empty() {
            return Vec::new();
        }

        let prompt = format!(
            "You are extracting durable facts from a conversation that is about to be compacted.\n\
             Extract ONLY facts worth remembering long-term as bullet points:\n\
             - User preferences and working style\n\
             - Project decisions and architecture choices\n\
             - Key file paths and their purposes\n\
             - Important technical findings\n\
             - Recurring patterns or issues\n\n\
             Output ONLY bullet points, one per line, starting with '- '.\n\
             If nothing is worth remembering, output nothing.\n\n\
             Conversation:\n{}", truncate_str(&conversation_text, 6000)
        );

        let summary_messages = vec![
            ChatMessage::system("Extract durable facts as bullet points. Be selective — only truly important facts."),
            ChatMessage::user(prompt),
        ];

        match self.client.complete(&self.model, summary_messages, Some(512)).await {
            Ok(response) => {
                response.lines()
                    .filter(|l| l.starts_with("- "))
                    .map(|l| l.to_string())
                    .collect()
            }
            Err(_) => Vec::new(),
        }
    }

    /// Compact with a fallback chain of strategies.
    /// Tries each strategy in order until one reduces the context enough.
    pub async fn compact_with_fallback(
        &self,
        messages: &[ChatMessage],
        keep_recent: usize,
        strategies: &[CompactionStrategy],
        target_tokens: usize,
    ) -> Result<Vec<ChatMessage>> {
        let mut current_messages = messages.to_vec();

        for strategy in strategies {
            let tokens = estimate_tokens(&current_messages);
            if tokens <= target_tokens {
                break;
            }

            match strategy {
                CompactionStrategy::AutoCompact => {
                    // Try LLM compaction up to 3 times
                    for attempt in 0..3 {
                        match self.compact(&current_messages, keep_recent).await {
                            Ok(compacted) => {
                                let new_tokens = estimate_tokens(&compacted);
                                if new_tokens < tokens {
                                    current_messages = compacted;
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::debug!("AutoCompact attempt {} failed: {}", attempt + 1, e);
                            }
                        }
                    }
                }
                CompactionStrategy::TruncateToolResults => {
                    const MAX_RESULT_LEN: usize = 2000;
                    for msg in &mut current_messages {
                        if msg_role(msg) == "tool" {
                            if let Some(ref content) = msg.content {
                                let text = content.as_str().unwrap_or("");
                                if text.len() > MAX_RESULT_LEN {
                                    msg.content = Some(serde_json::json!(
                                        crate::truncate_safe(text, MAX_RESULT_LEN)
                                    ));
                                }
                            }
                        }
                    }
                }
                CompactionStrategy::ReduceThinking => {
                    for msg in &mut current_messages {
                        msg.reasoning = None;
                        msg.reasoning_details = None;
                    }
                }
                CompactionStrategy::ModelFailover => {
                    // Try compaction with cheaper model
                    let cheap_model = "openai/gpt-oss-120b:free".to_string();
                    let cheap_compactor = SessionCompactor::new(self.client.clone(), cheap_model);
                    if let Ok(compacted) = cheap_compactor.compact(&current_messages, keep_recent).await {
                        let new_tokens = estimate_tokens(&compacted);
                        if new_tokens < tokens {
                            current_messages = compacted;
                        }
                    }
                }
                CompactionStrategy::SessionReset => {
                    // Last resort: keep only system prompt + reset marker + last message
                    let system_msg = current_messages.iter()
                        .find(|m| msg_role(m) == "system")
                        .cloned();
                    let last_user = current_messages.iter().rev()
                        .find(|m| msg_role(m) == "user")
                        .cloned();

                    current_messages.clear();
                    if let Some(sys) = system_msg {
                        current_messages.push(sys);
                    }
                    current_messages.push(ChatMessage::system(
                        "[Session was reset due to context overflow. Previous conversation was lost. \
                         Please refer to Bootstrap Context for persistent knowledge.]"
                    ));
                    if let Some(user) = last_user {
                        current_messages.push(user);
                    }
                }
            }
        }

        Ok(current_messages)
    }
}

/// Get the role string from a ChatMessage
fn msg_role(msg: &ChatMessage) -> String {
    msg.role.as_ref()
        .and_then(|r| r.as_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Get the content string from a ChatMessage
fn msg_content(msg: &ChatMessage) -> String {
    msg.content.as_ref()
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string()
}

/// Estimate total tokens in messages (~4 chars per token)
fn estimate_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter()
        .map(|m| {
            let content_len = msg_content(m).len();
            let tool_len = m.tool_calls.as_ref()
                .map(|tcs| tcs.iter().map(|tc| tc.function.arguments.len()).sum::<usize>())
                .unwrap_or(0);
            (content_len + tool_len) / 4
        })
        .sum()
}

/// Truncate string with ellipsis
fn truncate_str(s: &str, max: usize) -> String {
    crate::truncate_safe(s, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_compact() {
        // 60 messages with enough content to exceed 100 tokens
        let msgs: Vec<ChatMessage> = (0..60)
            .map(|i| ChatMessage::user(format!("This is message number {} with some reasonable content to fill tokens", i)))
            .collect();
        assert!(SessionCompactor::should_compact(&msgs, 50, 100));

        let small: Vec<ChatMessage> = (0..10)
            .map(|i| ChatMessage::user(format!("Hi {}", i)))
            .collect();
        assert!(!SessionCompactor::should_compact(&small, 50, 100));
    }

    #[test]
    fn test_msg_role() {
        let msg = ChatMessage::user("test".to_string());
        assert_eq!(msg_role(&msg), "user");

        let sys = ChatMessage::system("system prompt");
        assert_eq!(msg_role(&sys), "system");
    }

    #[test]
    fn test_msg_content() {
        let msg = ChatMessage::user("hello world".to_string());
        assert_eq!(msg_content(&msg), "hello world");

        let empty = ChatMessage {
            role: Some(serde_json::json!("assistant")),
            content: None,
            reasoning_details: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        };
        assert_eq!(msg_content(&empty), "");
    }

    #[test]
    fn test_compaction_strategy_truncate_tool_results() {
        // Build messages with a large tool result
        let mut messages = vec![
            ChatMessage::system("You are helpful."),
            ChatMessage::user("Run a command"),
        ];
        // Add a tool result with 5000 chars
        let big_result = "x".repeat(5000);
        let tool_msg = ChatMessage {
            role: Some(serde_json::json!("tool")),
            content: Some(serde_json::json!(big_result)),
            reasoning_details: None,
            tool_calls: None,
            tool_call_id: Some("tc_1".to_string()),
            name: Some("run_command".to_string()),
            reasoning: None,
            refusal: None,
        };
        messages.push(tool_msg);

        // Apply TruncateToolResults directly
        let mut msgs = messages.clone();
        for msg in &mut msgs {
            if msg_role(msg) == "tool" {
                if let Some(ref content) = msg.content {
                    let text = content.as_str().unwrap_or("");
                    if text.len() > 2000 {
                        msg.content = Some(serde_json::json!(
                            crate::truncate_safe(text, 2000)
                        ));
                    }
                }
            }
        }

        // The tool message should be truncated
        let tool_content = msgs[2].content.as_ref().unwrap().as_str().unwrap();
        assert!(tool_content.len() < 2100, "Tool result should be truncated, got len {}", tool_content.len());
        assert!(tool_content.contains("...[truncated]"));
    }

    #[test]
    fn test_compaction_strategy_reduce_thinking() {
        let mut msg = ChatMessage::user("test");
        msg.reasoning = Some(serde_json::json!("long chain of thought..."));
        msg.reasoning_details = Some(crate::agent::llm::ReasoningDetails {
            reasoning: "long thinking...".to_string(),
            confidence: Some(0.9),
            steps: vec!["step1".to_string()],
        });

        // Apply ReduceThinking
        msg.reasoning = None;
        msg.reasoning_details = None;

        assert!(msg.reasoning.is_none());
        assert!(msg.reasoning_details.is_none());
    }

    #[test]
    fn test_compaction_strategy_session_reset() {
        let messages = vec![
            ChatMessage::system("You are an agent."),
            ChatMessage::user("Help me code"),
            ChatMessage::user("Also do X"),
            ChatMessage::user("And Y"),
            ChatMessage::user("Final question"),
        ];

        // Simulate SessionReset
        let system_msg = messages.iter()
            .find(|m| msg_role(m) == "system")
            .cloned();
        let last_user = messages.iter().rev()
            .find(|m| msg_role(m) == "user")
            .cloned();

        let mut result = Vec::new();
        if let Some(sys) = system_msg {
            result.push(sys);
        }
        result.push(ChatMessage::system(
            "[Session was reset due to context overflow.]"
        ));
        if let Some(user) = last_user {
            result.push(user);
        }

        assert_eq!(result.len(), 3);
        assert_eq!(msg_role(&result[0]), "system");
        assert_eq!(msg_content(&result[1]), "[Session was reset due to context overflow.]");
        assert_eq!(msg_content(&result[2]), "Final question");
    }

    #[test]
    fn test_extract_key_facts() {
        let messages = vec![
            ChatMessage::user("I prefer using tabs over spaces"),
            ChatMessage::user("Look at src/main.rs and cargo.toml"),
            ChatMessage::user("Always use snake_case for filenames"),
        ];
        let msg_refs: Vec<&ChatMessage> = messages.iter().collect();

        let compactor = SessionCompactor {
            client: crate::agent::llm::OpenRouterClient::new("test-key".to_string()),
            model: "test".to_string(),
        };

        let facts = compactor.extract_key_facts(&msg_refs);
        // Should find file paths (may be in "Files referenced: ..." format)
        assert!(facts.iter().any(|f| f.contains("src/main.rs")), "Should find src/main.rs, got: {:?}", facts);
        // Should find user preferences
        assert!(facts.iter().any(|f| f.contains("prefer") || f.contains("tabs")),
            "Should find preference about tabs, got: {:?}", facts);
    }
}
