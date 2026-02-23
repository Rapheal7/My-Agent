//! SQLite-based persistent storage for conversations and knowledge

use anyhow::Result;
use rusqlite::{Connection, params, OptionalExtension};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use chrono::{DateTime, Utc};
use serde_json;

use super::{ConversationRecord, KnowledgeEntry};
use super::embeddings::cosine_similarity;

/// SQLite-based memory store
pub struct SqliteMemoryStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteMemoryStore {
    /// Create a new SQLite memory store at the given path
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Open connection
        let conn = Connection::open(&path)?;

        // Enable WAL mode for better performance
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

        // Initialize database schema
        Self::init_schema(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Initialize the database schema
    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(r#"
            -- Main conversations table
            CREATE TABLE IF NOT EXISTS conversations (
                id TEXT PRIMARY KEY,
                title TEXT,
                messages TEXT NOT NULL,
                summary TEXT,
                embedding BLOB,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                tags TEXT DEFAULT '[]'
            );

            -- Individual message embeddings for fine-grained search
            CREATE TABLE IF NOT EXISTS message_embeddings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT NOT NULL,
                message_idx INTEGER NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                embedding BLOB,
                created_at TEXT NOT NULL,
                FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
            );

            -- FTS5 virtual table for full-text search
            CREATE VIRTUAL TABLE IF NOT EXISTS conversations_fts USING fts5(
                id,
                title,
                content,
                tokenize = 'porter unicode61'
            );

            -- FTS5 for messages (more granular search)
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                id,
                conversation_id,
                role,
                content,
                tokenize = 'porter unicode61'
            );

            -- Knowledge base table
            CREATE TABLE IF NOT EXISTS knowledge (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                embedding BLOB,
                source TEXT NOT NULL,
                importance REAL DEFAULT 0.5,
                access_count INTEGER DEFAULT 0,
                created_at TEXT NOT NULL,
                last_accessed TEXT NOT NULL
            );

            -- FTS5 for knowledge base
            CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts USING fts5(
                id,
                content,
                tokenize = 'porter unicode61'
            );

