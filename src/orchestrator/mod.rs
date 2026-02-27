//! Multi-agent orchestration module

pub mod orchestrator;
pub mod spawner;
pub mod agent_types;
pub mod router;
pub mod budget;
pub mod bus;
pub mod context;
pub mod cost;
pub mod cli;
pub mod pipeline;

// Re-export commonly used types
pub use orchestrator::{SmartReasoningOrchestrator, OrchestrationPlan, AgentSpec, TaskType, ExecutionMode};
pub use spawner::{AgentSpawner, create_agent_spec};
pub use agent_types::SubagentType;
