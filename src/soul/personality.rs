//! Soul Personality Configuration
//!
//! Defines the agent's personality, behavior, and response style.
//! Loaded from a personality file (TOML/JSON) or defaults.

use serde::{Deserialize, Serialize};
use anyhow::Result;
use std::path::PathBuf;
use std::fs;

/// Personality configuration for the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Personality {
    /// Agent's name
    pub name: String,
    /// Core personality traits
    pub traits: Vec<String>,
    /// Communication style
    pub style: CommunicationStyle,
    /// System prompt template
    pub system_prompt: String,
    /// Greeting message
    pub greeting: Option<String>,
    /// Farewell message
    pub farewell: Option<String>,
    /// Behavioral rules
    pub rules: Vec<BehaviorRule>,
    /// Skills the agent prefers to use
    pub preferred_skills: Vec<String>,
    /// How the agent responds to different task types
    pub task_responses: TaskResponses,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunicationStyle {
    /// Formality level: casual, neutral, formal
    pub formality: String,
    /// Response length: brief, medium, detailed
    pub length: String,
    /// Use emojis
    pub emojis: bool,
    /// Show thinking process
    pub show_thinking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorRule {
    /// Rule name
    pub name: String,
    /// When to apply
    pub trigger: String,
    /// What to do
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResponses {
    pub code: TaskResponse,
    pub research: TaskResponse,
    pub exploration: TaskResponse,
    pub reasoning: TaskResponse,
    pub general: TaskResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResponse {
    /// Approach description
    pub approach: String,
    /// Tools to prefer
    pub preferred_tools: Vec<String>,
}

impl Default for Personality {
    fn default() -> Self {
        Self {
            name: "My Agent".to_string(),
            traits: vec![
                "helpful".to_string(),
                "concise".to_string(),
                "proactive".to_string(),
            ],
            style: CommunicationStyle {
                formality: "neutral".to_string(),
                length: "medium".to_string(),
                emojis: false,
                show_thinking: false,
            },
            system_prompt: "You are a helpful AI assistant. Be concise and friendly. \
                When working with files or code, use the available tools. \
                For complex tasks, coordinate with specialized agents.".to_string(),
            greeting: Some("Hello! How can I help you today?".to_string()),
            farewell: Some("Goodbye! Have a great day!".to_string()),
            rules: vec![
                BehaviorRule {
                    name: "use_tools".to_string(),
                    trigger: "file or code operations".to_string(),
                    action: "use available tools instead of guessing".to_string(),
                },
                BehaviorRule {
                    name: "orchestrate_complex".to_string(),
                    trigger: "multi-step or complex tasks".to_string(),
                    action: "suggest spawning specialized agents".to_string(),
                },
            ],
            preferred_skills: vec![
                "file_search".to_string(),
                "code_analysis".to_string(),
            ],
            task_responses: TaskResponses {
                code: TaskResponse {
                    approach: "Write clean, well-documented code. Follow best practices.".to_string(),
                    preferred_tools: vec!["search_content".to_string(), "read_file".to_string()],
                },
                research: TaskResponse {
                    approach: "Search thoroughly, cite sources, summarize findings.".to_string(),
                    preferred_tools: vec!["web_search".to_string()],
                },
                exploration: TaskResponse {
                    approach: "Start broad, then narrow down. Provide file lists and summaries.".to_string(),
                    preferred_tools: vec!["search_content".to_string(), "glob".to_string()],
                },
                reasoning: TaskResponse {
                    approach: "Think step by step. Show logical progression.".to_string(),
                    preferred_tools: vec![],
                },
                general: TaskResponse {
                    approach: "Be helpful and direct. Answer questions clearly.".to_string(),
                    preferred_tools: vec![],
                },
            },
        }
    }
}

impl Personality {
    /// Load personality from file or return default
    pub fn load() -> Result<Self> {
        let path = Self::config_path();

        if path.exists() {
            let content = fs::read_to_string(&path)?;
            if path.extension().map(|e| e == "toml").unwrap_or(false) {
                let personality: Personality = toml::from_str(&content)?;
                return Ok(personality);
            }
            // Try JSON
            let personality: Personality = serde_json::from_str(&content)?;
            Ok(personality)
        } else {
            // Create default personality file
            let personality = Self::default();
            personality.save()?;
            Ok(personality)
        }
    }

    /// Save personality to file
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }

    /// Get the config file path
    fn config_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("my-agent")
            .join("soul")
            .join("personality.toml")
    }

    /// Get the system prompt for a specific mode
    pub fn get_system_prompt(&self, mode: &str) -> String {
        use super::system_prompts::{get_main_system_prompt, get_mode_prompt};

        let main_prompt = get_main_system_prompt();
        let mode_prompt = get_mode_prompt(mode);

        format!("{}\n\n{}", main_prompt.trim(), mode_prompt)
    }

    /// Get greeting message
    pub fn get_greeting(&self) -> String {
        self.greeting.clone().unwrap_or_else(|| format!("{} ready to help!", self.name))
    }

    /// Get farewell message
    pub fn get_farewell(&self) -> String {
        self.farewell.clone().unwrap_or_else(|| "Goodbye!".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_personality() {
        let p = Personality::default();
        assert_eq!(p.name, "My Agent");
        assert!(!p.traits.is_empty());
    }

    #[test]
    fn test_system_prompt() {
        let p = Personality::default();
        let prompt = p.get_system_prompt("tools");
        assert!(prompt.contains("tools"));
    }
}