            -- Indexes for faster queries
            CREATE INDEX IF NOT EXISTS idx_conversations_created ON conversations(created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_conversations_updated ON conversations(updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_message_embeddings_conv ON message_embeddings(conversation_id);
            CREATE INDEX IF NOT EXISTS idx_knowledge_source ON knowledge(source);
            CREATE INDEX IF NOT EXISTS idx_knowledge_importance ON knowledge(importance DESC);
        "#)?;

        Ok(())
    }

    /// Save a conversation to the database
    pub async fn save_conversation(&self, record: &ConversationRecord) -> Result<()> {
        let conn = self.conn.lock().await;

        // Serialize messages to JSON
        let messages_json = serde_json::to_string(&record.messages)?;
        let tags_json = serde_json::to_string(&record.tags)?;
        let embedding_blob = record.embedding.as_ref()
            .map(|e| Self::embedding_to_blob(e));

        // Extract content for FTS
        let content = record.messages.iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
            .join(" ");

        // Insert or replace conversation
        conn.execute(
            r#"INSERT OR REPLACE INTO conversations
               (id, title, messages, summary, embedding, created_at, updated_at, tags)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
            params![
                record.id,
                record.title,
                messages_json,
                record.summary,
                embedding_blob,
                record.created_at.to_rfc3339(),
                record.updated_at.to_rfc3339(),
                tags_json,
            ]
        )?;

        // Update FTS index (delete then insert since FTS5 doesn't support UPSERT)
        conn.execute(
            "DELETE FROM conversations_fts WHERE id = ?1",
            params![record.id]
        ).ok();

        conn.execute(
            "INSERT INTO conversations_fts (id, title, content) VALUES (?1, ?2, ?3)",
            params![
                record.id,
                record.title.clone().unwrap_or_default(),
                content,
            ]
        )?;

        // Update message-level FTS
        conn.execute(
            "DELETE FROM messages_fts WHERE conversation_id = ?1",
            params![record.id]
        ).ok();

        for (idx, msg) in record.messages.iter().enumerate() {
            let msg_id = format!("{}-{}", record.id, idx);
            conn.execute(
                "INSERT INTO messages_fts (id, conversation_id, role, content) VALUES (?1, ?2, ?3, ?4)",
                params![msg_id, record.id, format!("{:?}", msg.role), msg.content]
            ).ok();
        }

        Ok(())
    }

    /// Save message-level embeddings for fine-grained semantic search
    pub async fn save_message_embedding(
        &self,
        conversation_id: &str,
        message_idx: usize,
        role: &str,
        content: &str,
        embedding: &[f32],
    ) -> Result<()> {
        let conn = self.conn.lock().await;

        conn.execute(
            r#"INSERT OR REPLACE INTO message_embeddings
               (conversation_id, message_idx, role, content, embedding, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            params![
                conversation_id,
                message_idx as i32,
                role,
                content,
                Self::embedding_to_blob(embedding),
                Utc::now().to_rfc3339(),
            ]
        )?;

        Ok(())
    }

    /// Load a conversation by ID
    pub async fn load_conversation(&self, id: &str) -> Result<Option<ConversationRecord>> {
        let conn = self.conn.lock().await;

        let mut stmt = conn.prepare_cached(
            "SELECT id, title, messages, summary, embedding, created_at, updated_at, tags
             FROM conversations WHERE id = ?1"
        )?;

        let result = stmt.query_row(params![id], |row| {
            let id: String = row.get(0)?;
            let title: Option<String> = row.get(1)?;
            let messages_json: String = row.get(2)?;
            let summary: Option<String> = row.get(3)?;
            let embedding_blob: Option<Vec<u8>> = row.get(4)?;
            let created_at_str: String = row.get(5)?;
            let updated_at_str: String = row.get(6)?;
            let tags_json: String = row.get(7)?;

            Ok(ConversationRecord {
                id,
                title,
                messages: serde_json::from_str(&messages_json)
                    .unwrap_or_default(),
                summary,
                embedding: embedding_blob.as_ref()
                    .map(|b| Self::blob_to_embedding(b)),
                created_at: DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                updated_at: DateTime::parse_from_rfc3339(&updated_at_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                tags: serde_json::from_str(&tags_json).unwrap_or_default(),
            })
        }).optional()?;

        Ok(result)
    }

    /// List conversations with pagination
    pub async fn list_conversations(&self, limit: usize, offset: usize) -> Result<Vec<ConversationRecord>> {
        let conn = self.conn.lock().await;

        let mut stmt = conn.prepare_cached(
            "SELECT id, title, messages, summary, embedding, created_at, updated_at, tags
             FROM conversations
             ORDER BY updated_at DESC
             LIMIT ?1 OFFSET ?2"
        )?;

        let records = stmt.query_map(params![limit, offset], |row| {
            let id: String = row.get(0)?;
            let title: Option<String> = row.get(1)?;
            let messages_json: String = row.get(2)?;
            let summary: Option<String> = row.get(3)?;
            let embedding_blob: Option<Vec<u8>> = row.get(4)?;
            let created_at_str: String = row.get(5)?;
            let updated_at_str: String = row.get(6)?;
            let tags_json: String = row.get(7)?;

            Ok(ConversationRecord {
                id,
                title,
                messages: serde_json::from_str(&messages_json)
                    .unwrap_or_default(),
                summary,
                embedding: embedding_blob.as_ref()
                    .map(|b| Self::blob_to_embedding(b)),
                created_at: DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                updated_at: DateTime::parse_from_rfc3339(&updated_at_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                tags: serde_json::from_str(&tags_json).unwrap_or_default(),
            })
        })?.collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Search conversations using full-text search
    pub async fn search_conversations(&self, query: &str, limit: usize) -> Result<Vec<ConversationRecord>> {
        let conn = self.conn.lock().await;

        // Sanitize query for FTS5 - remove special characters
        let sanitize_query = |q: &str| -> String {
            // Remove FTS5 special characters
            q.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == ' ')
                .collect::<String>()
        };

        // Use FTS5 MATCH query with prefix matching
        let fts_query = query.split_whitespace()
            .map(|w| {
                let sanitized = sanitize_query(w);
                if sanitized.is_empty() { String::new() } else { format!("{}*", sanitized) }
            })
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(" OR ");

        if fts_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = conn.prepare_cached(
            r#"SELECT c.id, c.title, c.messages, c.summary, c.embedding, c.created_at, c.updated_at, c.tags
               FROM conversations c
               JOIN conversations_fts fts ON c.id = fts.id
               WHERE conversations_fts MATCH ?1
               ORDER BY bm25(conversations_fts) DESC
               LIMIT ?2"#
        )?;

        let records = stmt.query_map(params![fts_query, limit], |row| {
            let id: String = row.get(0)?;
            let title: Option<String> = row.get(1)?;
            let messages_json: String = row.get(2)?;
            let summary: Option<String> = row.get(3)?;
            let embedding_blob: Option<Vec<u8>> = row.get(4)?;
            let created_at_str: String = row.get(5)?;
            let updated_at_str: String = row.get(6)?;
            let tags_json: String = row.get(7)?;

            Ok(ConversationRecord {
                id,
                title,
                messages: serde_json::from_str(&messages_json)
                    .unwrap_or_default(),
                summary,
                embedding: embedding_blob.as_ref()
                    .map(|b| Self::blob_to_embedding(b)),
                created_at: DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                updated_at: DateTime::parse_from_rfc3339(&updated_at_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                tags: serde_json::from_str(&tags_json).unwrap_or_default(),
            })
        })?.collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Search messages (more granular than conversation-level search)
    pub async fn search_messages(&self, query: &str, limit: usize) -> Result<Vec<MessageSearchResult>> {
        let conn = self.conn.lock().await;

        // Sanitize query for FTS5 - remove special characters
        let sanitize_query = |q: &str| -> String {
            q.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == ' ')
                .collect::<String>()
        };

        let fts_query = query.split_whitespace()
            .map(|w| {
                let sanitized = sanitize_query(w);
                if sanitized.is_empty() { String::new() } else { format!("{}*", sanitized) }
            })
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(" OR ");

        if fts_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = conn.prepare_cached(
            r#"SELECT m.id, m.conversation_id, m.role, m.content, c.title
               FROM messages_fts m
               LEFT JOIN conversations c ON m.conversation_id = c.id
               WHERE messages_fts MATCH ?1
               ORDER BY bm25(messages_fts) DESC
               LIMIT ?2"#
        )?;

        let results = stmt.query_map(params![fts_query, limit], |row| {
            Ok(MessageSearchResult {
                message_id: row.get(0)?,
                conversation_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                conversation_title: row.get(4)?,
            })
        })?.collect::<Result<Vec<_>, _>>()?;

        Ok(results)
    }

    /// Semantic search using vector embeddings with pre-filtering
    /// Only loads embeddings from recent conversations for efficiency
    pub async fn semantic_search(&self, query_embedding: &[f32], limit: usize) -> Result<Vec<(ConversationRecord, f32)>> {
        let conn = self.conn.lock().await;

        // Pre-filter to only recent conversations with embeddings (more efficient)
        let mut stmt = conn.prepare_cached(
            r#"SELECT id, title, messages, summary, embedding, created_at, updated_at, tags
               FROM conversations
               WHERE embedding IS NOT NULL
               ORDER BY updated_at DESC
               LIMIT 1000"#
        )?;

        let records = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let title: Option<String> = row.get(1)?;
            let messages_json: String = row.get(2)?;
            let summary: Option<String> = row.get(3)?;
            let embedding_blob: Vec<u8> = row.get(4)?;
            let created_at_str: String = row.get(5)?;
            let updated_at_str: String = row.get(6)?;
            let tags_json: String = row.get(7)?;

            Ok((ConversationRecord {
                id,
                title,
                messages: serde_json::from_str(&messages_json)
                    .unwrap_or_default(),
                summary,
                embedding: Some(Self::blob_to_embedding(&embedding_blob)),
                created_at: DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                updated_at: DateTime::parse_from_rfc3339(&updated_at_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                tags: serde_json::from_str(&tags_json).unwrap_or_default(),
            }, Self::blob_to_embedding(&embedding_blob)))
        })?.collect::<Result<Vec<_>, _>>()?;

        // Calculate cosine similarity and sort
        let mut results: Vec<_> = records.into_iter()
            .map(|(record, embedding)| {
                let similarity = cosine_similarity(query_embedding, &embedding);
                (record, similarity)
            })
            .filter(|(_, sim)| *sim > 0.1) // Filter low-similarity results
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        Ok(results)
    }

    /// Semantic search at message level (more precise)
    pub async fn semantic_search_messages(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<MessageSemanticResult>> {
        let conn = self.conn.lock().await;

        // Get recent messages with embeddings
        let mut stmt = conn.prepare_cached(
            r#"SELECT me.id, me.conversation_id, me.message_idx, me.role, me.content, me.embedding,
                      c.title
               FROM message_embeddings me
               LEFT JOIN conversations c ON me.conversation_id = c.id
               WHERE me.embedding IS NOT NULL
               ORDER BY me.created_at DESC
               LIMIT 500"#
        )?;

        let entries = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let conversation_id: String = row.get(1)?;
            let message_idx: i32 = row.get(2)?;
            let role: String = row.get(3)?;
            let content: String = row.get(4)?;
            let embedding_blob: Vec<u8> = row.get(5)?;
            let conversation_title: Option<String> = row.get(6)?;

            Ok((
                id,
                conversation_id,
                message_idx as usize,
                role,
                content,
                Self::blob_to_embedding(&embedding_blob),
                conversation_title,
            ))
        })?.collect::<Result<Vec<_>, _>>()?;

        // Calculate similarity and sort
        let mut results: Vec<_> = entries.into_iter()
            .map(|(id, conv_id, idx, role, content, embedding, title)| {
                let similarity = cosine_similarity(query_embedding, &embedding);
                MessageSemanticResult {
                    id,
                    conversation_id: conv_id,
                    message_idx: idx,
                    role,
                    content,
                    similarity,
                    conversation_title: title,
                }
            })
            .filter(|r| r.similarity > 0.2)
            .collect();

        results.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        Ok(results)
    }

    /// Delete a conversation
    pub async fn delete_conversation(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;

        conn.execute("DELETE FROM conversations WHERE id = ?1", params![id])?;
        conn.execute("DELETE FROM conversations_fts WHERE id = ?1", params![id])?;
        conn.execute("DELETE FROM messages_fts WHERE conversation_id = ?1", params![id])?;
        conn.execute("DELETE FROM message_embeddings WHERE conversation_id = ?1", params![id])?;

        Ok(())
    }

    /// Save a knowledge entry
    pub async fn save_knowledge(&self, entry: &KnowledgeEntry) -> Result<()> {
        let conn = self.conn.lock().await;

        let embedding_blob = entry.embedding.as_ref()
            .map(|e| Self::embedding_to_blob(e));

        conn.execute(
            r#"INSERT OR REPLACE INTO knowledge
               (id, content, embedding, source, importance, access_count, created_at, last_accessed)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
            params![
                entry.id,
                entry.content,
                embedding_blob,
                entry.source,
                entry.importance,
                entry.access_count,
                entry.created_at.to_rfc3339(),
                entry.last_accessed.to_rfc3339(),
            ]
        )?;

        // Update FTS index
        conn.execute(
            "DELETE FROM knowledge_fts WHERE id = ?1",
            params![entry.id]
        ).ok();

        conn.execute(
            "INSERT INTO knowledge_fts (id, content) VALUES (?1, ?2)",
            params![entry.id, entry.content]
        )?;

        Ok(())
    }

    /// Search knowledge base using embeddings
    pub async fn search_knowledge(&self, query_embedding: &[f32], limit: usize) -> Result<Vec<(KnowledgeEntry, f32)>> {
        let conn = self.conn.lock().await;

        let mut stmt = conn.prepare_cached(
            "SELECT id, content, embedding, source, importance, access_count, created_at, last_accessed
             FROM knowledge
             WHERE embedding IS NOT NULL
             ORDER BY importance DESC
             LIMIT 500"
        )?;

        let entries = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let embedding_blob: Vec<u8> = row.get(2)?;
            let source: String = row.get(3)?;
            let importance: f32 = row.get(4)?;
            let access_count: u32 = row.get::<_, i32>(5)? as u32;
            let created_at_str: String = row.get(6)?;
            let last_accessed_str: String = row.get(7)?;

            Ok((KnowledgeEntry {
                id,
                content,
                embedding: Some(Self::blob_to_embedding(&embedding_blob)),
                source,
                importance,
                access_count,
                created_at: DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                last_accessed: DateTime::parse_from_rfc3339(&last_accessed_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            }, Self::blob_to_embedding(&embedding_blob)))
        })?.collect::<Result<Vec<_>, _>>()?;

        // Calculate cosine similarity and sort
        let mut results: Vec<_> = entries.into_iter()
            .map(|(entry, embedding)| {
                let similarity = cosine_similarity(query_embedding, &embedding);
                (entry, similarity)
            })
            .filter(|(_, sim)| *sim > 0.1)
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        Ok(results)
    }

    /// Get conversation count
    pub async fn conversation_count(&self) -> Result<usize> {
        let conn = self.conn.lock().await;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM conversations",
            [],
            |row| row.get(0)
        )?;

        Ok(count as usize)
    }

    /// Get knowledge count
    pub async fn knowledge_count(&self) -> Result<usize> {
        let conn = self.conn.lock().await;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM knowledge",
            [],
            |row| row.get(0)
        )?;

        Ok(count as usize)
    }

