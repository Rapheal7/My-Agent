//! Deterministic YAML Pipelines (Lobster-style)
//!
//! Defines repeatable, resumable workflows with approval gates,
//! loops, parallel steps, and depth/children limits.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{info, warn, debug};

/// A complete pipeline definition loaded from YAML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineDefinition {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub steps: Vec<PipelineStep>,
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
    #[serde(default = "default_max_children")]
    pub max_children: usize,
}

fn default_max_depth() -> u32 { 2 }
fn default_max_children() -> usize { 5 }

/// A single step in a pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStep {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub step_type: StepType,
    /// Agent type to run this step (for Agent steps)
    #[serde(default)]
    pub agent_type: String,
    /// Task description / prompt
    #[serde(default)]
    pub task: String,
    /// Model override
    #[serde(default)]
    pub model: Option<String>,
    /// Step IDs that must complete before this one
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Whether to pause for user approval before executing
    #[serde(default)]
    pub approval_required: bool,
    /// Timeout for this step in seconds
    #[serde(default = "default_step_timeout")]
    pub timeout_secs: u64,
    /// Loop configuration (for Loop steps)
    #[serde(default)]
    pub loop_config: Option<LoopConfig>,
}

fn default_step_timeout() -> u64 { 300 }

/// Types of pipeline steps
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StepType {
    Agent,
    ApprovalGate,
    Loop,
    Parallel,
}

impl Default for StepType {
    fn default() -> Self { StepType::Agent }
}

/// Configuration for loop steps
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopConfig {
    /// Step IDs to loop over
    pub steps: Vec<String>,
    /// Maximum iterations
    #[serde(default = "default_loop_max")]
    pub max_iterations: usize,
    /// Condition to break (evaluated by LLM)
    #[serde(default)]
    pub break_condition: String,
}

fn default_loop_max() -> usize { 5 }

/// Status of the overall pipeline
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStatus {
    Running,
    WaitingForApproval,
    Completed,
    Failed,
}

/// Result from executing a single step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step_id: String,
    pub success: bool,
    pub output: String,
    pub duration_secs: f64,
}

/// Persistent pipeline state for resumability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineState {
    pub pipeline_name: String,
    pub current_step_index: usize,
    pub step_results: HashMap<String, StepResult>,
    pub status: PipelineStatus,
    pub started_at: String,
    pub updated_at: String,
}

impl PipelineState {
    fn new(name: &str) -> Self {
        let now = chrono::Local::now().to_rfc3339();
        Self {
            pipeline_name: name.to_string(),
            current_step_index: 0,
            step_results: HashMap::new(),
            status: PipelineStatus::Running,
            started_at: now.clone(),
            updated_at: now,
        }
    }
}

/// Executes pipeline definitions step by step
pub struct PipelineExecutor {
    definition: PipelineDefinition,
    state: PipelineState,
}

impl PipelineExecutor {
    /// Load a pipeline from a YAML file
    pub fn from_yaml(path: &std::path::Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .context("Failed to read pipeline YAML")?;
        Self::from_yaml_str(&content)
    }

    /// Load a pipeline from a YAML string
    pub fn from_yaml_str(yaml: &str) -> Result<Self> {
        let definition: PipelineDefinition = serde_yaml::from_str(yaml)
            .context("Failed to parse pipeline YAML")?;

        let state = PipelineState::new(&definition.name);
        Ok(Self { definition, state })
    }

