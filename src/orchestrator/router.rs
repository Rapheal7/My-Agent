//! Cost-optimized model router
//!
//! Routes to Free → Cheap → Premium models

/// Task types for model routing
#[derive(Debug, Clone)]
pub enum TaskType {
    Code,
    CodeComplex,
    Research,
    Analysis,
    Reasoning,
    Quick,
    General,
}

impl From<TaskType> for String {
    fn from(t: TaskType) -> Self {
        match t {
            TaskType::Code => "code".to_string(),
            TaskType::CodeComplex => "code_complex".to_string(),
            TaskType::Research => "research".to_string(),
            TaskType::Analysis => "analysis".to_string(),
            TaskType::Reasoning => "reasoning".to_string(),
            TaskType::Quick => "quick".to_string(),
            TaskType::General => "general".to_string(),
        }
    }
}

/// Model configuration
#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub model: String,
    pub max_tokens: u32,
}

/// Cost-optimized model router
pub struct ModelRouter {
    // TODO: Add pricing registry
}

impl ModelRouter {
    pub fn new() -> Self {
        Self {}
    }

    /// Get the best FREE model for a task type
    pub fn get_free_model(&self, task_type: &TaskType) -> ModelConfig {
        match task_type {
            TaskType::Code => ModelConfig {
                model: "openrouter/pony-alpha".to_string(),
                max_tokens: 8192,
            },
            TaskType::CodeComplex => ModelConfig {
                model: "qwen/qwen-2.5-coder-32b-instruct".to_string(),
                max_tokens: 8192,
            },
            TaskType::Research => ModelConfig {
                model: "perplexity/sonar".to_string(),
                max_tokens: 2048,
            },
            TaskType::Analysis => ModelConfig {
                model: "google/gemma-2-9b-it".to_string(),
                max_tokens: 2048,
            },
            TaskType::Reasoning => ModelConfig {
                model: "deepseek/deepseek-r1".to_string(),
                max_tokens: 4096,
            },
            TaskType::Quick => ModelConfig {
                model: "mistralai/mistral-7b-instruct".to_string(),
                max_tokens: 1024,
            },
            TaskType::General => ModelConfig {
                model: "meta-llama/llama-3.1-8b-instruct".to_string(),
                max_tokens: 2048,
            },
        }
    }
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::new()
    }
}