    /// Get memory statistics
    pub async fn stats(&self) -> Result<MemoryStats> {
        let conn = self.conn.lock().await;

        let conversations: i64 = conn.query_row(
            "SELECT COUNT(*) FROM conversations", [], |row| row.get(0)
        )?;

        let messages: i64 = conn.query_row(
            "SELECT COUNT(*) FROM message_embeddings", [], |row| row.get(0)
        )?;

        let knowledge: i64 = conn.query_row(
            "SELECT COUNT(*) FROM knowledge", [], |row| row.get(0)
        )?;

        let embeddings: i64 = conn.query_row(
            "SELECT COUNT(*) FROM conversations WHERE embedding IS NOT NULL", [], |row| row.get(0)
        )?;

        let oldest: Option<String> = conn.query_row(
            "SELECT MIN(created_at) FROM conversations", [],
            |row| row.get(0)
        ).ok();

        let newest: Option<String> = conn.query_row(
            "SELECT MAX(updated_at) FROM conversations", [],
            |row| row.get(0)
        ).ok();

        Ok(MemoryStats {
            total_conversations: conversations as usize,
            total_messages: messages as usize,
            total_knowledge: knowledge as usize,
            conversations_with_embeddings: embeddings as usize,
            oldest_conversation: oldest,
            newest_conversation: newest,
        })
    }