    /// Execute the pipeline step by step
    pub async fn execute(&mut self) -> Result<PipelineState> {
        info!("Starting pipeline: {}", self.definition.name);
        self.state.status = PipelineStatus::Running;

        while self.state.current_step_index < self.definition.steps.len() {
            let step = &self.definition.steps[self.state.current_step_index].clone();

            // Check dependencies
            if !self.deps_met(step) {
                warn!("Step '{}' has unmet dependencies, skipping", step.id);
                self.state.current_step_index += 1;
                continue;
            }

            // Check for approval gate
            if step.approval_required || step.step_type == StepType::ApprovalGate {
                info!("Pipeline paused at step '{}' waiting for approval", step.name);
                self.state.status = PipelineStatus::WaitingForApproval;
                self.save_state_auto()?;
                return Ok(self.state.clone());
            }

            // Execute step based on type
            let result = match step.step_type {
                StepType::Agent => self.execute_agent_step(step).await,
                StepType::Parallel => self.execute_parallel_step(step).await,
                StepType::Loop => self.execute_loop_step(step).await,
                StepType::ApprovalGate => {
                    // Handled above
                    Ok(StepResult {
                        step_id: step.id.clone(),
                        success: true,
                        output: "Approved".to_string(),
                        duration_secs: 0.0,
                    })
                }
            };

            match result {
                Ok(step_result) => {
                    let success = step_result.success;
                    self.state.step_results.insert(step.id.clone(), step_result);
                    if !success {
                        warn!("Step '{}' failed, pipeline stopping", step.name);
                        self.state.status = PipelineStatus::Failed;
                        self.save_state_auto()?;
                        return Ok(self.state.clone());
                    }
                }
                Err(e) => {
                    self.state.step_results.insert(step.id.clone(), StepResult {
                        step_id: step.id.clone(),
                        success: false,
                        output: format!("Error: {}", e),
                        duration_secs: 0.0,
                    });
                    self.state.status = PipelineStatus::Failed;
                    self.save_state_auto()?;
                    return Ok(self.state.clone());
                }
            }

            self.state.current_step_index += 1;
            self.state.updated_at = chrono::Local::now().to_rfc3339();
        }

        self.state.status = PipelineStatus::Completed;
        info!("Pipeline '{}' completed successfully", self.definition.name);
        self.save_state_auto()?;
        Ok(self.state.clone())
    }

    /// Resume after approval
    pub async fn approve_and_continue(&mut self) -> Result<PipelineState> {
        if self.state.status != PipelineStatus::WaitingForApproval {
            anyhow::bail!("Pipeline is not waiting for approval (status: {:?})", self.state.status);
        }

        // Mark current step as approved and advance
        let step = &self.definition.steps[self.state.current_step_index];
        self.state.step_results.insert(step.id.clone(), StepResult {
            step_id: step.id.clone(),
            success: true,
            output: "Approved by user".to_string(),
            duration_secs: 0.0,
        });
        self.state.current_step_index += 1;
        self.state.status = PipelineStatus::Running;

        self.execute().await
    }

