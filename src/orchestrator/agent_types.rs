//! Typed subagent definitions
//!
//! Defines Claude Code-style typed agents with tool restrictions.

use crate::agent::tools::Tool;

/// Subagent type determines tool access and behavior
#[derive(Debug, Clone, PartialEq)]
pub enum SubagentType {
    /// Read-only codebase exploration (search, read, glob)
    Explore,
    /// Design implementation plans (read-only + analysis)
    Plan,
    /// Shell command execution specialist
    Bash,
    /// Full file operations + shell for writing code
    Coder,
    /// Web research agent
    Researcher,
    /// All tools available
    General,
}

impl SubagentType {
    /// Get the tool names this agent type is allowed to use
    pub fn allowed_tool_names(&self) -> Vec<&str> {
        match self {
            SubagentType::Explore => vec![
                "read_file", "list_directory", "search_content", "find_files",
                "glob", "get_cwd", "file_info",
            ],
            SubagentType::Plan => vec![
                "read_file", "list_directory", "search_content", "find_files",
                "glob", "get_cwd", "file_info",
            ],
            SubagentType::Bash => vec![
                "execute_command", "read_file", "list_directory", "get_cwd",
            ],
            SubagentType::Coder => vec![
                "read_file", "write_file", "append_file", "list_directory",
                "search_content", "find_files", "glob", "get_cwd", "file_info",
                "create_directory", "delete_file", "execute_command",
            ],
            SubagentType::Researcher => vec![
                "fetch_url", "read_file", "list_directory", "search_content",
                "get_cwd",
            ],
            SubagentType::General => vec![
                "read_file", "write_file", "append_file", "list_directory",
                "search_content", "find_files", "glob", "get_cwd", "file_info",
                "create_directory", "delete_file", "execute_command", "fetch_url",
            ],
        }
    }

    /// Get the type-specific system prompt
    pub fn system_prompt(&self) -> &str {
        match self {
            SubagentType::Explore => {
                "You are an Explore agent. Your job is to search and read code to answer questions about a codebase. \
                 Use search_content, find_files, glob, and read_file to navigate. \
                 Report your findings clearly with file paths and line numbers."
            }
            SubagentType::Plan => {
                "You are a Plan agent. Your job is to design implementation approaches. \
                 Read the relevant code, understand the architecture, and produce a step-by-step plan. \
                 Include specific file paths and describe each change needed."
            }
            SubagentType::Bash => {
                "You are a Bash agent. Your job is to execute shell commands to accomplish tasks. \
                 Use execute_command to run commands. Read files to understand context when needed. \
                 Report command output and any errors."
            }
            SubagentType::Coder => {
                "You are a Coder agent. Your job is to write and modify code. \
                 Read existing code first, then make targeted changes using write_file. \
                 Test your changes with execute_command when appropriate. \
                 Keep changes minimal and focused. Write production-quality code with \
                 proper error handling, types, and documentation."
            }
            SubagentType::Researcher => {
                "You are a Researcher agent. Your job is to find information from the web and local files. \
                 Use fetch_url for web research and search_content/read_file for local information. \
                 IMPORTANT: Your output must be a CONCISE SUMMARY (under 2000 words). \
                 Do NOT include raw HTML, full page dumps, or unprocessed content. \
                 Extract only the key facts, best practices, and actionable information. \
                 Structure your summary with bullet points and cite sources by URL."
            }
            SubagentType::General => {
                "You are a General agent with full tool access. Complete the assigned task using \
                 whatever tools are needed. Be thorough and report your results clearly."
            }
        }
    }

    /// Maximum ReAct iterations for this agent type
    pub fn max_iterations(&self) -> usize {
        match self {
            SubagentType::Explore => 20,
            SubagentType::Plan => 25,
            SubagentType::Bash => 15,
            SubagentType::Coder => 25,
            SubagentType::Researcher => 20,
            SubagentType::General => 25,
        }
    }

    /// Filter a full tool list down to only the tools this agent type can use
    pub fn filter_tools(&self, all_tools: Vec<Tool>) -> Vec<Tool> {
        let allowed = self.allowed_tool_names();
        all_tools.into_iter()
            .filter(|t| allowed.contains(&t.name.as_str()))
            .collect()
    }

    /// Map a capability string to the appropriate SubagentType
    pub fn from_capability(capability: &str) -> Self {
        match capability.to_lowercase().as_str() {
            "explore" | "explorer" | "search" | "discover" => SubagentType::Explore,
            "plan" | "planner" | "architect" => SubagentType::Plan,
            "bash" | "shell" | "command" => SubagentType::Bash,
            "code" | "coder" | "developer" | "programmer" => SubagentType::Coder,
            "research" | "researcher" | "web" => SubagentType::Researcher,
            _ => SubagentType::General,
        }
    }

    /// Get the best model for this agent type from config
    pub fn preferred_model(&self, config: &crate::config::Config) -> String {
        match self {
            SubagentType::Coder => config.models.code.clone(),
            SubagentType::Researcher => config.models.research.clone(),
            SubagentType::Plan => config.models.reasoning.clone(),
            SubagentType::Explore => config.models.utility.clone(),
            SubagentType::Bash => config.models.utility.clone(),
            SubagentType::General => config.models.code.clone(),
        }
    }

    /// Display name for this agent type
    pub fn display_name(&self) -> &str {
        match self {
            SubagentType::Explore => "Explore",
            SubagentType::Plan => "Plan",
            SubagentType::Bash => "Bash",
            SubagentType::Coder => "Coder",
            SubagentType::Researcher => "Researcher",
            SubagentType::General => "General",
        }
    }
}