    /// Clean up old conversations (keep last N)
    pub async fn cleanup_old(&self, keep_last: usize) -> Result<usize> {
        let conn = self.conn.lock().await;

        // Check if we need to clean up
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM conversations", [],
            |row| row.get(0)
        ).unwrap_or(0);

        if count as usize <= keep_last {
            return Ok(0);
        }

        // Delete old conversations
        let deleted = conn.execute(
            r#"DELETE FROM conversations WHERE id IN (
                SELECT id FROM conversations
                ORDER BY updated_at DESC
                LIMIT -1 OFFSET ?
            )"#,
            params![keep_last]
        )?;

        // Clean up related tables
        conn.execute(
            r#"DELETE FROM conversations_fts WHERE id NOT IN (SELECT id FROM conversations)"#,
            []
        ).ok();

        conn.execute(
            r#"DELETE FROM messages_fts WHERE conversation_id NOT IN (SELECT id FROM conversations)"#,
            []
        ).ok();

        conn.execute(
            r#"DELETE FROM message_embeddings WHERE conversation_id NOT IN (SELECT id FROM conversations)"#,
            []
        ).ok();

        // Vacuum to reclaim space
        conn.execute("VACUUM", [])?;

        Ok(deleted)
    }

    /// Convert embedding vector to binary blob
    fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
        let mut blob = Vec::with_capacity(embedding.len() * 4);
        for &val in embedding {
            blob.extend_from_slice(&val.to_le_bytes());
        }
        blob
    }

    /// Convert binary blob to embedding vector
    fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
        let len = blob.len() / 4;
        let mut embedding = Vec::with_capacity(len);
        for i in 0..len {
            let bytes = &blob[i * 4..(i + 1) * 4];
            let val = f32::from_le_bytes(bytes.try_into().unwrap_or([0; 4]));
            embedding.push(val);
        }
        embedding
    }
}

