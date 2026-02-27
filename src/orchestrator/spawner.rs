//! Agent Spawner Module
//!
//! Manages agent lifecycle with proper dual-channel communication via AgentBus
//! and full ReAct tool-calling loops for subagents.

use crate::agent::llm::{OpenRouterClient, ChatMessage};
use crate::agent::tool_loop::{run_tool_loop, ToolLoopConfig};
use crate::agent::tools::{ToolContext, builtin_tools};
use crate::orchestrator::agent_types::SubagentType;
use crate::orchestrator::bus::{AgentBus, AgentMessage, AgentReceiver};
use crate::orchestrator::context::{SharedContext, AgentStatus, AgentInfo};
use crate::orchestrator::orchestrator::{AgentSpec, ExecutionMode};

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};
use uuid::Uuid;

const MAX_AGENTS: usize = 10;
/// Maximum children any single agent can spawn
pub const MAX_CHILDREN_PER_AGENT: usize = 5;

/// Agent spawner with proper dual-channel communication
pub struct AgentSpawner {
    context: Arc<SharedContext>,
    bus: Arc<AgentBus>,
    handles: Vec<SubagentHandle>,
    /// Tokio task handles for spawned agents — used to abort on cancellation
    task_handles: Vec<tokio::task::JoinHandle<()>>,
    /// Current nesting depth
    pub current_depth: u32,
    /// Maximum nesting depth
    pub max_depth: u32,
}

impl Drop for AgentSpawner {
    fn drop(&mut self) {
        // Abort all running agent tasks when the spawner is dropped (e.g., on Ctrl+C cancel)
        for handle in self.task_handles.drain(..) {
            handle.abort();
        }
    }
}

/// Handle to a spawned subagent
pub struct SubagentHandle {
    pub id: String,
    pub agent_type: SubagentType,
    pub task_description: String,
    /// Receive results from child via AgentBus parent channel
    pub result_receiver: AgentReceiver,
}

impl AgentSpawner {
    pub fn new(context: Arc<SharedContext>, bus: Arc<AgentBus>) -> Self {
        Self { context, bus, handles: Vec::new(), task_handles: Vec::new(), current_depth: 0, max_depth: 2 }
    }

    pub async fn spawn_batch(&mut self, specs: Vec<AgentSpec>, _mode: ExecutionMode) -> Result<Vec<String>> {
        if specs.len() > MAX_AGENTS {
            return Err(anyhow::anyhow!("Too many agents: {} > {}", specs.len(), MAX_AGENTS));
        }

        let mut ids = Vec::new();
        for spec in specs {
            let agent_type = SubagentType::from_capability(&spec.capability);
            let id = self.spawn_typed(spec, agent_type).await?;
            ids.push(id);
        }
        Ok(ids)
    }

    /// Spawn a typed subagent with proper tool restrictions and dual channels
    pub async fn spawn_typed(&mut self, spec: AgentSpec, agent_type: SubagentType) -> Result<String> {
        // Depth check
        if self.current_depth >= self.max_depth {
            anyhow::bail!(
                "Maximum agent nesting depth ({}) reached, cannot spawn more children",
                self.max_depth
            );
        }
        // Children-per-agent check
        if self.handles.len() >= MAX_CHILDREN_PER_AGENT {
            anyhow::bail!(
                "Maximum children per agent ({}) reached",
                MAX_CHILDREN_PER_AGENT
            );
        }
        let agent_id = Uuid::new_v4().to_string();
        let short_id = format!("{}-{}", agent_type.display_name(), &agent_id[..8]);
        let ctx_agent_type = Self::subagent_to_context_type(&agent_type);

        info!("Spawning {} agent {} with model {}", agent_type.display_name(), short_id, spec.model);

        self.context.register_agent(
            agent_id.clone(),
            short_id.clone(),
            ctx_agent_type,
            spec.model.clone(),
        ).await;

        // Create dual channels via AgentBus:
        // 1. task_channel: parent sends tasks -> child receives
        let (task_sender, mut task_receiver) = self.bus.create_channel(&agent_id);
        self.bus.register_child(agent_id.clone(), task_sender).await;

        // 2. result_channel: child sends results -> parent receives
        let (result_sender, result_receiver) = self.bus.create_channel(format!("{}-results", agent_id));

        let context = self.context.clone();
        let id_clone = agent_id.clone();
        let client = context.client.clone();
        let model = spec.model.clone();
        let task_desc = spec.task.clone();
        let agent_type_clone = agent_type.clone();

        let join_handle = tokio::spawn(async move {
            context.update_agent_status(&id_clone, AgentStatus::Ready).await;

            loop {
                tokio::select! {
                    Some(msg) = task_receiver.recv() => {
                        match msg {
                            AgentMessage::Task { task_id, description, context: _task_ctx } => {
                                context.update_agent_status(&id_clone, AgentStatus::Busy).await;
                                context.record_task_start(
                                    task_id.clone(),
                                    id_clone.clone(),
                                    description.clone(),
                                ).await;

                                // Use full ReAct tool loop with proper ToolContext
                                let tool_ctx = ToolContext::with_project_paths();
                                let allowed_tools = agent_type_clone.filter_tools(builtin_tools());

                                let config = ToolLoopConfig {
                                    model: model.clone(),
                                    system_prompt: agent_type_clone.system_prompt().to_string(),
                                    allowed_tools,
                                    max_iterations: agent_type_clone.max_iterations(),
                                    max_tokens: 4096,
                                    timeout_secs: 600,
                                    on_tool_start: None,
                                    on_tool_complete: None,
                                    on_progress: None,
                                };

                                let initial_messages = vec![
                                    ChatMessage::user(description.clone()),
                                ];

                                match run_tool_loop(&client, initial_messages, &tool_ctx, &config).await {
                                    Ok(result) => {
                                        context.record_task_complete(
                                            &task_id, true,
                                            Some(result.final_response.clone()),
                                            None, None,
                                        ).await;

                                        let _ = result_sender.send(AgentMessage::TaskResult {
                                            task_id,
                                            success: result.success,
                                            output: result.final_response,
                                            metadata: serde_json::json!({
                                                "iterations": result.iterations,
                                                "tool_calls": result.tool_calls_made,
                                            }),
                                        });
                                    }
                                    Err(e) => {
                                        warn!("Agent {} task failed: {}", id_clone, e);
                                        let error_msg = format!("Error: {}", e);
                                        context.record_task_complete(
                                            &task_id, false,
                                            Some(error_msg.clone()),
                                            None, None,
                                        ).await;

                                        let _ = result_sender.send(AgentMessage::TaskResult {
                                            task_id,
                                            success: false,
                                            output: error_msg,
                                            metadata: serde_json::json!({}),
                                        });
                                    }
                                }
                                context.update_agent_status(&id_clone, AgentStatus::Ready).await;
                            }
                            AgentMessage::Shutdown => {
                                info!("Agent {} shutting down", id_clone);
                                context.update_agent_status(&id_clone, AgentStatus::Shutdown).await;
                                break;
                            }
                            _ => {
                                warn!("Agent {} received unexpected message", id_clone);
                            }
                        }
                    }
                    _ = tokio::time::sleep(tokio::time::Duration::from_secs(120)) => {
                        // Heartbeat timeout - agent is idle
                    }
                    else => break,
                }
            }
        });

        self.task_handles.push(join_handle);
        self.handles.push(SubagentHandle {
            id: agent_id.clone(),
            agent_type,
            task_description: task_desc,
            result_receiver,
        });

        info!("Agent {} spawned", agent_id);
        Ok(agent_id)
    }

