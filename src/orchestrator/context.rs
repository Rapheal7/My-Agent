//! Shared context between agents
//!
//! Provides shared state, memory, and resources for all agents in the system.

use crate::agent::llm::OpenRouterClient;
use crate::orchestrator::budget::BudgetManager;
use crate::security::ApprovalManager;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Shared context for all agents in a session
#[derive(Clone)]
pub struct SharedContext {
    /// Inner shared state
    inner: Arc<RwLock<ContextInner>>,
    /// Budget manager for cost tracking
    pub budget: BudgetManager,
    /// Approval manager for security
    pub approvals: ApprovalManager,
    /// LLM client for API calls
    pub client: OpenRouterClient,
}

/// Internal shared state
struct ContextInner {
    /// Session ID
    pub session_id: String,
    /// Agent registry
    pub agents: HashMap<String, AgentInfo>,
    /// Shared memory/knowledge base
    pub memory: HashMap<String, MemoryEntry>,
    /// File references shared between agents
    pub shared_files: HashMap<String, FileReference>,
    /// Task history
    pub task_history: Vec<TaskRecord>,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Last activity timestamp
    pub last_activity: DateTime<Utc>,
}

/// Information about a registered agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub agent_type: AgentType,
    pub model: String,
    pub status: AgentStatus,
    pub created_at: DateTime<Utc>,
    pub task_count: u32,
}

/// Types of agents
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentType {
    /// Code-focused agent
    Coder,
    /// Research/web search agent
    Researcher,
    /// Analysis and data processing agent
    Analyst,
    /// Planning and orchestration agent
    Planner,
    /// Utility agent (file ops, exploration, general tasks)
    Utility,
}

/// Agent status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentStatus {
    Initializing,
    Ready,
    Busy,
    Error(String),
    Shutdown,
}

/// Memory entry in shared context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub key: String,
    pub value: serde_json::Value,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub access_count: u32,
}

/// File reference shared between agents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileReference {
    pub path: String,
    pub description: String,
    pub shared_by: String,
    pub shared_at: DateTime<Utc>,
    pub content_hash: Option<String>,
}

/// Task execution record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub task_id: String,
    pub agent_id: String,
    pub description: String,
    pub status: TaskStatus,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub tokens_used: Option<u32>,
    pub cost: Option<f64>,
    /// Actual task result text (populated on completion)
    pub output: Option<String>,
}

/// Task execution status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

