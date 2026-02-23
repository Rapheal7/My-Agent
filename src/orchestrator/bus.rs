//! Agent communication bus
//!
//! Provides message passing between parent and child agents.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::{mpsc, Mutex};
use std::sync::Arc;
use uuid::Uuid;

/// Message types for agent communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMessage {
    /// Task assignment from parent to child
    Task {
        task_id: String,
        description: String,
        context: serde_json::Value,
    },
    /// Task result from child to parent
    TaskResult {
        task_id: String,
        success: bool,
        output: String,
        metadata: serde_json::Value,
    },
    /// Progress update from child
    Progress {
        task_id: String,
        percent: u8,
        message: String,
    },
    /// Request for clarification
    Clarification {
        task_id: String,
        question: String,
    },
    /// Response to clarification
    ClarificationResponse {
        task_id: String,
        answer: String,
    },
    /// Error from child agent
    Error {
        task_id: String,
        error: String,
    },
    /// Direct message from one agent to another
    DirectMessage {
        from_agent: String,
        to_agent: String,
        message_type: MessageType,
        content: serde_json::Value,
    },
    /// Collaboration request
    CollaborationRequest {
        from_agent: String,
        task_id: String,
        description: String,
        priority: Priority,
    },
    /// Collaboration response
    CollaborationResponse {
        from_agent: String,
        task_id: String,
        accepted: bool,
        reason: Option<String>,
    },
    /// Heartbeat to check if agent is alive
    Heartbeat,
    /// Agent is shutting down
    Shutdown,
}

/// Types of direct messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageType {
    /// Share knowledge/data
    KnowledgeShare,
    /// Request for help
    HelpRequest,
    /// Status update
    StatusUpdate,
    /// Custom message
    Custom(String),
}

/// Priority levels for collaboration requests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

/// Channel for sending messages to an agent
#[derive(Clone)]
pub struct AgentSender {
    pub agent_id: String,
    pub(crate) tx: mpsc::UnboundedSender<AgentMessage>,
}

impl AgentSender {
    /// Send a message to the agent
    pub fn send(&self, msg: AgentMessage) -> Result<(), mpsc::error::SendError<AgentMessage>> {
        self.tx.send(msg)
    }
}

/// Channel for receiving messages from an agent
pub struct AgentReceiver {
    pub agent_id: String,
    pub(crate) rx: mpsc::UnboundedReceiver<AgentMessage>,
}

impl AgentReceiver {
    /// Receive a message from the agent
    pub async fn recv(&mut self) -> Option<AgentMessage> {
        self.rx.recv().await
    }

    /// Try to receive a message without blocking
    pub fn try_recv(&mut self) -> Result<AgentMessage, mpsc::error::TryRecvError> {
        self.rx.try_recv()
    }
}

/// Communication bus for inter-agent messaging
pub struct AgentBus {
    /// Channels to child agents (parent -> child)
    child_channels: Mutex<HashMap<String, AgentSender>>,
    /// Channels from child agents (child -> parent)
    parent_channels: Mutex<HashMap<String, AgentSender>>,
    /// Broadcast channel for all agents
    broadcast_tx: mpsc::UnboundedSender<(String, AgentMessage)>,
    broadcast_rx: Mutex<mpsc::UnboundedReceiver<(String, AgentMessage)>>,
}

impl AgentBus {
    /// Create a new agent bus
    pub fn new() -> Self {
        let (broadcast_tx, broadcast_rx) = mpsc::unbounded_channel();
        Self {
            child_channels: Mutex::new(HashMap::new()),
            parent_channels: Mutex::new(HashMap::new()),
            broadcast_tx,
            broadcast_rx: Mutex::new(broadcast_rx),
        }
    }

    /// Create a new channel pair for an agent
    pub fn create_channel(&self, agent_id: impl Into<String>) -> (AgentSender, AgentReceiver) {
        let agent_id = agent_id.into();
        let (tx, rx) = mpsc::unbounded_channel();

        let sender = AgentSender {
            agent_id: agent_id.clone(),
            tx: tx.clone(),
        };

        let receiver = AgentReceiver {
            agent_id: agent_id.clone(),
            rx,
        };

        (sender, receiver)
    }

    /// Register a child agent channel (parent can send to child)
    pub async fn register_child(&self, agent_id: String, sender: AgentSender) {
        let mut channels = self.child_channels.lock().await;
        channels.insert(agent_id, sender);
    }

    /// Register a parent channel for a child (child can send to parent)
    pub async fn register_parent_channel(&self, child_id: String, parent_sender: AgentSender) {
        let mut channels = self.parent_channels.lock().await;
        channels.insert(child_id, parent_sender);
    }

    /// Send message to a specific child agent
    pub async fn send_to_child(&self, agent_id: &str, msg: AgentMessage) -> Result<(), String> {
        let channels = self.child_channels.lock().await;
        if let Some(sender) = channels.get(agent_id) {
            sender.send(msg).map_err(|e| format!("Failed to send: {:?}", e))
        } else {
            Err(format!("Agent {} not found", agent_id))
        }
    }

    /// Send message to parent from child
    pub async fn send_to_parent(&self, child_id: &str, msg: AgentMessage) -> Result<(), String> {
        let channels = self.parent_channels.lock().await;
        if let Some(sender) = channels.get(child_id) {
            sender.send(msg).map_err(|e| format!("Failed to send: {:?}", e))
        } else {
            Err(format!("Parent channel for {} not found", child_id))
        }
    }

    /// Broadcast message to all agents
    pub fn broadcast(&self, from_agent: String, msg: AgentMessage) -> Result<(), String> {
        self.broadcast_tx
            .send((from_agent, msg))
            .map_err(|e| format!("Broadcast failed: {:?}", e))
    }

    /// Get all child agent IDs
    pub async fn list_children(&self) -> Vec<String> {
        let channels = self.child_channels.lock().await;
        channels.keys().cloned().collect()
    }

    /// Remove a child agent channel
    pub async fn remove_child(&self, agent_id: &str) {
        let mut channels = self.child_channels.lock().await;
        channels.remove(agent_id);
    }

    /// Wait for broadcast messages
    pub async fn recv_broadcast(&self) -> Option<(String, AgentMessage)> {
        let mut rx = self.broadcast_rx.lock().await;
        rx.recv().await
    }

    /// Send a direct message from one agent to another
    /// This enables peer-to-peer communication without going through the parent
    pub async fn send_agent_to_agent(
        &self,
        from_agent: &str,
        to_agent: &str,
        message: AgentMessage,
    ) -> Result<(), String> {
        // For now, route through child channels (agents register as children)
        // In a full P2P setup, agents would have direct channels
        let channels = self.child_channels.lock().await;
        if let Some(sender) = channels.get(to_agent) {
            sender.send(message).map_err(|e| format!("Failed to send direct message: {:?}", e))
        } else {
            Err(format!("Target agent {} not found", to_agent))
        }
    }

    /// Check if an agent channel exists
    pub async fn agent_exists(&self, agent_id: &str) -> bool {
        let channels = self.child_channels.lock().await;
        channels.contains_key(agent_id)
    }

    /// Get number of registered agents
    pub async fn agent_count(&self) -> usize {
        let channels = self.child_channels.lock().await;
        channels.len()
    }
}

impl Default for AgentBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle for communicating with a spawned agent
#[derive(Clone)]
pub struct AgentHandle {
    pub id: String,
    pub sender: AgentSender,
}

impl AgentHandle {
    /// Send a message to the agent
    pub fn send(&self, msg: AgentMessage) -> Result<(), mpsc::error::SendError<AgentMessage>> {
        self.sender.send(msg)
    }
}