    /// Assign a task and wait for the result (blocking)
    pub async fn assign_and_wait(
        &mut self,
        agent_id: &str,
        task: String,
        context: serde_json::Value,
        timeout: Duration,
    ) -> Result<String> {
        let task_id = Uuid::new_v4().to_string();

        // Send task to child via bus
        self.bus.send_to_child(agent_id, AgentMessage::Task {
            task_id: task_id.clone(),
            description: task,
            context,
        }).await.map_err(|e| anyhow::anyhow!("{}", e))?;

        // Wait for result from the result_receiver
        let handle = self.handles.iter_mut()
            .find(|h| h.id == agent_id)
            .ok_or_else(|| anyhow::anyhow!("Agent {} not found", agent_id))?;

        match tokio::time::timeout(timeout, handle.result_receiver.recv()).await {
            Ok(Some(AgentMessage::TaskResult { output, success, .. })) => {
                if success {
                    Ok(output)
                } else {
                    Err(anyhow::anyhow!("Agent task failed: {}", output))
                }
            }
            Ok(Some(AgentMessage::Error { error, .. })) => {
                Err(anyhow::anyhow!("Agent error: {}", error))
            }
            Ok(Some(_)) => {
                Err(anyhow::anyhow!("Unexpected message from agent"))
            }
            Ok(None) => {
                Err(anyhow::anyhow!("Agent channel closed"))
            }
            Err(_) => {
                Err(anyhow::anyhow!("Agent timed out after {:?}", timeout))
            }
        }
    }

    /// Assign a task without waiting (background)
    pub async fn assign_background(
        &self,
        agent_id: &str,
        task: String,
        context: serde_json::Value,
    ) -> Result<String> {
        let task_id = Uuid::new_v4().to_string();

        self.bus.send_to_child(agent_id, AgentMessage::Task {
            task_id: task_id.clone(),
            description: task,
            context,
        }).await.map_err(|e| anyhow::anyhow!("{}", e))?;

        Ok(task_id)
    }

    /// Non-blocking poll for result from a specific agent
    pub fn poll_result(&mut self, agent_id: &str) -> Option<String> {
        if let Some(handle) = self.handles.iter_mut().find(|h| h.id == agent_id) {
            match handle.result_receiver.try_recv() {
                Ok(AgentMessage::TaskResult { output, .. }) => Some(output),
                _ => None,
            }
        } else {
            None
        }
    }

    pub async fn list_agents(&self) -> Vec<AgentInfo> {
        self.context.list_agents().await
    }

    pub async fn shutdown_all(&mut self) -> Result<()> {
        info!("Shutting down all agents");
        // Send graceful shutdown messages
        for handle in &self.handles {
            let _ = self.bus.send_to_child(&handle.id, AgentMessage::Shutdown).await;
        }
        self.handles.clear();
        // Abort all tokio tasks to ensure immediate cleanup
        for task_handle in self.task_handles.drain(..) {
            task_handle.abort();
        }
        Ok(())
    }

    fn subagent_to_context_type(st: &SubagentType) -> crate::orchestrator::context::AgentType {
        use crate::orchestrator::context::AgentType;
        match st {
            SubagentType::Explore => AgentType::Utility,
            SubagentType::Plan => AgentType::Planner,
            SubagentType::Bash => AgentType::Utility,
            SubagentType::Coder => AgentType::Coder,
            SubagentType::Researcher => AgentType::Researcher,
            SubagentType::General => AgentType::Utility,
        }
    }
}

pub fn create_agent_spec(capability: &str, task: &str, model: &str) -> AgentSpec {
    AgentSpec { capability: capability.to_string(), task: task.to_string(), model: model.to_string() }
}