impl SharedContext {
    /// Create a new shared context
    pub fn new(client: OpenRouterClient) -> anyhow::Result<Self> {
        let now = Utc::now();
        let inner = ContextInner {
            session_id: Uuid::new_v4().to_string(),
            agents: HashMap::new(),
            memory: HashMap::new(),
            shared_files: HashMap::new(),
            task_history: Vec::new(),
            created_at: now,
            last_activity: now,
        };

        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
            budget: BudgetManager::new(),
            approvals: ApprovalManager::with_defaults(),
            client,
        })
    }

    /// Get session ID
    pub async fn session_id(&self) -> String {
        let inner = self.inner.read().await;
        inner.session_id.clone()
    }

    /// Register a new agent
    pub async fn register_agent(
        &self,
        id: String,
        name: String,
        agent_type: AgentType,
        model: String,
    ) {
        let mut inner = self.inner.write().await;
        inner.agents.insert(
            id.clone(),
            AgentInfo {
                id,
                name,
                agent_type,
                model,
                status: AgentStatus::Initializing,
                created_at: Utc::now(),
                task_count: 0,
            },
        );
        inner.last_activity = Utc::now();
    }

    /// Update agent status
    pub async fn update_agent_status(&self, agent_id: &str, status: AgentStatus) {
        let mut inner = self.inner.write().await;
        if let Some(agent) = inner.agents.get_mut(agent_id) {
            agent.status = status;
        }
        inner.last_activity = Utc::now();
    }

    /// Get agent info
    pub async fn get_agent(&self, agent_id: &str) -> Option<AgentInfo> {
        let inner = self.inner.read().await;
        inner.agents.get(agent_id).cloned()
    }

    /// List all registered agents
    pub async fn list_agents(&self) -> Vec<AgentInfo> {
        let inner = self.inner.read().await;
        inner.agents.values().cloned().collect()
    }

    /// Store a memory entry
    pub async fn set_memory(
        &self,
        key: impl Into<String>,
        value: impl Serialize,
        created_by: impl Into<String>,
    ) -> anyhow::Result<()> {
        let mut inner = self.inner.write().await;
        let entry = MemoryEntry {
            key: key.into(),
            value: serde_json::to_value(value)?,
            created_by: created_by.into(),
            created_at: Utc::now(),
            access_count: 0,
        };
        inner.memory.insert(entry.key.clone(), entry);
        inner.last_activity = Utc::now();
        Ok(())
    }

    /// Get a memory entry
    pub async fn get_memory(&self, key: &str) -> Option<MemoryEntry> {
        let mut inner = self.inner.write().await;

        // First, update the entry if it exists
        let entry_exists = inner.memory.contains_key(key);
        if entry_exists {
            if let Some(entry) = inner.memory.get_mut(key) {
                entry.access_count += 1;
            }
            inner.last_activity = Utc::now();
            // Return a cloned copy
            return inner.memory.get(key).cloned();
        }
        None
    }

    /// Share a file between agents
    pub async fn share_file(
        &self,
        path: impl Into<String>,
        description: impl Into<String>,
        shared_by: impl Into<String>,
    ) {
        let mut inner = self.inner.write().await;
        let path_str = path.into();
        let reference = FileReference {
            path: path_str.clone(),
            description: description.into(),
            shared_by: shared_by.into(),
            shared_at: Utc::now(),
            content_hash: None,
        };
        inner.shared_files.insert(path_str, reference);
        inner.last_activity = Utc::now();
    }

    /// Get shared file reference
    pub async fn get_shared_file(&self, path: &str) -> Option<FileReference> {
        let inner = self.inner.read().await;
        inner.shared_files.get(path).cloned()
    }

    /// List all shared files
    pub async fn list_shared_files(&self) -> Vec<FileReference> {
        let inner = self.inner.read().await;
        inner.shared_files.values().cloned().collect()
    }

    /// Record a task start
    pub async fn record_task_start(
        &self,
        task_id: String,
        agent_id: String,
        description: String,
    ) {
        let mut inner = self.inner.write().await;

        // Increment agent task count first
        if let Some(agent) = inner.agents.get_mut(&agent_id) {
            agent.task_count += 1;
        }

        // Then create and push record
        let record = TaskRecord {
            task_id,
            agent_id,
            description,
            status: TaskStatus::Running,
            started_at: Utc::now(),
            completed_at: None,
            tokens_used: None,
            cost: None,
            output: None,
        };
        inner.task_history.push(record);
        inner.last_activity = Utc::now();
    }

    /// Record task completion
    pub async fn record_task_complete(
        &self,
        task_id: &str,
        success: bool,
        output: Option<String>,
        tokens_used: Option<u32>,
        cost: Option<f64>,
    ) {
        let mut inner = self.inner.write().await;
        if let Some(record) = inner.task_history.iter_mut().find(|r| r.task_id == task_id) {
            record.status = if success {
                TaskStatus::Completed
            } else {
                TaskStatus::Failed("Unknown error".to_string())
            };
            record.completed_at = Some(Utc::now());
            record.output = output;
            record.tokens_used = tokens_used;
            record.cost = cost;
        }
        inner.last_activity = Utc::now();
    }

    /// Get task history
    pub async fn get_task_history(&self) -> Vec<TaskRecord> {
        let inner = self.inner.read().await;
        inner.task_history.clone()
    }

    /// Get session statistics
    pub async fn get_stats(&self) -> SessionStats {
        let inner = self.inner.read().await;
        SessionStats {
            session_id: inner.session_id.clone(),
            agent_count: inner.agents.len(),
            task_count: inner.task_history.len(),
            memory_entries: inner.memory.len(),
            shared_files: inner.shared_files.len(),
            created_at: inner.created_at,
            last_activity: inner.last_activity,
            duration_seconds: (Utc::now() - inner.created_at).num_seconds(),
        }
    }

    /// Remove an agent from the registry
    pub async fn unregister_agent(&self, agent_id: &str) {
        let mut inner = self.inner.write().await;
        inner.agents.remove(agent_id);
        inner.last_activity = Utc::now();
    }

    /// Find agents by type
    pub async fn find_agents_by_type(&self, agent_type: AgentType) -> Vec<AgentInfo> {
        let inner = self.inner.read().await;
        inner.agents.values().filter(|a| a.agent_type == agent_type).cloned().collect()
    }

    /// Find agents by status
    pub async fn find_agents_by_status(&self, status: AgentStatus) -> Vec<AgentInfo> {
        let inner = self.inner.read().await;
        inner.agents.values().filter(|a| a.status == status).cloned().collect()
    }

    /// Find agents ready for collaboration (Ready status, not busy)
    pub async fn find_available_agents(&self) -> Vec<AgentInfo> {
        let inner = self.inner.read().await;
        inner.agents.values().filter(|a| matches!(a.status, AgentStatus::Ready)).cloned().collect()
    }

    /// Find agents by capability (capability is stored in the agent_type)
    pub async fn find_agents_by_capability(&self, capability: &str) -> Vec<AgentInfo> {
        let target_type = match capability.to_lowercase().as_str() {
            "code" | "coder" | "developer" => Some(AgentType::Coder),
            "research" | "researcher" => Some(AgentType::Researcher),
            "analysis" | "analyst" | "analyze" => Some(AgentType::Analyst),
            "plan" | "planner" | "orchestrate" => Some(AgentType::Planner),
            // Consolidated into Utility
            "utility" | "file" | "filesystem" | "general" | "explore" | "explorer" | "search" | "discover" => Some(AgentType::Utility),
            _ => None,
        };

        if let Some(agent_type) = target_type {
            self.find_agents_by_type(agent_type).await
        } else {
            Vec::new()
        }
    }

    /// Get agent name by ID
    pub async fn get_agent_name(&self, agent_id: &str) -> Option<String> {
        let inner = self.inner.read().await;
        inner.agents.get(agent_id).map(|a| a.name.clone())
    }
}

/// Session statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    pub session_id: String,
    pub agent_count: usize,
    pub task_count: usize,
    pub memory_entries: usize,
    pub shared_files: usize,
    pub created_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub duration_seconds: i64,
}
