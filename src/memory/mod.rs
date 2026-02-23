//! Persistent Memory Module for my-agent
//!
//! Provides:
//! - SQLite-based conversation persistence
//! - Full-text search (FTS5) for message content
//! - Vector embeddings for semantic search (OpenAI API or local fallback)
//! - Message-level embeddings for fine-grained search
//! - Cross-session memory retrieval
//! - Recursive context management for long inputs

pub mod sqlite;
pub mod embeddings;
pub mod retrieval;
pub mod recursive;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

pub use sqlite::{SqliteMemoryStore, MemoryStats, MessageSearchResult, MessageSemanticResult};
pub use embeddings::{EmbeddingModel, EmbeddingConfig, cosine_similarity};
pub use retrieval::SemanticSearch;
pub use recursive::{RecursiveContextManager, RecursiveConfig, RecursiveResult, SummaryNode};

/// A stored conversation record with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRecord {
    /// Unique conversation ID
    pub id: String,
    /// Conversation title (auto-generated from first user message)
    pub title: Option<String>,
    /// All messages in the conversation
    pub messages: Vec<crate::types::Message>,
    /// Optional summary for context compression
    pub summary: Option<String>,
    /// Vector embedding of the conversation (for semantic search)
    pub embedding: Option<Vec<f32>>,
    /// When the conversation was created
    pub created_at: DateTime<Utc>,
    /// When the conversation was last updated
    pub updated_at: DateTime<Utc>,
    /// Tags for categorization
    pub tags: Vec<String>,
}

/// A memory entry in the knowledge base
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    /// Unique entry ID
    pub id: String,
    /// The content to remember
    pub content: String,
    /// Vector embedding of the content
    pub embedding: Option<Vec<f32>>,
    /// Source of this knowledge (e.g., "conversation", "document", "user_input")
    pub source: String,
    /// Importance score (0.0 to 1.0, higher = more important)
    pub importance: f32,
    /// Access count (for determining frequently accessed memories)
    pub access_count: u32,
    /// When this entry was created
    pub created_at: DateTime<Utc>,
    /// When this entry was last accessed
    pub last_accessed: DateTime<Utc>,
}

/// Memory store configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Path to the SQLite database file
    pub database_path: PathBuf,
    /// Maximum number of messages to keep in context before compression
    pub max_context_messages: usize,
    /// Enable semantic search with embeddings
    pub enable_embeddings: bool,
    /// Embedding model configuration
    pub embedding_config: EmbeddingConfig,
    /// Days to keep conversations before archival
    pub retention_days: u32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        let data_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("my-agent");

        Self {
            database_path: data_dir.join("memory.db"),
            max_context_messages: 50,
            enable_embeddings: true,
            embedding_config: EmbeddingConfig::default(),
            retention_days: 365,
        }
    }
}

/// Main memory store combining SQLite and embeddings
#[derive(Clone)]
pub struct MemoryStore {
    /// SQLite backend for persistence
    sqlite: Arc<SqliteMemoryStore>,
    /// Optional embedding model for semantic search
    embedding_model: Option<Arc<EmbeddingModel>>,
    /// Configuration
    config: MemoryConfig,
}

impl MemoryStore {
    /// Create a new memory store with the given configuration
    pub async fn new(config: MemoryConfig) -> Result<Self> {
        // Ensure the database directory exists
        if let Some(parent) = config.database_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Initialize SQLite store
        let sqlite = Arc::new(SqliteMemoryStore::new(&config.database_path).await?);

        // Initialize embedding model if enabled
        // Use with_keyring_key() to automatically get API key from keyring
        let embedding_model = if config.enable_embeddings {
            match EmbeddingModel::with_keyring_key().await {
                Ok(model) => Some(Arc::new(model)),
                Err(e) => {
                    tracing::warn!("Failed to initialize embedding model: {}. Semantic search disabled.", e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            sqlite,
            embedding_model,
            config,
        })
    }

    /// Create with default configuration
    pub async fn default_store() -> Result<Self> {
        Self::new(MemoryConfig::default()).await
    }

    /// Save a conversation to persistent storage
    ///
    /// Takes a ConversationRecord directly to avoid circular dependencies
    pub async fn save_conversation(&self, record: &ConversationRecord) -> Result<()> {
        // Generate embedding for the conversation if model is available
        let enriched_record = if record.embedding.is_none() {
            if let Some(ref model) = self.embedding_model {
                let text = record.messages.iter()
                    .map(|m| m.content.clone())
                    .collect::<Vec<_>>()
                    .join(" ");
                let embedding = model.embed(&text).await.ok();

                let mut enriched = record.clone();
                enriched.embedding = embedding;
                enriched
            } else {
                record.clone()
            }
        } else {
            record.clone()
        };

        self.sqlite.save_conversation(&enriched_record).await
    }

    /// Load a conversation by ID
    pub async fn load_conversation(&self, id: &str) -> Result<Option<ConversationRecord>> {
        self.sqlite.load_conversation(id).await
    }

    /// List all conversations (paginated)
    pub async fn list_conversations(&self, limit: usize, offset: usize) -> Result<Vec<ConversationRecord>> {
        self.sqlite.list_conversations(limit, offset).await
    }

    /// Search conversations using full-text search
    pub async fn search_conversations(&self, query: &str, limit: usize) -> Result<Vec<ConversationRecord>> {
        self.sqlite.search_conversations(query, limit).await
    }

    /// Semantic search using embeddings
    pub async fn semantic_search(&self, query: &str, limit: usize) -> Result<Vec<(ConversationRecord, f32)>> {
        let model = self.embedding_model.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Embedding model not initialized"))?;

        let query_embedding = model.embed(query).await?;
        self.sqlite.semantic_search(&query_embedding, limit).await
    }

    /// Delete a conversation
    pub async fn delete_conversation(&self, id: &str) -> Result<()> {
        self.sqlite.delete_conversation(id).await
    }

    /// Get recent conversations for context
    pub async fn get_recent_context(&self, limit: usize) -> Result<Vec<ConversationRecord>> {
        self.sqlite.list_conversations(limit, 0).await
    }

    /// Add a knowledge entry
    pub async fn add_knowledge(&self, content: &str, source: &str, importance: f32) -> Result<String> {
        let embedding = if let Some(ref model) = self.embedding_model {
            model.embed(content).await.ok()
        } else {
            None
        };

        let entry = KnowledgeEntry {
            id: uuid::Uuid::new_v4().to_string(),
            content: content.to_string(),
            embedding,
            source: source.to_string(),
            importance,
            access_count: 0,
            created_at: Utc::now(),
            last_accessed: Utc::now(),
        };

        self.sqlite.save_knowledge(&entry).await?;
        Ok(entry.id)
    }

    /// Search knowledge base
    pub async fn search_knowledge(&self, query: &str, limit: usize) -> Result<Vec<(KnowledgeEntry, f32)>> {
        let model = self.embedding_model.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Embedding model not initialized"))?;

        let query_embedding = model.embed(query).await?;
        self.sqlite.search_knowledge(&query_embedding, limit).await
    }

    /// Get the SQLite store for direct access
    pub fn sqlite(&self) -> Arc<SqliteMemoryStore> {
        self.sqlite.clone()
    }

    /// Check if embeddings are available
    pub fn has_embeddings(&self) -> bool {
        self.embedding_model.is_some()
    }

    /// Get the embedding model
    pub fn embedding_model(&self) -> Option<Arc<EmbeddingModel>> {
        self.embedding_model.clone()
    }

    /// Get memory statistics
    pub async fn stats(&self) -> Result<MemoryStats> {
        self.sqlite.stats().await
    }
}