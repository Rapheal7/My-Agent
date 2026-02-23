//! Recursive Context Manager
//!
//! Implements MIT's Recursive Language Model approach for handling inputs
//! beyond standard context window limits through hierarchical chunking and
//! recursive summarization.
//!
//! Based on: https://arxiv.org/abs/2512.24601

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, debug, warn};

use crate::agent::llm::{OpenRouterClient, ChatMessage};

/// Default chunk size for processing (in tokens, approximated by chars/4)
const DEFAULT_CHUNK_SIZE: usize = 4000; // ~16K chars

/// Maximum recursion depth for hierarchical summarization
const MAX_RECURSION_DEPTH: usize = 5;

/// Minimum content size to trigger recursive processing (in tokens)
const RECURSION_THRESHOLD: usize = 6000; // ~24K chars

/// Recursive Context Manager
pub struct RecursiveContextManager {
    client: OpenRouterClient,
    model: String,
    chunk_size: usize,
    max_depth: usize,
    /// Cache of processed summaries
    summary_cache: Arc<RwLock<Vec<SummaryNode>>>,
}

/// Node in the summary hierarchy tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryNode {
    /// Unique identifier for this node
    pub id: String,
    /// Original chunk index (for leaf nodes)
    pub chunk_index: Option<usize>,
    /// Summary content
    pub summary: String,
    /// Token count of the summary
    pub token_count: usize,
    /// Depth in the hierarchy tree
    pub depth: usize,
    /// Child nodes (for non-leaf nodes)
    pub children: Vec<String>,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Result of recursive processing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecursiveResult {
    /// Final composed summary
    pub final_summary: String,
    /// Total chunks processed
    pub total_chunks: usize,
    /// Recursion depth reached
    pub depth_reached: usize,
    /// Original token count (estimated)
    pub original_tokens: usize,
    /// Final token count
    pub final_tokens: usize,
    /// Compression ratio
    pub compression_ratio: f64,
    /// Summary hierarchy
    pub hierarchy: Vec<SummaryNode>,
}

/// Configuration for recursive processing
#[derive(Debug, Clone)]
pub struct RecursiveConfig {
    /// Size of each chunk (in approximate tokens)
    pub chunk_size: usize,
    /// Maximum recursion depth
    pub max_depth: usize,
    /// Threshold to trigger recursive processing (in tokens)
    pub recursion_threshold: usize,
    /// Model to use for summarization
    pub model: String,
    /// Whether to preserve full context in cache
    pub cache_full_context: bool,
}

impl Default for RecursiveConfig {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            max_depth: MAX_RECURSION_DEPTH,
            recursion_threshold: RECURSION_THRESHOLD,
            model: "z-ai/glm-5".to_string(),
            cache_full_context: true,
        }
    }
}

