//! Smart Reasoning Orchestrator
//!
//! Uses configurable model (default Kimi K2.5) to analyze requests and create agent teams
//! Can dynamically create new skills when capabilities are needed

use crate::agent::llm::{OpenRouterClient, ChatMessage};
use crate::skills::default_registry;
use crate::config::Config;
use anyhow::Result;
use tracing::{info, debug, warn};

/// Maximum tokens allowed for the system prompt
const MAX_SYSTEM_PROMPT_TOKENS: usize = 100000; // 100k tokens max for system prompt

/// Roughly estimate token count (approximately 4 chars per token)
fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

/// The Smart Reasoning Orchestrator - the "brain" that decides what agents to create
pub struct SmartReasoningOrchestrator {
    client: OpenRouterClient,
    model: String,
}

impl SmartReasoningOrchestrator {
    pub fn new() -> Result<Self> {
        let client = OpenRouterClient::from_keyring()?;
        let config = Config::load().unwrap_or_default();
        let model = config.models.orchestrator.clone();
        Ok(Self { client, model })
    }

    pub fn with_client(client: OpenRouterClient) -> Self {
        let config = Config::load().unwrap_or_default();
        let model = config.models.orchestrator.clone();
        Self { client, model }
    }

    /// Create with custom model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Get available skills for the system prompt
    fn get_skills_description() -> String {
        let registry = default_registry();
        let skills = registry.list();

        let mut desc = String::from("Available Skills:\n");
        for skill in &skills {
            desc.push_str(&format!("- {} ({}): {}\n",
                skill.id, skill.name, skill.description));
        }
        desc.push_str("\nYou can create NEW skills by specifying:\n");
        desc.push_str("SKILL_NEEDED: <skill description>\n");
        desc.push_str("SKILL_NAME: <suggested name>\n");
        desc
    }

    /// Receive request from PersonaPlex (voice) or Text interface
    /// Analyze what's needed and create appropriate agent team
    pub async fn process_request(&self, request: &str) -> Result<OrchestrationPlan> {
        info!("Orchestrator analyzing request with model {}: {}", self.model, request);

        let system_prompt = Self::build_system_prompt();

        // Check token count and warn if too large
        let prompt_tokens = estimate_tokens(&system_prompt);
        if prompt_tokens > MAX_SYSTEM_PROMPT_TOKENS {
            warn!("System prompt is large ({} tokens), may exceed context limit", prompt_tokens);
        }
        debug!("System prompt size: ~{} tokens", prompt_tokens);

        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(request.to_string()),
        ];

        // Use configured model for orchestration
        let response = self.client.complete(
            &self.model,
            messages,
            Some(2048),
        ).await?;

        debug!("Orchestrator response: {}", response);

        // Parse the response into an orchestration plan
        let plan = self.parse_orchestration_response(&response)?;

        info!("Orchestration plan: {:?} agents needed, skill_needed: {:?}",
            if plan.needs_agents { plan.agents.len() } else { 0 },
            plan.skill_needed);