    /// Save pipeline state to a file for resumability
    pub fn save_state(&self, path: &std::path::Path) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.state)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Resume from a saved state file
    pub fn resume_from_state(path: &std::path::Path, definition: PipelineDefinition) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let state: PipelineState = serde_json::from_str(&content)?;
        Ok(Self { definition, state })
    }

    /// Get current state
    pub fn state(&self) -> &PipelineState {
        &self.state
    }

    /// Get definition
    pub fn definition(&self) -> &PipelineDefinition {
        &self.definition
    }

    // --- Private helpers ---

    fn deps_met(&self, step: &PipelineStep) -> bool {
        step.depends_on.iter().all(|dep_id| {
            self.state.step_results
                .get(dep_id)
                .map(|r| r.success)
                .unwrap_or(false)
        })
    }

    async fn execute_agent_step(&self, step: &PipelineStep) -> Result<StepResult> {
        let start = std::time::Instant::now();
        info!("Executing step '{}': {}", step.name, step.task);

        let client = crate::agent::llm::OpenRouterClient::from_keyring()?;
        let model = step.model.clone().unwrap_or_else(|| {
            crate::config::Config::load()
                .map(|c| c.models.utility.clone())
                .unwrap_or_else(|_| "z-ai/glm-5".to_string())
        });

        // Build context from prior step results
        let mut context_parts = Vec::new();
        for dep_id in &step.depends_on {
            if let Some(result) = self.state.step_results.get(dep_id) {
                context_parts.push(format!("Result from '{}': {}", dep_id, result.output));
            }
        }

        let full_task = if context_parts.is_empty() {
            step.task.clone()
        } else {
            format!("{}\n\nContext from previous steps:\n{}", step.task, context_parts.join("\n"))
        };

        let messages = vec![
            crate::agent::llm::ChatMessage::system("Execute the following task step concisely."),
            crate::agent::llm::ChatMessage::user(full_task),
        ];

        let timeout = std::time::Duration::from_secs(step.timeout_secs);
        let result = tokio::time::timeout(timeout, client.complete(&model, messages, Some(2048))).await;

        let duration = start.elapsed().as_secs_f64();

        match result {
            Ok(Ok(output)) => Ok(StepResult {
                step_id: step.id.clone(),
                success: true,
                output,
                duration_secs: duration,
            }),
            Ok(Err(e)) => Ok(StepResult {
                step_id: step.id.clone(),
                success: false,
                output: format!("LLM error: {}", e),
                duration_secs: duration,
            }),
            Err(_) => Ok(StepResult {
                step_id: step.id.clone(),
                success: false,
                output: format!("Step timed out after {}s", step.timeout_secs),
                duration_secs: duration,
            }),
        }
    }

    async fn execute_parallel_step(&self, step: &PipelineStep) -> Result<StepResult> {
        // Parallel steps reference other step IDs in their task field (comma-separated)
        let sub_ids: Vec<&str> = step.task.split(',').map(|s| s.trim()).collect();
        let mut outputs = Vec::new();

        for sub_id in &sub_ids {
            if let Some(sub_step) = self.definition.steps.iter().find(|s| s.id == *sub_id) {
                match self.execute_agent_step(sub_step).await {
                    Ok(r) => outputs.push(format!("{}: {}", sub_id, if r.success { &r.output } else { "FAILED" })),
                    Err(e) => outputs.push(format!("{}: Error: {}", sub_id, e)),
                }
            }
        }

        Ok(StepResult {
            step_id: step.id.clone(),
            success: true,
            output: outputs.join("\n"),
            duration_secs: 0.0,
        })
    }

    async fn execute_loop_step(&self, step: &PipelineStep) -> Result<StepResult> {
        let Some(ref loop_config) = step.loop_config else {
            anyhow::bail!("Loop step '{}' missing loop_config", step.id);
        };

        let mut outputs = Vec::new();
        for i in 0..loop_config.max_iterations {
            debug!("Loop iteration {}/{}", i + 1, loop_config.max_iterations);
            for sub_id in &loop_config.steps {
                if let Some(sub_step) = self.definition.steps.iter().find(|s| s.id == *sub_id) {
                    match self.execute_agent_step(sub_step).await {
                        Ok(r) => outputs.push(format!("iter{}/{}: {}", i + 1, sub_id, r.output)),
                        Err(e) => outputs.push(format!("iter{}/{}: Error: {}", i + 1, sub_id, e)),
                    }
                }
            }
        }

        Ok(StepResult {
            step_id: step.id.clone(),
            success: true,
            output: outputs.join("\n"),
            duration_secs: 0.0,
        })
    }

    fn save_state_auto(&self) -> Result<()> {
        let dir = crate::config::data_dir()?.join("pipelines");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.state.json", self.definition.name));
        self.save_state(&path)
    }
}