impl RecursiveContextManager {
    /// Create a new recursive context manager
    pub fn new(client: OpenRouterClient) -> Self {
        Self {
            client,
            model: "z-ai/glm-5".to_string(),
            chunk_size: DEFAULT_CHUNK_SIZE,
            max_depth: MAX_RECURSION_DEPTH,
            summary_cache: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Create with custom configuration
    pub fn with_config(client: OpenRouterClient, config: RecursiveConfig) -> Self {
        Self {
            client,
            model: config.model,
            chunk_size: config.chunk_size,
            max_depth: config.max_depth,
            summary_cache: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Estimate token count (roughly 4 chars per token)
    fn estimate_tokens(text: &str) -> usize {
        text.len() / 4
    }

    /// Check if content needs recursive processing
    pub fn needs_recursion(&self, content: &str) -> bool {
        Self::estimate_tokens(content) > RECURSION_THRESHOLD
    }

    /// Split content into chunks
    fn chunk_content(&self, content: &str) -> Vec<String> {
        let chars: Vec<char> = content.chars().collect();
        let chunk_chars = self.chunk_size * 4; // Convert tokens to chars

        if chars.len() <= chunk_chars {
            return vec![content.to_string()];
        }

        let mut chunks = Vec::new();
        let mut start = 0;

        while start < chars.len() {
            let end = std::cmp::min(start + chunk_chars, chars.len());

            // Try to find a good break point (sentence or paragraph)
            let mut break_point = end;
            if end < chars.len() {
                // Look for paragraph break first
                for i in (start..end).rev() {
                    if chars[i] == '\n' && i > 0 && chars[i - 1] == '\n' {
                        break_point = i;
                        break;
                    }
                }
                // If no paragraph break, look for sentence end
                if break_point == end {
                    for i in (start..end).rev() {
                        if chars[i] == '.' || chars[i] == '!' || chars[i] == '?' {
                            break_point = i + 1;
                            break;
                        }
                    }
                }
            }

            let chunk: String = chars[start..break_point].iter().collect();
            if !chunk.trim().is_empty() {
                chunks.push(chunk);
            }
            start = break_point;
        }

        chunks
    }

    /// Summarize a single chunk
    async fn summarize_chunk(&self, chunk: &str, context: &str, chunk_index: usize, total_chunks: usize) -> Result<String> {
        let system_prompt = format!(
            "You are a precise summarizer. Your task is to create a concise but complete summary of the provided text chunk.\n\
            This is chunk {} of {} from a longer document.\n\
            Context: {}\n\n\
            Guidelines:\n\
            - Preserve all key information, facts, and relationships\n\
            - Maintain chronological or logical order\n\
            - Note any unresolved references or dependencies on other chunks\n\
            - Keep the summary under 500 tokens",
            chunk_index + 1, total_chunks, context
        );

        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(format!("Summarize this text chunk:\n\n{}", chunk)),
        ];

        let response = self.client.complete(&self.model, messages, Some(1024)).await?;
        Ok(response)
    }

    /// Compose multiple summaries into a higher-level summary
    async fn compose_summaries(&self, summaries: &[String], level: usize) -> Result<String> {
        let combined = summaries.join("\n\n---\n\n");

        let system_prompt = format!(
            "You are composing multiple summaries into a coherent higher-level summary.\n\
            This is composition level {} in a hierarchical summarization process.\n\n\
            Guidelines:\n\
            - Combine related information across summaries\n\
            - Resolve any cross-references between sections\n\
            - Maintain a logical flow and structure\n\
            - Preserve all essential information\n\
            - The result should be a unified, coherent summary\n\
            - Keep under 1000 tokens",
            level
        );

        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(format!("Compose these summaries into a coherent summary:\n\n{}", combined)),
        ];

        let response = self.client.complete(&self.model, messages, Some(2048)).await?;
        Ok(response)
    }

    /// Process content recursively
    pub async fn process(&self, content: &str) -> Result<RecursiveResult> {
        let original_tokens = Self::estimate_tokens(content);

        info!("Processing content with {} estimated tokens", original_tokens);

        // Check if we need recursion
        if original_tokens <= RECURSION_THRESHOLD {
            debug!("Content under threshold, no recursion needed");
            return Ok(RecursiveResult {
                final_summary: content.to_string(),
                total_chunks: 1,
                depth_reached: 0,
                original_tokens,
                final_tokens: original_tokens,
                compression_ratio: 1.0,
                hierarchy: vec![],
            });
        }

        // Chunk the content
        let chunks = self.chunk_content(content);
        info!("Split into {} chunks", chunks.len());

        // Process each chunk
        let mut current_summaries: Vec<String> = Vec::new();
        let mut all_nodes: Vec<SummaryNode> = Vec::new();
        let mut depth = 0;

        // Level 0: Summarize each chunk
        for (i, chunk) in chunks.iter().enumerate() {
            let summary = self.summarize_chunk(chunk, "", i, chunks.len()).await?;
            let node = SummaryNode {
                id: format!("chunk-{}", i),
                chunk_index: Some(i),
                summary: summary.clone(),
                token_count: Self::estimate_tokens(&summary),
                depth: 0,
                children: vec![],
                created_at: chrono::Utc::now(),
            };
            all_nodes.push(node);
            current_summaries.push(summary);
        }

        // Recursive composition
        while current_summaries.len() > 1 && depth < self.max_depth {
            depth += 1;
            debug!("Composing at depth {}, {} summaries", depth, current_summaries.len());

            // Group summaries for composition (max 5 at a time)
            let mut next_level: Vec<String> = Vec::new();
            for group in current_summaries.chunks(5) {
                let composed = self.compose_summaries(group, depth).await?;
                let node = SummaryNode {
                    id: format!("compose-{}-{}", depth, next_level.len()),
                    chunk_index: None,
                    summary: composed.clone(),
                    token_count: Self::estimate_tokens(&composed),
                    depth,
                    children: (0..group.len()).map(|i| format!("level-{}-{}", depth - 1, i)).collect(),
                    created_at: chrono::Utc::now(),
                };
                all_nodes.push(node);
                next_level.push(composed);
            }

            current_summaries = next_level;
        }

        // Final summary
        let final_summary = if current_summaries.len() == 1 {
            current_summaries.remove(0)
        } else {
            self.compose_summaries(&current_summaries, depth + 1).await?
        };

        let final_tokens = Self::estimate_tokens(&final_summary);
        let compression_ratio = original_tokens as f64 / final_tokens as f64;

        info!("Recursive processing complete: {} -> {} tokens ({}x compression)",
              original_tokens, final_tokens, compression_ratio);

        // Cache the hierarchy
        let mut cache = self.summary_cache.write().await;
        cache.clone_from(&all_nodes);

        Ok(RecursiveResult {
            final_summary,
            total_chunks: chunks.len(),
            depth_reached: depth,
            original_tokens,
            final_tokens,
            compression_ratio,
            hierarchy: all_nodes,
        })
    }

    /// Process a conversation history recursively
    pub async fn process_conversation(&self, messages: &[ChatMessage]) -> Result<RecursiveResult> {
        // Convert messages to a single content string
        let content = messages.iter()
            .map(|m| {
                let role = m.role.as_ref()
                    .and_then(|r| r.as_str())
                    .unwrap_or("message");
                let role_name = match role {
                    "system" => "SYSTEM",
                    "user" => "USER",
                    "assistant" => "ASSISTANT",
                    _ => "MESSAGE",
                };
                let content = m.content.as_ref()
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                format!("{}: {}", role_name, content)
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        self.process(&content).await
    }

    /// Get a sliding window summary for incremental context
    pub async fn get_sliding_summary(&self, content: &str, window_size: usize, overlap: usize) -> Result<Vec<String>> {
        let chunks = self.chunk_content(content);
        let mut summaries = Vec::new();

        // Create overlapping windows
        let mut i = 0;
        while i < chunks.len() {
            let end = std::cmp::min(i + window_size, chunks.len());
            let window_content = chunks[i..end].join("\n\n");

            let summary = if Self::estimate_tokens(&window_content) > 2000 {
                // Summarize large windows
                self.summarize_chunk(&window_content, "", i, chunks.len()).await?
            } else {
                window_content
            };

            summaries.push(summary);
            i += window_size - overlap;
        }

        Ok(summaries)
    }

    /// Clear the summary cache
    pub async fn clear_cache(&self) {
        let mut cache = self.summary_cache.write().await;
        cache.clear();
    }

    /// Get cached summaries
    pub async fn get_cached_summaries(&self) -> Vec<SummaryNode> {
        let cache = self.summary_cache.read().await;
        cache.clone()
    }
}

/// Extension trait for ChatMessage to support recursive processing
pub trait RecursiveMessageExt {
    /// Check if message history needs recursive compression
    fn needs_compression(&self, max_tokens: usize) -> bool;

    /// Get estimated token count
    fn token_count(&self) -> usize;
}

impl RecursiveMessageExt for Vec<ChatMessage> {
    fn needs_compression(&self, max_tokens: usize) -> bool {
        self.token_count() > max_tokens
    }

    fn token_count(&self) -> usize {
        self.iter()
            .filter_map(|m| m.content.as_ref())
            .map(|c| match c {
                serde_json::Value::String(s) => RecursiveContextManager::estimate_tokens(s),
                _ => 0,
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        let text = "Hello world this is a test";
        let tokens = RecursiveContextManager::estimate_tokens(text);
        assert!(tokens > 0);
        assert!(tokens < text.len());
    }

    #[test]
    fn test_chunk_content_small() {
        // This would need a client to test properly
        // Just test the logic
        let text = "This is a short text that doesn't need chunking.";
        let tokens = RecursiveContextManager::estimate_tokens(text);
        assert!(tokens < RECURSION_THRESHOLD);
    }

    #[test]
    fn test_recursive_config_default() {
        let config = RecursiveConfig::default();
        assert_eq!(config.chunk_size, DEFAULT_CHUNK_SIZE);
        assert_eq!(config.max_depth, MAX_RECURSION_DEPTH);
        assert_eq!(config.recursion_threshold, RECURSION_THRESHOLD);
    }
}