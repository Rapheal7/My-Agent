//! Semantic search and retrieval functionality

use anyhow::Result;
use std::collections::HashMap;

use super::{ConversationRecord, KnowledgeEntry, MemoryStore};

/// Semantic search engine for memory retrieval
pub struct SemanticSearch {
    memory_store: std::sync::Arc<MemoryStore>,
}

impl SemanticSearch {
    /// Create a new semantic search instance
    pub fn new(memory_store: std::sync::Arc<MemoryStore>) -> Self {
        Self { memory_store }
    }

    /// Search for conversations semantically similar to the query
    ///
    /// Returns up to `limit` results with their similarity scores
    pub async fn search_conversations(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let results = self.memory_store.semantic_search(query, limit).await?;

        Ok(results.into_iter()
            .map(|(record, score)| SearchResult {
                id: record.id,
                title: record.title,
                summary: record.summary,
                score,
                record_type: RecordType::Conversation,
                created_at: record.created_at,
            })
            .collect())
    }

    /// Search for knowledge entries semantically similar to the query
    pub async fn search_knowledge(&self, query: &str, limit: usize) -> Result<Vec<KnowledgeSearchResult>> {
        let results = self.memory_store.search_knowledge(query, limit).await?;

        Ok(results.into_iter()
            .map(|(entry, score)| KnowledgeSearchResult {
                id: entry.id,
                content: entry.content,
                source: entry.source,
                importance: entry.importance,
                score,
            })
            .collect())
    }

    /// Hybrid search combining keyword and semantic search
    ///
    /// Uses reciprocal rank fusion to combine results
    pub async fn hybrid_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Get results from both search methods
        let fts_results = self.memory_store.search_conversations(query, limit).await?;
        let semantic_results = self.memory_store.semantic_search(query, limit).await?;

        // Combine using reciprocal rank fusion
        let mut scores: HashMap<String, f32> = HashMap::new();
        let mut records: HashMap<String, ConversationRecord> = HashMap::new();

        // FTS results (lower weight as they're keyword-based)
        const FTS_WEIGHT: f32 = 0.4;
        for (rank, record) in fts_results.into_iter().enumerate() {
            let rrf_score = FTS_WEIGHT / (60.0 + rank as f32);
            *scores.entry(record.id.clone()).or_default() += rrf_score;
            records.insert(record.id.clone(), record);
        }

        // Semantic results (higher weight for semantic understanding)
        const SEMANTIC_WEIGHT: f32 = 0.6;
        for (record, similarity) in semantic_results.into_iter() {
            let rrf_score = SEMANTIC_WEIGHT * similarity;
            *scores.entry(record.id.clone()).or_default() += rrf_score;
            records.insert(record.id.clone(), record);
        }

        // Sort by combined score
        let mut combined: Vec<_> = scores.into_iter()
            .map(|(id, score)| {
                let record = records.remove(&id).unwrap();
                (record, score)
            })
            .collect();

        combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        combined.truncate(limit);

        Ok(combined.into_iter()
            .map(|(record, score)| SearchResult {
                id: record.id,
                title: record.title,
                summary: record.summary,
                score,
                record_type: RecordType::Conversation,
                created_at: record.created_at,
            })
            .collect())
    }

    /// Get context for the current conversation
    ///
    /// Retrieves relevant past conversations and knowledge
    pub async fn get_context(&self, current_query: &str, max_messages: usize) -> Result<ContextResult> {
        // Get relevant conversations
        let conversations = self.hybrid_search(current_query, 5).await?;

        // Get relevant knowledge
        let knowledge = self.search_knowledge(current_query, 5).await?;

        // Build context string
        let mut context_parts = Vec::new();

        if !conversations.is_empty() {
            context_parts.push("Relevant past conversations:".to_string());
            for conv in conversations.iter().take(3) {
                if let Some(ref title) = conv.title {
                    context_parts.push(format!("- {} (relevance: {:.2})", title, conv.score));
                }
            }
        }

        if !knowledge.is_empty() {
            context_parts.push("\nRelevant knowledge:".to_string());
            for entry in knowledge.iter().take(3) {
                context_parts.push(format!("- {} (relevance: {:.2})", entry.content, entry.score));
            }
        }

        Ok(ContextResult {
            context_text: context_parts.join("\n"),
            conversations,
            knowledge,
            max_messages,
        })
    }
}

/// A search result with relevance score
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Unique ID of the record
    pub id: String,
    /// Title (for conversations)
    pub title: Option<String>,
    /// Summary (if available)
    pub summary: Option<String>,
    /// Relevance score (0.0 to 1.0, higher is more relevant)
    pub score: f32,
    /// Type of record
    pub record_type: RecordType,
    /// When the record was created
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Knowledge search result
#[derive(Debug, Clone)]
pub struct KnowledgeSearchResult {
    /// Unique ID
    pub id: String,
    /// The knowledge content
    pub content: String,
    /// Source of the knowledge
    pub source: String,
    /// Importance score
    pub importance: f32,
    /// Relevance score
    pub score: f32,
}

/// Type of memory record
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecordType {
    Conversation,
    Knowledge,
    Task,
}

/// Result of context retrieval
#[derive(Debug, Clone)]
pub struct ContextResult {
    /// Human-readable context text
    pub context_text: String,
    /// Relevant conversations
    pub conversations: Vec<SearchResult>,
    /// Relevant knowledge entries
    pub knowledge: Vec<KnowledgeSearchResult>,
    /// Maximum messages to include
    pub max_messages: usize,
}

impl ContextResult {
    /// Format the context for inclusion in an LLM prompt
    pub fn to_prompt_context(&self) -> String {
        if self.context_text.is_empty() {
            String::new()
        } else {
            format!(
                "Here is some relevant context from memory:\n{}\n\nUse this context to help answer the user's question.",
                self.context_text
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_result_scoring() {
        let result = SearchResult {
            id: "test".to_string(),
            title: Some("Test".to_string()),
            summary: None,
            score: 0.85,
            record_type: RecordType::Conversation,
            created_at: chrono::Utc::now(),
        };

        assert!(result.score > 0.0);
        assert!(result.score <= 1.0);
    }
}