        Ok(plan)
    }

    /// Build the system prompt for orchestration
    fn build_system_prompt() -> String {
        let skills_desc = Self::get_skills_description();

        // Concise system prompt
        format!(r#"You are a task orchestrator. Analyze requests and determine agent teams needed.

{skills_desc}

Agent types:
- code: Programming tasks
- research: Web search and research
- analysis: Data analysis
- reasoning: Complex reasoning
- file: File operations
- explorer: Codebase/computer search and exploration (search files, find patterns, discover code)
- general: General tasks
- skill-creator: Creating new skills

Free models: qwen/qwen-2.5-coder-32b-instruct (code), meta-llama/llama-3.1-8b-instruct (general), moonshotai/kimi-k2.5 (skill-creator), z-ai/glm-5 (explorer)

If task needs a missing capability, add:
SKILL_NEEDED: <description>
SKILL_NAME: <name>

Respond EXACTLY in this format:
TASK_TYPE: <Simple|Conversation|Complex|MultiStep>
NEEDS_AGENTS: <yes|no>
EXECUTION_MODE: <Sequential|Parallel>
SKILL_NEEDED: <description or "none">
SKILL_NAME: <name or empty>
AGENTS:
- type: <type>, task: "<task>", model: "<model>""#)
    }

    /// Parse the orchestrator's response into a structured plan
    fn parse_orchestration_response(&self, response: &str) -> Result<OrchestrationPlan> {
        let mut plan = OrchestrationPlan::default();
        let mut agents = Vec::new();

        for line in response.lines() {
            let line = line.trim();

            if line.starts_with("TASK_TYPE:") {
                let task_type = line.trim_start_matches("TASK_TYPE:").trim();
                plan.task_type = match task_type.to_lowercase().as_str() {
                    "simple" => TaskType::Simple,
                    "conversation" => TaskType::Conversation,
                    "complex" => TaskType::Complex,
                    "multistep" | "multi_step" | "multi-step" => TaskType::MultiStep,
                    _ => TaskType::Simple,
                };
            } else if line.starts_with("NEEDS_AGENTS:") {
                let needs = line.trim_start_matches("NEEDS_AGENTS:").trim();
                plan.needs_agents = needs.eq_ignore_ascii_case("yes");
            } else if line.starts_with("EXECUTION_MODE:") {
                let mode = line.trim_start_matches("EXECUTION_MODE:").trim();
                plan.execution_mode = match mode.to_lowercase().as_str() {
                    "parallel" => ExecutionMode::Parallel,
                    _ => ExecutionMode::Sequential,
                };
            } else if line.starts_with("SKILL_NEEDED:") {
                let skill = line.trim_start_matches("SKILL_NEEDED:").trim();
                if skill != "none" && !skill.is_empty() {
                    plan.skill_needed = Some(skill.to_string());
                }
            } else if line.starts_with("SKILL_NAME:") {
                let name = line.trim_start_matches("SKILL_NAME:").trim();
                if !name.is_empty() {
                    plan.skill_name = Some(name.to_string());
                }
            } else if line.starts_with("-") && line.contains("type:") {
                // Parse agent specification
                if let Some(agent) = self.parse_agent_line(line) {
                    agents.push(agent);
                }
            }
        }

        plan.agents = agents;

        // If no agents were parsed but we need them, create a default agent
        if plan.needs_agents && plan.agents.is_empty() {
            plan.agents.push(AgentSpec {
                model: "meta-llama/llama-3.1-8b-instruct".to_string(),
                task: "Handle the request".to_string(),
                capability: "general".to_string(),
            });
        }

        Ok(plan)
    }

    fn parse_agent_line(&self, line: &str) -> Option<AgentSpec> {
        // Parse: - type: code, task: "design web scraper", model: "qwen/..."
        let line = line.trim_start_matches("-").trim();

        let mut agent_type = "general".to_string();
        let mut task = String::new();
        let mut model = "meta-llama/llama-3.1-8b-instruct".to_string();

        // Extract type
        if let Some(type_start) = line.find("type:") {
            let type_part = &line[type_start + 5..];
            if let Some(comma_pos) = type_part.find(',') {
                agent_type = type_part[..comma_pos].trim().trim_matches('"').to_string();
            } else {
                agent_type = type_part.trim().trim_matches('"').to_string();
            }
        }

        // Extract task
        if let Some(task_start) = line.find("task:") {
            let task_part = &line[task_start + 5..];
            if let Some(quote_start) = task_part.find('"') {
                let after_start = &task_part[quote_start + 1..];
                if let Some(quote_end) = after_start.find('"') {
                    task = after_start[..quote_end].to_string();
                }
            }
        }

        // Extract model
        if let Some(model_start) = line.find("model:") {
            let model_part = &line[model_start + 6..];
            if let Some(quote_start) = model_part.find('"') {
                let after_start = &model_part[quote_start + 1..];
                if let Some(quote_end) = after_start.find('"') {
                    model = after_start[..quote_end].to_string();
                }
            }
        }

        if task.is_empty() {
            task = format!("Perform {} task", agent_type);
        }

        Some(AgentSpec {
            model,
            task,
            capability: agent_type,
        })
    }

    /// Create a skill dynamically based on description
    pub async fn create_skill(&self, description: &str, name: Option<&str>) -> Result<String> {
        use crate::skills::generator::{SkillGenerator, GenerationRequest};
        use crate::skills::registry::Permission;

        info!("Creating skill: {} ({})", name.unwrap_or("auto-named"), description);

        let api_key = crate::security::keyring::get_api_key().unwrap_or_default();
        let generator = if !api_key.is_empty() {
            SkillGenerator::new().with_api_key(api_key)
        } else {
            SkillGenerator::new()
        };

        let request = GenerationRequest {
            description: description.to_string(),
            name: name.map(|s| s.to_string()),
            category: None,
            permissions: vec![Permission::ReadFiles],
            examples: vec![],
        };

        let generated = generator.generate(request).await?;
        let skill_id = generated.meta.id.clone();

        // Compile and register
        let registry = default_registry();
        let skill = generator.compile_skill(&generated)?;
        registry.register(skill)?;
        registry.save_skill(&generated.meta)?;

        info!("Skill created: {}", skill_id);
        Ok(skill_id)
    }
}

#[derive(Debug, Default)]
pub struct OrchestrationPlan {
    pub task_type: TaskType,
    pub needs_agents: bool,
    pub agents: Vec<AgentSpec>,
    pub execution_mode: ExecutionMode,
    pub skill_needed: Option<String>,
    pub skill_name: Option<String>,
}

#[derive(Debug, Default)]
pub enum TaskType {
    #[default]
    Simple,
    Conversation,
    Complex,
    MultiStep,
}

#[derive(Debug, Default)]
pub enum ExecutionMode {
    #[default]
    Sequential,
    Parallel,
}

#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub model: String,
    pub task: String,
    pub capability: String,
}