/// Result of message-level search
#[derive(Debug, Clone)]
pub struct MessageSearchResult {
    pub message_id: String,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    pub conversation_title: Option<String>,
}

/// Result of message-level semantic search
#[derive(Debug, Clone)]
pub struct MessageSemanticResult {
    pub id: i64,
    pub conversation_id: String,
    pub message_idx: usize,
    pub role: String,
    pub content: String,
    pub similarity: f32,
    pub conversation_title: Option<String>,
}

/// Memory database statistics
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub total_conversations: usize,
    pub total_messages: usize,
    pub total_knowledge: usize,
    pub conversations_with_embeddings: usize,
    pub oldest_conversation: Option<String>,
    pub newest_conversation: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_save_and_load_conversation() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = SqliteMemoryStore::new(&db_path).await.unwrap();

        let record = ConversationRecord {
            id: "test-id".to_string(),
            title: Some("Test Conversation".to_string()),
            messages: vec![
                crate::types::Message {
                    role: crate::types::Role::User,
                    content: "Hello world".to_string(),
                    timestamp: Utc::now(),
                }
            ],
            summary: None,
            embedding: Some(vec![0.1, 0.2, 0.3]),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            tags: vec!["test".to_string()],
        };

        store.save_conversation(&record).await.unwrap();

        let loaded = store.load_conversation("test-id").await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.id, "test-id");
        assert_eq!(loaded.title, Some("Test Conversation".to_string()));
    }

    #[tokio::test]
    async fn test_stats() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = SqliteMemoryStore::new(&db_path).await.unwrap();

        let stats = store.stats().await.unwrap();
        assert_eq!(stats.total_conversations, 0);
    }
}