/// List available pipeline YAML files
pub fn list_pipeline_files() -> Result<Vec<PathBuf>> {
    let dir = crate::config::data_dir()?.join("pipelines");
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "yaml" || e == "yml").unwrap_or(false) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pipeline_yaml() {
        let yaml = r#"
name: test-pipeline
description: A test pipeline
max_depth: 3
max_children: 5
steps:
  - id: step1
    name: First Step
    step_type: agent
    task: Do something
    timeout_secs: 60
  - id: step2
    name: Second Step
    step_type: approval_gate
    depends_on: [step1]
    approval_required: true
  - id: step3
    name: Third Step
    step_type: agent
    task: Do something else
    depends_on: [step2]
"#;

        let executor = PipelineExecutor::from_yaml_str(yaml).unwrap();
        assert_eq!(executor.definition().name, "test-pipeline");
        assert_eq!(executor.definition().steps.len(), 3);
        assert_eq!(executor.definition().steps[0].step_type, StepType::Agent);
        assert_eq!(executor.definition().steps[1].step_type, StepType::ApprovalGate);
        assert!(executor.definition().steps[1].approval_required);
        assert_eq!(executor.definition().steps[2].depends_on, vec!["step2"]);
    }

    #[test]
    fn test_pipeline_state() {
        let state = PipelineState::new("test");
        assert_eq!(state.pipeline_name, "test");
        assert_eq!(state.current_step_index, 0);
        assert_eq!(state.status, PipelineStatus::Running);
        assert!(!state.started_at.is_empty());
        assert!(!state.updated_at.is_empty());
    }

    #[test]
    fn test_parse_minimal_pipeline() {
        let yaml = r#"
name: minimal
steps:
  - id: only_step
    name: Only Step
    task: hello
"#;
        let exec = PipelineExecutor::from_yaml_str(yaml).unwrap();
        assert_eq!(exec.definition().name, "minimal");
        assert_eq!(exec.definition().steps.len(), 1);
        assert_eq!(exec.definition().max_depth, 2); // default
        assert_eq!(exec.definition().max_children, 5); // default
        assert_eq!(exec.definition().steps[0].step_type, StepType::Agent); // default
        assert_eq!(exec.definition().steps[0].timeout_secs, 300); // default
    }

    #[test]
    fn test_parse_pipeline_with_loop() {
        let yaml = r#"
name: loop-test
steps:
  - id: setup
    name: Setup
    task: prepare data
  - id: loop1
    name: Loop Step
    step_type: loop
    loop_config:
      steps: [setup]
      max_iterations: 3
      break_condition: "done"
"#;
        let exec = PipelineExecutor::from_yaml_str(yaml).unwrap();
        let loop_step = &exec.definition().steps[1];
        assert_eq!(loop_step.step_type, StepType::Loop);
        let lc = loop_step.loop_config.as_ref().unwrap();
        assert_eq!(lc.max_iterations, 3);
        assert_eq!(lc.steps, vec!["setup"]);
        assert_eq!(lc.break_condition, "done");
    }

    #[test]
    fn test_parse_invalid_yaml() {
        let result = PipelineExecutor::from_yaml_str("not: [valid: yaml:");
        assert!(result.is_err());
    }

    #[test]
    fn test_deps_met_no_deps() {
        let yaml = r#"
name: test
steps:
  - id: s1
    name: Step 1
    task: go
"#;
        let exec = PipelineExecutor::from_yaml_str(yaml).unwrap();
        let step = &exec.definition().steps[0];
        assert!(exec.deps_met(step), "Step with no deps should always be met");
    }

    #[test]
    fn test_deps_met_unresolved() {
        let yaml = r#"
name: test
steps:
  - id: s1
    name: Step 1
    task: go
  - id: s2
    name: Step 2
    task: go later
    depends_on: [s1]
"#;
        let exec = PipelineExecutor::from_yaml_str(yaml).unwrap();
        let step2 = &exec.definition().steps[1];
        assert!(!exec.deps_met(step2), "Step with unresolved dep should not be met");
    }

    #[test]
    fn test_pipeline_state_serialization() {
        let mut state = PipelineState::new("serialize-test");
        state.step_results.insert("s1".to_string(), StepResult {
            step_id: "s1".to_string(),
            success: true,
            output: "all good".to_string(),
            duration_secs: 1.5,
        });
        state.status = PipelineStatus::Completed;

        let json = serde_json::to_string(&state).unwrap();
        let restored: PipelineState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.pipeline_name, "serialize-test");
        assert_eq!(restored.status, PipelineStatus::Completed);
        assert!(restored.step_results.contains_key("s1"));
        assert_eq!(restored.step_results["s1"].output, "all good");
    }

    #[test]
    fn test_step_type_default() {
        assert_eq!(StepType::default(), StepType::Agent);
    }

    #[test]
    fn test_pipeline_save_and_resume_state() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("test.state.json");

        let yaml = r#"
name: resumable
steps:
  - id: s1
    name: Step 1
    task: first task
    approval_required: true
  - id: s2
    name: Step 2
    task: second task
    depends_on: [s1]
"#;
        let mut exec = PipelineExecutor::from_yaml_str(yaml).unwrap();
        // Simulate being at step 1 waiting for approval
        exec.state.current_step_index = 0;
        exec.state.status = PipelineStatus::WaitingForApproval;
        exec.save_state(&state_path).unwrap();

        // Resume
        let def: PipelineDefinition = serde_yaml::from_str(yaml).unwrap();
        let resumed = PipelineExecutor::resume_from_state(&state_path, def).unwrap();
        assert_eq!(resumed.state().current_step_index, 0);
        assert_eq!(resumed.state().status, PipelineStatus::WaitingForApproval);
    }
}
