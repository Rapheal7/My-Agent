//! Conversation history management with optional persistence

use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::RwLock;

// Re-export types from the shared types module
pub use crate::types::{Message, Role};

/// Conversation history
pub struct Conversation {
    /// Unique conversation ID
    pub id: String,
    /// All messages in the conversation
    pub messages: Vec<Message>,
    /// When the conversation was created
    pub created_at: DateTime<Utc>,
    /// When the conversation was last updated
    pub updated_at: DateTime<Utc>,
    /// Optional title for the conversation
    pub title: Option<String>,
}

impl Conversation {
    /// Create a new empty conversation
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            title: None,
        }
    }

    /// Create a conversation from a ConversationRecord
    pub fn from_record(record: crate::memory::ConversationRecord) -> Self {
        Self {
            id: record.id,
            messages: record.messages,
            created_at: record.created_at,
            updated_at: record.updated_at,
            title: record.title,
        }
    }

    /// Convert to a ConversationRecord for persistence
    pub fn to_record(&self) -> crate::memory::ConversationRecord {
        crate::memory::ConversationRecord {
            id: self.id.clone(),
            title: self.title.clone(),
            messages: self.messages.clone(),
            summary: None,
            embedding: None,
            created_at: self.created_at,
            updated_at: self.updated_at,
            tags: Vec::new(),
        }
    }

    /// Add a message to the conversation
    pub fn add_message(&mut self, role: Role, content: String) {
        let now = Utc::now();
        self.messages.push(Message {
            role,
            content,
            timestamp: now,
        });
        self.updated_at = now;

        // Auto-generate title from first user message if not set
        if self.title.is_none() {
            if let Some(first_user_msg) = self.messages.iter()
                .find(|m| m.role == Role::User)
            {
                let content = &first_user_msg.content;
                if content.len() > 50 {
                    self.title = Some(format!("{}...", content.chars().take(50).collect::<String>()));
                } else {
                    self.title = Some(content.clone());
                }
            }
        }
    }

    /// Get messages formatted for LLM API
    pub fn to_llm_messages(&self) -> Vec<(String, String)> {
        self.messages.iter()
            .map(|m| (m.role.to_openai_string().to_string(), m.content.clone()))
            .collect()
    }

    /// Get the last N messages for context
    pub fn last_n_messages(&self, n: usize) -> &[Message] {
        let start = self.messages.len().saturating_sub(n);
        &self.messages[start..]
    }

    /// Get a summary of the conversation for display
    pub fn summary(&self) -> String {
        let msg_count = self.messages.len();
        let user_count = self.messages.iter().filter(|m| m.role == Role::User).count();
        let assistant_count = self.messages.iter().filter(|m| m.role == Role::Assistant).count();

        format!(
            "Conversation '{}' ({}): {} messages ({} user, {} assistant)",
            self.title.as_deref().unwrap_or("Untitled"),
            self.id,
            msg_count,
            user_count,
            assistant_count
        )
    }

    /// Clear all messages (keeps the same ID)
    pub fn clear(&mut self) {
        self.messages.clear();
        self.updated_at = Utc::now();
    }

    /// Check if the conversation is empty
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Get the message count
    pub fn len(&self) -> usize {
        self.messages.len()
    }
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

/// Conversation manager with optional persistence support
pub struct ConversationManager {
    memory_store: Option<Arc<crate::memory::MemoryStore>>,
    current_conversation: Arc<RwLock<Conversation>>,
}

impl ConversationManager {
    /// Create a new conversation manager without persistence
    pub fn new() -> Self {
        Self {
            memory_store: None,
            current_conversation: Arc::new(RwLock::new(Conversation::new())),
        }
    }

    /// Create a conversation manager with persistence
    pub fn with_persistence(memory_store: Arc<crate::memory::MemoryStore>) -> Self {
        Self {
            memory_store: Some(memory_store),
            current_conversation: Arc::new(RwLock::new(Conversation::new())),
        }
    }

    /// Start a new conversation
    pub async fn start_new(&self) -> String {
        let mut conv = self.current_conversation.write().await;
        *conv = Conversation::new();
        conv.id.clone()
    }

    /// Load an existing conversation by ID
    pub async fn load_conversation(&self, id: &str) -> anyhow::Result<bool> {
        if let Some(ref store) = self.memory_store {
            if let Some(record) = store.load_conversation(id).await? {
                let mut conv = self.current_conversation.write().await;
                *conv = Conversation::from_record(record);
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Get the current conversation (read access)
    pub async fn current(&self) -> tokio::sync::RwLockReadGuard<'_, Conversation> {
        self.current_conversation.read().await
    }

    /// Add a message to the current conversation and save
    pub async fn add_message(&self, role: Role, content: String) -> anyhow::Result<()> {
        let mut conv = self.current_conversation.write().await;
        conv.add_message(role, content);

        // Save if memory store is configured
        if let Some(ref store) = self.memory_store {
            store.save_conversation(&conv.to_record()).await?;
        }

        Ok(())
    }

    /// Save the current conversation
    pub async fn save(&self) -> anyhow::Result<()> {
        if let Some(ref store) = self.memory_store {
            let conv = self.current_conversation.read().await;
            store.save_conversation(&conv.to_record()).await?;
        }
        Ok(())
    }

    /// List recent conversations
    pub async fn list_recent(&self, limit: usize) -> anyhow::Result<Vec<crate::memory::ConversationRecord>> {
        if let Some(ref store) = self.memory_store {
            Ok(store.list_conversations(limit, 0).await?)
        } else {
            Ok(Vec::new())
        }
    }

    /// Search conversations
    pub async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<crate::memory::ConversationRecord>> {
        if let Some(ref store) = self.memory_store {
            Ok(store.search_conversations(query, limit).await?)
        } else {
            Ok(Vec::new())
        }
    }
}

impl Default for ConversationManager {
    fn default() -> Self {
        Self::new()
    }
}
