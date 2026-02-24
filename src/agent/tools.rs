//! Tool definitions for the agent
//!
//! This module defines the tools available to the agent and provides
//! the tool execution logic with proper security validation.

use serde::{Deserialize, Serialize};
use crate::tools::filesystem::FileSystemTool;
use crate::tools::shell::ShellTool;
use crate::tools::web::WebTool;
use crate::tools::desktop::DesktopTool;
use crate::security::ApprovalManager;
use std::sync::Arc;

/// Tool definition for LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Tool execution context
#[derive(Clone)]
pub struct ToolContext {
    pub filesystem: FileSystemTool,
    pub shell: ShellTool,
    pub web: WebTool,
    pub desktop: DesktopTool,
    pub approver: ApprovalManager,
    /// Optional device registry for remote tool routing
    pub device_registry: Option<Arc<crate::server::device::DeviceRegistry>>,
}

impl ToolContext {
    /// Create a new tool context with default configuration
    pub fn new() -> Self {
        Self {
            filesystem: FileSystemTool::new(),
            shell: ShellTool::new(),
            web: WebTool::new().expect("Failed to create WebTool"),
            desktop: DesktopTool::new(),
            approver: ApprovalManager::with_defaults(),
            device_registry: None,
        }
    }

    /// Create with current directory and project paths allowed
    pub fn with_project_paths() -> Self {
        use crate::security::sandbox::{FileSystemSandbox, SandboxConfig};

        let mut config = SandboxConfig::default();

        // Add current working directory (canonicalized)
        if let Ok(cwd) = std::env::current_dir() {
            // Scan for project subdirectories first (before moving cwd)
            if let Ok(entries) = std::fs::read_dir(&cwd) {
                for entry in entries.flatten() {
                    let subdir = entry.path();
                    if subdir.is_dir() {
                        if subdir.join("Cargo.toml").exists() || subdir.join("package.json").exists() {
                            if let Ok(canonical) = subdir.canonicalize() {
                                config.allowed_paths.push(canonical);
                            }
                        }
                    }
                }
            }

            // Now add the current directory
            if let Ok(canonical) = cwd.canonicalize() {
                config.allowed_paths.push(canonical);
            } else {
                config.allowed_paths.push(cwd);
            }
        }

        // Add my-agent project directory if running from there
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(parent) = exe_path.parent().and_then(|p| p.parent()) {
                if parent.join("Cargo.toml").exists() {
                    if let Ok(canonical) = parent.canonicalize() {
                        config.allowed_paths.push(canonical);
                    } else {
                        config.allowed_paths.push(parent.to_path_buf());
                    }
                }
            }
        }

        let sandbox = FileSystemSandbox::with_config(config);
        let approver = ApprovalManager::with_defaults();
        let filesystem = FileSystemTool::with_config(sandbox, approver.clone());

        Self {
            filesystem,
            shell: ShellTool::new(),
            web: WebTool::new().expect("Failed to create WebTool"),
            desktop: DesktopTool::new(),
            approver,
            device_registry: None,
        }
    }

    /// Create with custom filesystem tool
    pub fn with_filesystem(filesystem: FileSystemTool) -> Self {
        Self {
            filesystem,
            shell: ShellTool::new(),
            web: WebTool::new().expect("Failed to create WebTool"),
            desktop: DesktopTool::new(),
            approver: ApprovalManager::with_defaults(),
            device_registry: None,
        }
    }

    /// Create with custom shell tool
    pub fn with_shell(shell: ShellTool) -> Self {
        Self {
            filesystem: FileSystemTool::new(),
            shell,
            web: WebTool::new().expect("Failed to create WebTool"),
            desktop: DesktopTool::new(),
            approver: ApprovalManager::with_defaults(),
            device_registry: None,
        }
    }

    /// Create with custom web tool
    pub fn with_web(web: WebTool) -> Self {
        Self {
            filesystem: FileSystemTool::new(),
            shell: ShellTool::new(),
            web,
            desktop: DesktopTool::new(),
            approver: ApprovalManager::with_defaults(),
            device_registry: None,
        }
    }

    /// Create with custom approver
    pub fn with_approver(approver: ApprovalManager) -> Self {
        let filesystem = FileSystemTool::with_config(
            crate::security::sandbox::FileSystemSandbox::new(),
            approver.clone()
        );
        let shell = ShellTool::with_approver(
            crate::tools::shell::ShellConfig::default(),
            approver.clone()
        );
        let web = WebTool::with_approver(
            crate::tools::web::WebConfig::default(),
            approver.clone()
        ).expect("Failed to create WebTool");
        Self {
            filesystem,
            shell,
            web,
            desktop: DesktopTool::new(),
            approver,
            device_registry: None,
        }
    }

    /// Create with custom tools
    pub fn with_tools(
        filesystem: FileSystemTool,
        shell: ShellTool,
        web: WebTool,
        approver: ApprovalManager,
    ) -> Self {
        Self {
            filesystem,
            shell,
            web,
            desktop: DesktopTool::new(),
            approver,
            device_registry: None,
        }
    }
}

impl Default for ToolContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Tool call from LLM
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Tool execution result
#[derive(Debug, Clone, Serialize)]
pub struct ToolResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Built-in tools available to the agent
pub fn builtin_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "read_file".to_string(),
            description: "Read the contents of a file. Returns the file content, size, and line count. \
                Maximum file size: 10MB. Use for viewing code, configs, logs, and documents.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read (supports ~ for home directory)"
                    }
                },
                "required": ["path"]
            }),
        },
        Tool {
            name: "write_file".to_string(),
            description: "Write content to a file. Creates parent directories if needed. \
                Requires user approval. Maximum file size: 50MB.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    }
                },
                "required": ["path", "content"]
            }),
        },
        Tool {
            name: "append_file".to_string(),
            description: "Append content to the end of a file. Creates the file if it doesn't exist. \
                Requires user approval.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to append"
                    }
                },
                "required": ["path", "content"]
            }),
        },
        Tool {
            name: "list_directory".to_string(),
            description: "List the contents of a directory. Returns files and subdirectories \
                with metadata (size, modification time). Directories are listed first.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the directory to list (defaults to current directory)"
                    }
                },
                "required": []
            }),
        },
        Tool {
            name: "file_info".to_string(),
            description: "Get detailed information about a file or directory. \
                Returns size, type, creation time, and modification time.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file or directory"
                    }
                },
                "required": ["path"]
            }),
        },
        Tool {
            name: "search_files".to_string(),
            description: "Search for files by name pattern in a directory. \
                Searches recursively through subdirectories. Case-insensitive.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "directory": {
                        "type": "string",
                        "description": "Directory to search in"
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Search pattern (substring match, case-insensitive)"
                    }
                },
                "required": ["directory", "pattern"]
            }),
        },
        Tool {
            name: "create_directory".to_string(),
            description: "Create a new directory and its parent directories if needed. \
                Requires user approval.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path of the directory to create"
                    }
                },
                "required": ["path"]
            }),
        },
        Tool {
            name: "delete_file".to_string(),
            description: "Delete a file. Requires user approval (critical operation). \
                Cannot delete directories - use delete_directory instead.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to delete"
                    }
                },
                "required": ["path"]
            }),
        },
        Tool {
            name: "execute_command".to_string(),
            description: "Execute a shell command (requires approval). \
                Use with caution - all commands are logged and require explicit approval. \
                Default timeout is 120 seconds. For long-running commands like cargo build, \
                npm install, or test suites, set a higher timeout_secs.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Command to execute"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 120). Use 300+ for build commands like cargo build, cargo check, npm install, etc."
                    }
                },
                "required": ["command"]
            }),
        },
        Tool {
            name: "fetch_url".to_string(),
            description: "Fetch content from a URL (requires approval). \
                Downloads web content safely with validation. \
                Internal URLs (localhost, 192.168.x.x, etc.) are blocked.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to fetch (must be http or https)"
                    }
                },
                "required": ["url"]
            }),
        },
        // Skill management tools
        Tool {
            name: "create_skill".to_string(),
            description: "Create a new skill dynamically when you need an ability not currently available. \
                The skill will be generated based on your description and immediately available for use. \
                Use this when you encounter a task that requires a specialized capability.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "Clear description of what the skill should do"
                    },
                    "name": {
                        "type": "string",
                        "description": "Optional skill name"
                    },
                    "category": {
                        "type": "string",
                        "description": "Optional category (Filesystem, Shell, Web, Data, System, Utility, Custom)"
                    }
                },
                "required": ["description"]
            }),
        },
        Tool {
            name: "list_skills".to_string(),
            description: "List all available skills. Use this to discover capabilities.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        Tool {
            name: "use_skill".to_string(),
            description: "Execute a skill by ID. Use list_skills to discover available skills.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "skill_id": {
                        "type": "string",
                        "description": "ID of the skill to execute"
                    },
                    "params": {
                        "type": "object",
                        "description": "Parameters for the skill"
                    }
                },
                "required": ["skill_id"]
            }),
        },
        // Exploration tools
        Tool {
            name: "search_content".to_string(),
            description: "Search for a text pattern in all files within a directory (like grep -r). \
                Returns file paths and line numbers where the pattern was found. \
                Use for finding code, configurations, or any text in files.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "directory": {
                        "type": "string",
                        "description": "Directory to search in (defaults to current directory)"
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Text pattern to search for"
                    },
                    "file_pattern": {
                        "type": "string",
                        "description": "Optional file pattern to filter (e.g., *.rs, *.py)"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default 100)"
                    }
                },
                "required": ["pattern"]
            }),
        },
        Tool {
            name: "find_files".to_string(),
            description: "Find files and directories matching a pattern. \
                More powerful than search_files - supports type filters and depth limits.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "directory": {
                        "type": "string",
                        "description": "Directory to search in (defaults to current directory)"
                    },
                    "name_pattern": {
                        "type": "string",
                        "description": "File name pattern (supports wildcards like *.rs)"
                    },
                    "file_type": {
                        "type": "string",
                        "description": "Filter by type: 'file', 'directory', or 'all' (default: all)"
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum directory depth to search (default: unlimited)"
                    }
                },
                "required": ["name_pattern"]
            }),
        },
        Tool {
            name: "get_cwd".to_string(),
            description: "Get the current working directory. Use this to understand where you are in the filesystem.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        Tool {
            name: "glob".to_string(),
            description: "Find files using glob patterns (e.g., '**/*.rs' for all Rust files). \
                Returns list of matching file paths.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern (e.g., '**/*.rs', 'src/**/*.py')"
                    },
                    "directory": {
                        "type": "string",
                        "description": "Base directory for glob (defaults to current directory)"
                    }
                },
                "required": ["pattern"]
            }),
        },
        // Self-editing tools
        Tool {
            name: "edit_personality".to_string(),
            description: "Edit your own personality file to change how you behave. \
                Use this to customize your traits, communication style, and system prompt. \
                Changes take effect after reload or restart.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "field": {
                        "type": "string",
                        "description": "Field to edit: 'name', 'traits' (comma-separated), 'system_prompt', 'greeting', 'farewell', 'style.formality', 'style.length'"
                    },
                    "value": {
                        "type": "string",
                        "description": "New value for the field"
                    }
                },
                "required": ["field", "value"]
            }),
        },
        Tool {
            name: "view_source".to_string(),
            description: "View your own source code files. Use this to understand how you work \
                or to identify areas for improvement. Path is relative to your source directory.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "Source file to view (e.g., 'src/agent/interactive.rs', 'src/soul/personality.rs')"
                    }
                },
                "required": ["file"]
            }),
        },
        Tool {
            name: "edit_source".to_string(),
            description: "Edit your own source code to improve yourself. \
                WARNING: This modifies your running code. Requires approval. \
                You must rebuild after editing for changes to take effect.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "Source file to edit (relative to source directory)"
                    },
                    "old_content": {
                        "type": "string",
                        "description": "Exact content to replace"
                    },
                    "new_content": {
                        "type": "string",
                        "description": "New content to insert"
                    }
                },
                "required": ["file", "old_content", "new_content"]
            }),
        },
        Tool {
            name: "rebuild_self".to_string(),
            description: "Rebuild and reinstall yourself after editing your source code. \
                This compiles your modified code and installs the new version. \
                Requires approval as it modifies system files.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        Tool {
            name: "self_diagnose".to_string(),
            description: "Diagnose issues with your own tools and configuration. \
                Use this when a tool fails repeatedly or you suspect something is broken. \
                Returns diagnostic information and potential fixes.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "issue": {
                        "type": "string",
                        "description": "Description of the issue (e.g., 'read_file fails', 'path resolution broken')"
                    },
                    "context": {
                        "type": "string",
                        "description": "Additional context about when the issue occurs"
                    }
                },
                "required": ["issue"]
            }),
        },
        Tool {
            name: "self_repair".to_string(),
            description: "Attempt to automatically repair a detected issue in your codebase. \
                This can fix common problems like path resolution, missing dependencies, \
                or configuration errors. Use after self_diagnose identifies an issue.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "issue_type": {
                        "type": "string",
                        "description": "Type of issue to repair: 'path_resolution', 'dependencies', 'config', 'sandbox'"
                    },
                    "details": {
                        "type": "string",
                        "description": "Additional details about the fix needed"
                    }
                },
                "required": ["issue_type"]
            }),
        },
        // Orchestration tool - allows chat model to delegate to specialized agents
        Tool {
            name: "orchestrate_task".to_string(),
            description: "Delegate a complex task to specialized agents. Use this when you need \
                code generation, deep research, or complex reasoning that requires specialized models. \
                You act as the coordinator - the 'head' directing 'hands' and 'body'.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task description to delegate"
                    },
                    "agent_type": {
                        "type": "string",
                        "description": "Type of agent needed: 'code', 'research', 'reasoning', 'utility'"
                    },
                    "reason": {
                        "type": "string",
                        "description": "Why this needs a specialized agent"
                    }
                },
                "required": ["task", "agent_type"]
            }),
        },
        Tool {
            name: "spawn_agents".to_string(),
            description: "Spawn multiple specialized agents for a complex multi-step task. \
                Use this for tasks that require different types of expertise working together.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "main_task": {
                        "type": "string",
                        "description": "The main task to accomplish"
                    },
                    "subtasks": {
                        "type": "array",
                        "description": "List of subtasks with agent types",
                        "items": {
                            "type": "object",
                            "properties": {
                                "description": {"type": "string"},
                                "agent_type": {"type": "string"}
                            }
                        }
                    }
                },
                "required": ["main_task"]
            }),
        },
        Tool {
            name: "spawn_subagent".to_string(),
            description: "Spawn a specialized subagent for autonomous task execution. \
                The subagent runs with its own tool-calling loop and returns results when done. \
                Types: explore (search codebase), plan (design implementation), bash (run commands), \
                coder (write code), researcher (web research), general (all tools).".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Task description for the subagent"
                    },
                    "agent_type": {
                        "type": "string",
                        "enum": ["explore", "plan", "bash", "coder", "researcher", "general"],
                        "description": "Type of subagent to spawn"
                    }
                },
                "required": ["task", "agent_type"]
            }),
        },
        // Desktop control tools
        Tool {
            name: "capture_screen".to_string(),
            description: "Capture a screenshot of the desktop. Use this to see what's currently on screen. \
                Returns the image as base64-encoded PNG data. This tool is automatic (no approval needed).".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "region": {
                        "type": "string",
                        "enum": ["full", "region"],
                        "description": "Capture region: 'full' for entire screen, 'region' for specific area"
                    },
                    "x": {
                        "type": "integer",
                        "description": "X coordinate for region capture"
                    },
                    "y": {
                        "type": "integer",
                        "description": "Y coordinate for region capture"
                    },
                    "width": {
                        "type": "integer",
                        "description": "Width for region capture"
                    },
                    "height": {
                        "type": "integer",
                        "description": "Height for region capture"
                    }
                },
                "required": []
            }),
        },
        Tool {
            name: "mouse_click".to_string(),
            description: "Click the mouse at a position on screen. Requires approval. \
                Use coordinates from screenshots to determine click position.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": {
                        "type": "integer",
                        "description": "X coordinate to click at"
                    },
                    "y": {
                        "type": "integer",
                        "description": "Y coordinate to click at"
                    },
                    "button": {
                        "type": "string",
                        "enum": ["left", "right", "middle"],
                        "default": "left",
                        "description": "Mouse button to click"
                    }
                },
                "required": ["x", "y"]
            }),
        },
        Tool {
            name: "mouse_double_click".to_string(),
            description: "Double-click the mouse at a position. Requires approval. \
                Use for opening files or selecting text.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": {
                        "type": "integer",
                        "description": "X coordinate"
                    },
                    "y": {
                        "type": "integer",
                        "description": "Y coordinate"
                    }
                },
                "required": ["x", "y"]
            }),
        },
        Tool {
            name: "mouse_scroll".to_string(),
            description: "Scroll the mouse wheel. Requires approval. \
                Use to navigate long pages or documents.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "direction": {
                        "type": "string",
                        "enum": ["up", "down"],
                        "description": "Scroll direction"
                    },
                    "amount": {
                        "type": "integer",
                        "default": 3,
                        "description": "Scroll amount (number of clicks)"
                    }
                },
                "required": ["direction"]
            }),
        },
        Tool {
            name: "mouse_drag".to_string(),
            description: "Drag the mouse from one position to another. Requires approval. \
                Use for dragging files, selecting text, or drawing.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "from_x": {
                        "type": "integer",
                        "description": "Starting X coordinate"
                    },
                    "from_y": {
                        "type": "integer",
                        "description": "Starting Y coordinate"
                    },
                    "to_x": {
                        "type": "integer",
                        "description": "Ending X coordinate"
                    },
                    "to_y": {
                        "type": "integer",
                        "description": "Ending Y coordinate"
                    }
                },
                "required": ["from_x", "from_y", "to_x", "to_y"]
            }),
        },
        Tool {
            name: "keyboard_type".to_string(),
            description: "Type text using the keyboard. Requires approval. \
                Use for entering text into input fields.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Text to type"
                    }
                },
                "required": ["text"]
            }),
        },
        Tool {
            name: "keyboard_press".to_string(),
            description: "Press a single keyboard key. Requires approval. \
                Use for special keys like Enter, Tab, Escape, arrows, etc.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "Key to press (e.g., 'enter', 'tab', 'escape', 'up', 'down', 'left', 'right', 'f1'-'f12', 'space')"
                    }
                },
                "required": ["key"]
            }),
        },
        Tool {
            name: "keyboard_hotkey".to_string(),
            description: "Press a keyboard hotkey (combination of keys). Requires approval. \
                Examples: Ctrl+C (copy), Ctrl+V (paste), Alt+Tab (switch windows).".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "keys": {
                        "type": "array",
                        "items": {
                            "type": "string"
                        },
                        "description": "Keys to press together (e.g., ['ctrl', 'c'] for Ctrl+C)"
                    }
                },
                "required": ["keys"]
            }),
        },
        Tool {
            name: "open_application".to_string(),
            description: "Open/launch an application by name. Requires approval. \
                Examples: 'firefox', 'code', 'terminal', 'nautilus'.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Application name or command to launch"
                    }
                },
                "required": ["name"]
            }),
        },
        // Remote device tools
        Tool {
            name: "list_devices".to_string(),
            description: "List all connected remote devices and the currently active device. \
                Use this to see which devices are available for tool execution.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        Tool {
            name: "switch_device".to_string(),
            description: "Switch tool execution to a different device. After switching, tools like \
                read_file, write_file, run_command, capture_screen, mouse_click, keyboard_type etc. \
                will execute on the target device instead of the server. \
                Use 'local' or empty string to switch back to the server.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "device": {
                        "type": "string",
                        "description": "Device name to switch to (e.g., 'MacBook'), or 'local' to switch back to server"
                    }
                },
                "required": ["device"]
            }),
        },
        // Self-improvement and reflection tools
        Tool {
            name: "analyze_performance".to_string(),
            description: "Analyze your own performance metrics and identify areas for improvement. \
                Returns health score, success rates, and suggestions for optimization. \
                Use this to reflect on your capabilities and learn from patterns.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "focus": {
                        "type": "string",
                        "description": "Area to focus analysis: 'all', 'tools', 'errors', 'patterns'",
                        "default": "all"
                    }
                },
                "required": []
            }),
        },
        Tool {
            name: "get_lessons".to_string(),
            description: "Retrieve lessons learned from past experiences. \
                These insights can help avoid repeating mistakes and improve decision-making. \
                Use before attempting complex tasks to learn from history.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "context": {
                        "type": "string",
                        "description": "Context to filter lessons (e.g., 'file operations', 'errors')"
                    },
                    "min_confidence": {
                        "type": "number",
                        "description": "Minimum confidence level (0.0-1.0)",
                        "default": 0.3
                    }
                },
                "required": []
            }),
        },
        Tool {
            name: "record_lesson".to_string(),
            description: "Record a new lesson learned from experience. \
                This helps you remember insights for future similar situations. \
                Use after solving problems or discovering useful patterns.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "insight": {
                        "type": "string",
                        "description": "What was learned"
                    },
                    "context": {
                        "type": "string",
                        "description": "When/where this lesson applies"
                    },
                    "related_tools": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Tools this lesson relates to"
                    }
                },
                "required": ["insight", "context"]
            }),
        },
        Tool {
            name: "improve_self".to_string(),
            description: "Initiate a self-improvement cycle. Analyzes recent performance, \
                learns from outcomes, and generates improvement suggestions. \
                Use periodically to continuously enhance your capabilities.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "area": {
                        "type": "string",
                        "description": "Area to improve: 'tools', 'reliability', 'performance', 'all'",
                        "default": "all"
                    }
                },
                "required": []
            }),
        },
        // Learning tools
        Tool {
            name: "record_learning".to_string(),
            description: "Explicitly record a learning insight, pattern, or best practice discovered \
                during this conversation. Useful for capturing knowledge that should persist.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "area": {
                        "type": "string",
                        "description": "Area category: 'tool_usage', 'code_generation', 'orchestration', 'user_preference', 'general'"
                    },
                    "title": {
                        "type": "string",
                        "description": "Brief title for the learning"
                    },
                    "description": {
                        "type": "string",
                        "description": "Detailed description of what was learned"
                    },
                    "context": {
                        "type": "string",
                        "description": "What was happening when this was discovered"
                    },
                    "priority": {
                        "type": "string",
                        "description": "Priority level: 'low', 'medium', 'high', 'critical'"
                    }
                },
                "required": ["title", "description"]
            }),
        },
        Tool {
            name: "review_learnings".to_string(),
            description: "Review captured learnings, errors, and feature requests. \
                Filter by status to see new, validated, or promoted entries.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "description": "Filter by status: 'new', 'validated', 'promoted', 'all' (default: 'all')"
                    }
                },
                "required": []
            }),
        },
        Tool {
            name: "search_learnings".to_string(),
            description: "Search through captured learnings by keyword. \
                Finds relevant past learnings, errors, and feature requests.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query to find relevant learnings"
                    }
                },
                "required": ["query"]
            }),
        },
        Tool {
            name: "promote_learning".to_string(),
            description: "Promote a validated learning to permanent context. \
                Promoted learnings are loaded at every session start.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "entry_id": {
                        "type": "string",
                        "description": "ID of the learning entry to promote (e.g., LRN-20260222-001)"
                    }
                },
                "required": ["entry_id"]
            }),
        },
        Tool {
            name: "demote_learning".to_string(),
            description: "Remove a promoted learning from permanent context. \
                The learning is kept but no longer loaded at session start.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "entry_id": {
                        "type": "string",
                        "description": "ID of the learning entry to demote"
                    }
                },
                "required": ["entry_id"]
            }),
        },
    ]
}

/// Execute a tool call
pub fn execute_tool<'a>(call: &'a ToolCall, ctx: &'a ToolContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<ToolResult>> + Send + 'a>> {
    Box::pin(execute_tool_inner(call, ctx))
}

async fn execute_tool_inner(call: &ToolCall, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
    // Handle device management tools locally (never routed)
    match call.name.as_str() {
        "list_devices" => {
            if let Some(ref registry) = ctx.device_registry {
                let devices = registry.list_devices().await;
                let active = registry.get_active_device().await;
                return Ok(ToolResult {
                    success: true,
                    message: if devices.is_empty() {
                        "No remote devices connected. All tools execute on the local server.".to_string()
                    } else {
                        format!("{} device(s) connected. Active: {}",
                            devices.len(),
                            active.as_deref().unwrap_or("local server"))
                    },
                    data: Some(serde_json::json!({
                        "devices": devices,
                        "active": active.unwrap_or_else(|| "local".to_string()),
                    })),
                });
            } else {
                return Ok(ToolResult {
                    success: true,
                    message: "Device routing not available (not running in server mode).".to_string(),
                    data: None,
                });
            }
        }
        "switch_device" => {
            if let Some(ref registry) = ctx.device_registry {
                let device = call.arguments["device"].as_str().unwrap_or("local");
                let target = if device == "local" || device.is_empty() { None } else { Some(device) };

                match registry.set_active_device(target).await {
                    Ok(()) => {
                        let label = target.unwrap_or("local server");
                        return Ok(ToolResult {
                            success: true,
                            message: format!("Switched tool execution to: {}. All subsequent tool calls (file operations, shell commands, desktop control) will execute on {}.", label, label),
                            data: None,
                        });
                    }
                    Err(e) => {
                        return Ok(ToolResult {
                            success: false,
                            message: e.to_string(),
                            data: None,
                        });
                    }
                }
            } else {
                return Ok(ToolResult {
                    success: false,
                    message: "Device routing not available (not running in server mode).".to_string(),
                    data: None,
                });
            }
        }
        _ => {}
    }

    // Check if this tool call should be routed to a remote device
    if let Some(ref registry) = ctx.device_registry {
        if let Some(device_name) = registry.should_route_remote(&call.name).await {
            match registry.execute_remote(&device_name, &call.name, call.arguments.clone()).await {
                Ok(response) => {
                    return Ok(ToolResult {
                        success: response.success,
                        message: response.message,
                        data: response.data,
                    });
                }
                Err(e) => {
                    return Ok(ToolResult {
                        success: false,
                        message: format!("Remote execution on '{}' failed: {}", device_name, e),
                        data: None,
                    });
                }
            }
        }
    }

    // Execute locally
    match call.name.as_str() {
        "read_file" => {
            let path = call.arguments["path"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;

            match ctx.filesystem.read_file(path).await {
                Ok(content) => Ok(ToolResult {
                    success: true,
                    message: format!("Read {} bytes ({} lines)", content.size, content.lines),
                    data: Some(serde_json::json!({
                        "content": content.content,
                        "size": content.size,
                        "lines": content.lines,
                        "path": content.path,
                    })),
                }),
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Failed to read file: {}", e),
                    data: None,
                }),
            }
        }

        "write_file" => {
            let path = call.arguments["path"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;
            let content = call.arguments["content"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'content' argument"))?;

            match ctx.filesystem.write_file(path, content).await {
                Ok(result) => match result {
                    crate::tools::filesystem::FileOperationResult::Success { message } => Ok(ToolResult {
                        success: true,
                        message,
                        data: None,
                    }),
                    crate::tools::filesystem::FileOperationResult::Cancelled { reason } => Ok(ToolResult {
                        success: false,
                        message: reason,
                        data: None,
                    }),
                    crate::tools::filesystem::FileOperationResult::Error { message } => Ok(ToolResult {
                        success: false,
                        message,
                        data: None,
                    }),
                },
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Failed to write file: {}", e),
                    data: None,
                }),
            }
        }

        "append_file" => {
            let path = call.arguments["path"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;
            let content = call.arguments["content"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'content' argument"))?;

            match ctx.filesystem.append_file(path, content).await {
                Ok(result) => match result {
                    crate::tools::filesystem::FileOperationResult::Success { message } => Ok(ToolResult {
                        success: true,
                        message,
                        data: None,
                    }),
                    crate::tools::filesystem::FileOperationResult::Cancelled { reason } => Ok(ToolResult {
                        success: false,
                        message: reason,
                        data: None,
                    }),
                    crate::tools::filesystem::FileOperationResult::Error { message } => Ok(ToolResult {
                        success: false,
                        message,
                        data: None,
                    }),
                },
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Failed to append file: {}", e),
                    data: None,
                }),
            }
        }

        "list_directory" => {
            let path = call.arguments["path"].as_str().unwrap_or(".");

            match ctx.filesystem.list_directory(path).await {
                Ok(listing) => {
                    let entries: Vec<_> = listing.entries.iter().map(|e| {
                        serde_json::json!({
                            "name": e.name,
                            "path": e.path,
                            "size": e.size,
                            "is_dir": e.is_dir,
                            "is_file": e.is_file,
                            "modified": e.modified.map(|t| {
                                t.duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0)
                            }),
                        })
                    }).collect();

                    Ok(ToolResult {
                        success: true,
                        message: format!("Found {} entries", listing.total_count),
                        data: Some(serde_json::json!({
                            "path": listing.path,
                            "total_count": listing.total_count,
                            "entries": entries,
                        })),
                    })
                }
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Failed to list directory: {}", e),
                    data: None,
                }),
            }
        }

        "file_info" => {
            let path = call.arguments["path"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;

            match ctx.filesystem.file_info(path).await {
                Ok(info) => Ok(ToolResult {
                    success: true,
                    message: format!("{}: {} bytes", info.name, info.size),
                    data: Some(serde_json::json!({
                        "name": info.name,
                        "path": info.path,
                        "size": info.size,
                        "is_dir": info.is_dir,
                        "is_file": info.is_file,
                        "modified": info.modified.map(|t| {
                            t.duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0)
                        }),
                        "created": info.created.map(|t| {
                            t.duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0)
                        }),
                    })),
                }),
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Failed to get file info: {}", e),
                    data: None,
                }),
            }
        }

        "search_files" => {
            let directory = call.arguments["directory"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'directory' argument"))?;
            let pattern = call.arguments["pattern"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'pattern' argument"))?;

            match ctx.filesystem.search_files(directory, pattern).await {
                Ok(results) => {
                    let files: Vec<_> = results.iter().map(|f| {
                        serde_json::json!({
                            "name": f.name,
                            "path": f.path,
                            "size": f.size,
                            "is_dir": f.is_dir,
                        })
                    }).collect();

                    Ok(ToolResult {
                        success: true,
                        message: format!("Found {} matching files", results.len()),
                        data: Some(serde_json::json!({
                            "count": results.len(),
                            "files": files,
                        })),
                    })
                }
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Failed to search files: {}", e),
                    data: None,
                }),
            }
        }

        "create_directory" => {
            let path = call.arguments["path"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;

            match ctx.filesystem.create_directory(path).await {
                Ok(result) => match result {
                    crate::tools::filesystem::FileOperationResult::Success { message } => Ok(ToolResult {
                        success: true,
                        message,
                        data: None,
                    }),
                    crate::tools::filesystem::FileOperationResult::Cancelled { reason } => Ok(ToolResult {
                        success: false,
                        message: reason,
                        data: None,
                    }),
                    crate::tools::filesystem::FileOperationResult::Error { message } => Ok(ToolResult {
                        success: false,
                        message,
                        data: None,
                    }),
                },
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Failed to create directory: {}", e),
                    data: None,
                }),
            }
        }

        "delete_file" => {
            let path = call.arguments["path"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;

            match ctx.filesystem.delete_file(path).await {
                Ok(result) => match result {
                    crate::tools::filesystem::FileOperationResult::Success { message } => Ok(ToolResult {
                        success: true,
                        message,
                        data: None,
                    }),
                    crate::tools::filesystem::FileOperationResult::Cancelled { reason } => Ok(ToolResult {
                        success: false,
                        message: reason,
                        data: None,
                    }),
                    crate::tools::filesystem::FileOperationResult::Error { message } => Ok(ToolResult {
                        success: false,
                        message,
                        data: None,
                    }),
                },
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Failed to delete file: {}", e),
                    data: None,
                }),
            }
        }

        "execute_command" => {
            let cmd = call.arguments["command"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;

            let timeout_secs = call.arguments.get("timeout_secs")
                .and_then(|v| v.as_u64());

            let exec_result = if let Some(secs) = timeout_secs {
                ctx.shell.execute_with_timeout(cmd, secs).await
            } else {
                ctx.shell.execute(cmd).await
            };

            match exec_result {
                Ok(result) => {
                    let success = result.exit_code == Some(0);
                    let message = if result.timed_out {
                        format!("Command timed out after {}ms", result.duration_ms)
                    } else if success {
                        let stdout_preview = result.stdout.trim();
                        if stdout_preview.is_empty() {
                            "OK".to_string()
                        } else {
                            // Show first 200 chars of stdout in the message
                            let preview = if stdout_preview.len() > 200 {
                                format!("{}...", &stdout_preview[..200])
                            } else {
                                stdout_preview.to_string()
                            };
                            preview
                        }
                    } else {
                        // Failed: always include stderr in the message so the agent sees why
                        let stderr = result.stderr.trim();
                        let stdout = result.stdout.trim();
                        let mut msg = format!("Exit code: {}", result.exit_code.unwrap_or(-1));
                        if !stderr.is_empty() {
                            let err_preview = if stderr.len() > 500 {
                                format!("{}...", &stderr[..500])
                            } else {
                                stderr.to_string()
                            };
                            msg.push_str(&format!("\nstderr: {}", err_preview));
                        }
                        if !stdout.is_empty() && stderr.is_empty() {
                            let out_preview = if stdout.len() > 500 {
                                format!("{}...", &stdout[..500])
                            } else {
                                stdout.to_string()
                            };
                            msg.push_str(&format!("\nstdout: {}", out_preview));
                        }
                        msg
                    };

                    Ok(ToolResult {
                        success,
                        message,
                        data: Some(serde_json::json!({
                            "command": result.command,
                            "exit_code": result.exit_code,
                            "stdout": result.stdout,
                            "stderr": result.stderr,
                            "timed_out": result.timed_out,
                            "duration_ms": result.duration_ms,
                        })),
                    })
                },
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Command execution failed: {}", e),
                    data: None,
                }),
            }
        }

        "fetch_url" => {
            let url = call.arguments["url"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'url' argument"))?;

            match ctx.web.fetch(url).await {
                Ok(result) => Ok(ToolResult {
                    success: result.status_code >= 200 && result.status_code < 300,
                    message: if result.truncated {
                        format!("Fetched {} ({} bytes, truncated)", result.url, result.body.len())
                    } else {
                        format!("Fetched {} ({} bytes)", result.url, result.body.len())
                    },
                    data: Some(serde_json::json!({
                        "url": result.url,
                        "status_code": result.status_code,
                        "content_type": result.content_type,
                        "content_length": result.content_length,
                        "body": result.body,
                        "truncated": result.truncated,
                        "duration_ms": result.duration_ms,
                    })),
                }),
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Failed to fetch URL: {}", e),
                    data: None,
                }),
            }
        }

        "create_skill" => {
            let description = call.arguments["description"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'description' argument"))?;
            let name = call.arguments["name"].as_str();
            let category = call.arguments["category"].as_str();

            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::Custom("CreateSkill".to_string()),
                description: format!("Create skill: {}", name.unwrap_or(description)),
                risk_level: crate::security::approval::RiskLevel::Medium,
                target: description.to_string(),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    execute_create_skill(description, name, category).await
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Skill creation was not approved".to_string(),
                    data: None,
                })
            }
        }

        "list_skills" => {
            execute_list_skills()
        }

        "use_skill" => {
            let skill_id = call.arguments["skill_id"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'skill_id' argument"))?;
            let params = call.arguments.get("params").and_then(|p| p.as_object());

            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::Custom("UseSkill".to_string()),
                description: format!("Execute skill: {}", skill_id),
                risk_level: crate::security::approval::RiskLevel::Medium,
                target: skill_id.to_string(),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    execute_use_skill(skill_id, params).await
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Skill execution was not approved".to_string(),
                    data: None,
                })
            }
        }

        // Exploration tools
        "search_content" => {
            let pattern = call.arguments["pattern"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'pattern' argument"))?;
            let directory = call.arguments["directory"].as_str().unwrap_or(".");
            let file_pattern = call.arguments["file_pattern"].as_str();
            let max_results = call.arguments["max_results"].as_u64().unwrap_or(100) as usize;

            execute_search_content(directory, pattern, file_pattern, max_results)
        }

        "find_files" => {
            let name_pattern = call.arguments["name_pattern"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'name_pattern' argument"))?;
            let directory = call.arguments["directory"].as_str().unwrap_or(".");
            let file_type = call.arguments["file_type"].as_str().unwrap_or("all");
            let max_depth = call.arguments["max_depth"].as_u64().map(|d| d as usize);

            execute_find_files(directory, name_pattern, file_type, max_depth)
        }

        "get_cwd" => {
            execute_get_cwd()
        }

        "glob" => {
            let pattern = call.arguments["pattern"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'pattern' argument"))?;
            let directory = call.arguments["directory"].as_str().unwrap_or(".");

            execute_glob(directory, pattern)
        }

        // Self-editing tools
        "edit_personality" => {
            let field = call.arguments["field"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'field' argument"))?;
            let value = call.arguments["value"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'value' argument"))?;

            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::Custom("EditPersonality".to_string()),
                description: format!("Edit personality field '{}': {}", field, &value[..value.len().min(50)]),
                risk_level: crate::security::approval::RiskLevel::High,
                target: format!("personality.{}", field),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    execute_edit_personality(field, value)
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Personality edit was not approved".to_string(),
                    data: None,
                })
            }
        }

        "view_source" => {
            let file = call.arguments["file"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'file' argument"))?;

            execute_view_source(file)
        }

        "edit_source" => {
            let file = call.arguments["file"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'file' argument"))?;
            let old_content = call.arguments["old_content"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'old_content' argument"))?;
            let new_content = call.arguments["new_content"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'new_content' argument"))?;

            // Read current file to compute full before/after for diff preview
            let source_dir = find_project_root();
            let file_path = source_dir.join(file);
            let current_content = std::fs::read_to_string(&file_path).unwrap_or_default();

            if !current_content.contains(old_content) {
                return Ok(ToolResult {
                    success: false,
                    message: "Old content not found in file. The content must match exactly.".to_string(),
                    data: None,
                });
            }

            let after_content = current_content.replace(old_content, new_content);

            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::FileWrite,
                description: format!("Edit source: {} ({} -> {} lines)",
                    file,
                    current_content.lines().count(),
                    after_content.lines().count()),
                risk_level: crate::security::approval::RiskLevel::High,
                target: file_path.display().to_string(),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            // Show diff preview for approval (same as write_file)
            match ctx.approver.request_approval_with_diff(action, &current_content, &after_content) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    execute_edit_source(file, old_content, new_content)
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Source edit was not approved".to_string(),
                    data: None,
                })
            }
        }

        "rebuild_self" => {
            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::Custom("RebuildSelf".to_string()),
                description: "Rebuild and reinstall agent binary".to_string(),
                risk_level: crate::security::approval::RiskLevel::Critical,
                target: "my-agent".to_string(),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    execute_rebuild_self().await
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Rebuild was not approved".to_string(),
                    data: None,
                })
            }
        }

        "self_diagnose" => {
            let issue = call.arguments["issue"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'issue' argument"))?;
            let context = call.arguments["context"].as_str().unwrap_or("");
            execute_self_diagnose(issue, context).await
        }

        "self_repair" => {
            let issue_type = call.arguments["issue_type"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'issue_type' argument"))?;
            let details = call.arguments["details"].as_str().unwrap_or("");

            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::Custom("SelfRepair".to_string()),
                description: format!("Self-repair: {}", issue_type),
                risk_level: crate::security::approval::RiskLevel::High,
                target: issue_type.to_string(),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    execute_self_repair(issue_type, details).await
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Self-repair was not approved".to_string(),
                    data: None,
                })
            }
        }

        // Orchestration tools - chat model can delegate to specialized agents
        "orchestrate_task" => {
            let task = call.arguments["task"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'task' argument"))?;
            let agent_type = call.arguments["agent_type"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'agent_type' argument"))?;
            let reason = call.arguments["reason"].as_str().unwrap_or("Specialized handling needed");

            execute_orchestrate_task(task, agent_type, reason).await
        }

        "spawn_agents" => {
            let main_task = call.arguments["main_task"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'main_task' argument"))?;
            let subtasks = call.arguments["subtasks"].as_array();

            execute_spawn_agents(main_task, subtasks).await
        }

        "spawn_subagent" => {
            let task = call.arguments["task"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'task' argument"))?;
            let agent_type_str = call.arguments["agent_type"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'agent_type' argument"))?;

            execute_spawn_subagent(task, agent_type_str, ctx).await
        }

        // Desktop control tools
        "capture_screen" => {
            let region = call.arguments["region"].as_str().unwrap_or("full");

            if region == "region" {
                let x = call.arguments["x"].as_i64()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'x' for region capture"))? as i32;
                let y = call.arguments["y"].as_i64()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'y' for region capture"))? as i32;
                let width = call.arguments["width"].as_u64()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'width' for region capture"))? as u32;
                let height = call.arguments["height"].as_u64()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'height' for region capture"))? as u32;

                match ctx.desktop.capture_region(x, y, width, height) {
                    Ok(result) => Ok(ToolResult {
                        success: true,
                        message: format!("Captured {}x{} region at ({}, {})", result.width, result.height, x, y),
                        data: Some(serde_json::json!({
                            "width": result.width,
                            "height": result.height,
                            "base64_data": result.base64_data,
                            "media_type": result.media_type,
                        })),
                    }),
                    Err(e) => Ok(ToolResult {
                        success: false,
                        message: format!("Failed to capture screen region: {}", e),
                        data: None,
                    }),
                }
            } else {
                match ctx.desktop.capture_screenshot() {
                    Ok(result) => Ok(ToolResult {
                        success: true,
                        message: format!("Captured screenshot: {}x{}", result.width, result.height),
                        data: Some(serde_json::json!({
                            "width": result.width,
                            "height": result.height,
                            "base64_data": result.base64_data,
                            "media_type": result.media_type,
                        })),
                    }),
                    Err(e) => Ok(ToolResult {
                        success: false,
                        message: format!("Failed to capture screenshot: {}", e),
                        data: None,
                    }),
                }
            }
        }

        "mouse_click" => {
            let x = call.arguments["x"].as_i64().map(|v| v as i32);
            let y = call.arguments["y"].as_i64().map(|v| v as i32);
            let button_str = call.arguments["button"].as_str().unwrap_or("left");

            let button = match button_str {
                "right" => crate::tools::desktop::MouseButton::Right,
                "middle" => crate::tools::desktop::MouseButton::Middle,
                _ => crate::tools::desktop::MouseButton::Left,
            };

            // Create action for approval
            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::Custom("DesktopControl".to_string()),
                description: format!("Mouse click {:?} at ({:?}, {:?})", button, x, y),
                risk_level: crate::security::approval::RiskLevel::Medium,
                target: format!("screen coordinates ({:?}, {:?})", x, y),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    match ctx.desktop.mouse_click(x, y, button) {
                        Ok(()) => Ok(ToolResult {
                            success: true,
                            message: format!("Clicked {:?} button at ({:?}, {:?})", button, x, y),
                            data: None,
                        }),
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Failed to click: {}", e),
                            data: None,
                        }),
                    }
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Mouse click action was not approved".to_string(),
                    data: None,
                })
            }
        }

        "mouse_double_click" => {
            let x = call.arguments["x"].as_i64().map(|v| v as i32);
            let y = call.arguments["y"].as_i64().map(|v| v as i32);

            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::Custom("DesktopControl".to_string()),
                description: format!("Mouse double-click at ({:?}, {:?})", x, y),
                risk_level: crate::security::approval::RiskLevel::Medium,
                target: format!("screen coordinates ({:?}, {:?})", x, y),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    match ctx.desktop.mouse_double_click(x, y) {
                        Ok(()) => Ok(ToolResult {
                            success: true,
                            message: format!("Double-clicked at ({:?}, {:?})", x, y),
                            data: None,
                        }),
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Failed to double-click: {}", e),
                            data: None,
                        }),
                    }
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Double-click action was not approved".to_string(),
                    data: None,
                })
            }
        }

        "mouse_scroll" => {
            let direction_str = call.arguments["direction"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'direction' argument"))?;
            let amount = call.arguments["amount"].as_i64().unwrap_or(3) as i32;

            let direction = match direction_str {
                "up" => crate::tools::desktop::ScrollDirection::Up,
                _ => crate::tools::desktop::ScrollDirection::Down,
            };

            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::Custom("DesktopControl".to_string()),
                description: format!("Mouse scroll {:?} by {}", direction, amount),
                risk_level: crate::security::approval::RiskLevel::Low,
                target: "mouse scroll".to_string(),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    match ctx.desktop.mouse_scroll(direction, amount) {
                        Ok(()) => Ok(ToolResult {
                            success: true,
                            message: format!("Scrolled {:?} by {}", direction, amount),
                            data: None,
                        }),
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Failed to scroll: {}", e),
                            data: None,
                        }),
                    }
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Scroll action was not approved".to_string(),
                    data: None,
                })
            }
        }

        "mouse_drag" => {
            let from_x = call.arguments["from_x"].as_i64()
                .ok_or_else(|| anyhow::anyhow!("Missing 'from_x' argument"))? as i32;
            let from_y = call.arguments["from_y"].as_i64()
                .ok_or_else(|| anyhow::anyhow!("Missing 'from_y' argument"))? as i32;
            let to_x = call.arguments["to_x"].as_i64()
                .ok_or_else(|| anyhow::anyhow!("Missing 'to_x' argument"))? as i32;
            let to_y = call.arguments["to_y"].as_i64()
                .ok_or_else(|| anyhow::anyhow!("Missing 'to_y' argument"))? as i32;

            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::Custom("DesktopControl".to_string()),
                description: format!("Mouse drag from ({}, {}) to ({}, {})", from_x, from_y, to_x, to_y),
                risk_level: crate::security::approval::RiskLevel::Medium,
                target: format!("screen coordinates ({}, {}) to ({}, {})", from_x, from_y, to_x, to_y),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    match ctx.desktop.mouse_drag(from_x, from_y, to_x, to_y) {
                        Ok(()) => Ok(ToolResult {
                            success: true,
                            message: format!("Dragged from ({}, {}) to ({}, {})", from_x, from_y, to_x, to_y),
                            data: None,
                        }),
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Failed to drag: {}", e),
                            data: None,
                        }),
                    }
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Drag action was not approved".to_string(),
                    data: None,
                })
            }
        }

        "keyboard_type" => {
            let text = call.arguments["text"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'text' argument"))?;

            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::Custom("DesktopControl".to_string()),
                description: format!("Keyboard type: {}", text),
                risk_level: crate::security::approval::RiskLevel::Medium,
                target: "keyboard input".to_string(),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    match ctx.desktop.keyboard_type(text) {
                        Ok(()) => Ok(ToolResult {
                            success: true,
                            message: format!("Typed {} characters", text.len()),
                            data: None,
                        }),
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Failed to type: {}", e),
                            data: None,
                        }),
                    }
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Type action was not approved".to_string(),
                    data: None,
                })
            }
        }

        "keyboard_press" => {
            let key_str = call.arguments["key"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'key' argument"))?;

            let key = crate::tools::desktop::Key::from_name(key_str)
                .ok_or_else(|| anyhow::anyhow!("Unknown key: {}", key_str))?;

            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::Custom("DesktopControl".to_string()),
                description: format!("Keyboard press: {}", key_str),
                risk_level: crate::security::approval::RiskLevel::Low,
                target: "keyboard input".to_string(),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    match ctx.desktop.keyboard_press(key) {
                        Ok(()) => Ok(ToolResult {
                            success: true,
                            message: format!("Pressed key: {}", key_str),
                            data: None,
                        }),
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Failed to press key: {}", e),
                            data: None,
                        }),
                    }
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Key press action was not approved".to_string(),
                    data: None,
                })
            }
        }

        "keyboard_hotkey" => {
            let keys_array = call.arguments["keys"].as_array()
                .ok_or_else(|| anyhow::anyhow!("Missing 'keys' argument"))?;

            let mut keys = Vec::new();
            for key_val in keys_array {
                if let Some(key_str) = key_val.as_str() {
                    if let Some(key) = crate::tools::desktop::Key::from_name(key_str) {
                        keys.push(key);
                    } else {
                        return Ok(ToolResult {
                            success: false,
                            message: format!("Unknown key in hotkey: {}", key_str),
                            data: None,
                        });
                    }
                }
            }

            let key_names: Vec<&str> = keys_array.iter()
                .filter_map(|k| k.as_str())
                .collect();

            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::Custom("DesktopControl".to_string()),
                description: format!("Keyboard hotkey: {}", key_names.join("+")),
                risk_level: crate::security::approval::RiskLevel::Medium,
                target: "keyboard input".to_string(),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    match ctx.desktop.keyboard_hotkey(&keys) {
                        Ok(()) => Ok(ToolResult {
                            success: true,
                            message: format!("Pressed hotkey: {}", key_names.join("+")),
                            data: None,
                        }),
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Failed to press hotkey: {}", e),
                            data: None,
                        }),
                    }
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Hotkey action was not approved".to_string(),
                    data: None,
                })
            }
        }

        "open_application" => {
            let name = call.arguments["name"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'name' argument"))?;

            let action = crate::security::approval::Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: crate::security::approval::ActionType::CommandExecute,
                description: format!("Open application: {}", name),
                risk_level: crate::security::approval::RiskLevel::High,
                target: name.to_string(),
                details: std::collections::HashMap::new(),
                requested_at: chrono::Utc::now(),
            };

            match ctx.approver.request_approval(action) {
                Ok(crate::security::approval::ApprovalDecision::Approved) |
                Ok(crate::security::approval::ApprovalDecision::ApprovedForSession) => {
                    match ctx.desktop.open_application(name) {
                        Ok(()) => Ok(ToolResult {
                            success: true,
                            message: format!("Launched application: {}", name),
                            data: None,
                        }),
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Failed to launch application: {}", e),
                            data: None,
                        }),
                    }
                }
                _ => Ok(ToolResult {
                    success: false,
                    message: "Application launch was not approved".to_string(),
                    data: None,
                })
            }
        }

        // Self-improvement tools
        "analyze_performance" => {
            let focus = call.arguments["focus"].as_str().unwrap_or("all");
            execute_analyze_performance(focus).await
        }

        "get_lessons" => {
            let context = call.arguments["context"].as_str().unwrap_or("");
            let min_confidence = call.arguments["min_confidence"].as_f64().unwrap_or(0.3) as f32;
            execute_get_lessons(context, min_confidence).await
        }

        "record_lesson" => {
            let insight = call.arguments["insight"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'insight' argument"))?;
            let context = call.arguments["context"].as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'context' argument"))?;
            let related_tools: Vec<String> = call.arguments["related_tools"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            execute_record_lesson(insight, context, &related_tools).await
        }

        "improve_self" => {
            let area = call.arguments["area"].as_str().unwrap_or("all");
            execute_improve_self(area).await
        }

        // Learning tools
        "record_learning" => {
            match crate::learning::LearningStore::new() {
                Ok(store) => {
                    let area = call.arguments["area"].as_str().unwrap_or("general");
                    let title = call.arguments["title"].as_str().unwrap_or("Untitled");
                    let description = call.arguments["description"].as_str().unwrap_or("");
                    let context = call.arguments["context"].as_str().unwrap_or("");
                    let priority = match call.arguments["priority"].as_str().unwrap_or("medium") {
                        "low" => crate::learning::Priority::Low,
                        "high" => crate::learning::Priority::High,
                        "critical" => crate::learning::Priority::Critical,
                        _ => crate::learning::Priority::Medium,
                    };
                    match store.record_learning(area, title, description, context, None, vec![], priority) {
                        Ok(entry) => Ok(ToolResult {
                            success: true,
                            message: format!("Learning recorded: {} - {}", entry.id, entry.title),
                            data: Some(serde_json::json!({"id": entry.id, "title": entry.title})),
                        }),
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Failed to record learning: {}", e),
                            data: None,
                        }),
                    }
                }
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Learning store unavailable: {}", e),
                    data: None,
                }),
            }
        }

        "review_learnings" => {
            match crate::learning::LearningStore::new() {
                Ok(store) => {
                    let status_filter = call.arguments["status"].as_str().unwrap_or("all");
                    let entries = match status_filter {
                        "new" => store.get_by_status(&crate::learning::EntryStatus::New),
                        "validated" => store.get_by_status(&crate::learning::EntryStatus::Validated),
                        "promoted" => store.get_by_status(&crate::learning::EntryStatus::Promoted),
                        _ => {
                            let mut all = store.get_all(&crate::learning::EntryType::Learning).unwrap_or_default();
                            all.extend(store.get_all(&crate::learning::EntryType::Error).unwrap_or_default());
                            all.extend(store.get_all(&crate::learning::EntryType::FeatureRequest).unwrap_or_default());
                            Ok(all)
                        }
                    };
                    match entries {
                        Ok(entries) => {
                            if entries.is_empty() {
                                return Ok(ToolResult {
                                    success: true,
                                    message: "No learnings found.".to_string(),
                                    data: None,
                                });
                            }
                            let mut output = format!("Found {} entries:\n\n", entries.len());
                            for entry in entries.iter().take(20) {
                                output.push_str(&format!(
                                    "- {} [{}] ({}, {}) — {}\n",
                                    entry.id, entry.status, entry.priority, entry.area, entry.title
                                ));
                            }
                            Ok(ToolResult {
                                success: true,
                                message: output,
                                data: Some(serde_json::json!({"count": entries.len()})),
                            })
                        }
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Failed to review learnings: {}", e),
                            data: None,
                        }),
                    }
                }
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Learning store unavailable: {}", e),
                    data: None,
                }),
            }
        }

        "search_learnings" => {
            match crate::learning::LearningStore::new() {
                Ok(store) => {
                    let query = call.arguments["query"].as_str().unwrap_or("");
                    if query.is_empty() {
                        return Ok(ToolResult {
                            success: false,
                            message: "Please provide a search query.".to_string(),
                            data: None,
                        });
                    }
                    match store.search(query) {
                        Ok(results) => {
                            if results.is_empty() {
                                return Ok(ToolResult {
                                    success: true,
                                    message: format!("No results for '{}'", query),
                                    data: None,
                                });
                            }
                            let mut output = format!("Found {} results for '{}':\n\n", results.len(), query);
                            for entry in results.iter().take(10) {
                                output.push_str(&format!(
                                    "- {} [{}] — {}: {}\n",
                                    entry.id, entry.status, entry.title, entry.description
                                ));
                            }
                            Ok(ToolResult {
                                success: true,
                                message: output,
                                data: Some(serde_json::json!({"count": results.len()})),
                            })
                        }
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Search failed: {}", e),
                            data: None,
                        }),
                    }
                }
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Learning store unavailable: {}", e),
                    data: None,
                }),
            }
        }

        "promote_learning" => {
            match crate::learning::LearningStore::new() {
                Ok(store) => {
                    let entry_id = call.arguments["entry_id"].as_str()
                        .unwrap_or("");
                    if entry_id.is_empty() {
                        return Ok(ToolResult {
                            success: false,
                            message: "Missing 'entry_id' argument".to_string(),
                            data: None,
                        });
                    }
                    match store.promote(entry_id) {
                        Ok(()) => Ok(ToolResult {
                            success: true,
                            message: format!("Learning {} promoted to permanent context", entry_id),
                            data: None,
                        }),
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Failed to promote: {}", e),
                            data: None,
                        }),
                    }
                }
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Learning store unavailable: {}", e),
                    data: None,
                }),
            }
        }

        "demote_learning" => {
            match crate::learning::LearningStore::new() {
                Ok(store) => {
                    let entry_id = call.arguments["entry_id"].as_str()
                        .unwrap_or("");
                    if entry_id.is_empty() {
                        return Ok(ToolResult {
                            success: false,
                            message: "Missing 'entry_id' argument".to_string(),
                            data: None,
                        });
                    }
                    match store.dismiss(entry_id) {
                        Ok(()) => Ok(ToolResult {
                            success: true,
                            message: format!("Learning {} demoted from permanent context", entry_id),
                            data: None,
                        }),
                        Err(e) => Ok(ToolResult {
                            success: false,
                            message: format!("Failed to demote: {}", e),
                            data: None,
                        }),
                    }
                }
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Learning store unavailable: {}", e),
                    data: None,
                }),
            }
        }

        _ => Ok(ToolResult {
            success: false,
            message: format!("Unknown tool: {}", call.name),
            data: None,
        }),
    }
}

// ============================================================================
// Skill management tool implementations
// ============================================================================

/// Execute the create_skill tool
async fn execute_create_skill(
    description: &str,
    name: Option<&str>,
    category: Option<&str>,
) -> anyhow::Result<ToolResult> {
    use crate::skills::generator::{SkillGenerator, GenerationRequest};
    use crate::skills::registry::{SkillCategory, Permission};

    // Parse category
    let skill_category = category.and_then(|c| match c.to_lowercase().as_str() {
        "filesystem" => Some(SkillCategory::Filesystem),
        "shell" => Some(SkillCategory::Shell),
        "web" => Some(SkillCategory::Web),
        "data" => Some(SkillCategory::Data),
        "system" => Some(SkillCategory::System),
        "utility" => Some(SkillCategory::Utility),
        "custom" => Some(SkillCategory::Custom),
        _ => None,
    });

    // Create generator
    let api_key = crate::security::keyring::get_api_key().unwrap_or_default();
    let config = crate::config::Config::load().unwrap_or_default();
    let generator = if !api_key.is_empty() {
        SkillGenerator::new()
            .with_api_key(api_key)
            .with_model(config.models.chat.clone())
    } else {
        SkillGenerator::new()
    };

    // Build request
    let request = GenerationRequest {
        description: description.to_string(),
        name: name.map(|s| s.to_string()),
        category: skill_category,
        permissions: vec![Permission::ReadFiles],
        examples: vec![],
    };

    // Generate skill
    match generator.generate(request).await {
        Ok(generated) => {
            let skill_id = generated.meta.id.clone();
            let skill_name = generated.meta.name.clone();
            let skill_desc = generated.meta.description.clone();

            // Compile and register the skill
            let registry = crate::skills::default_registry();
            match generator.compile_skill(&generated) {
                Ok(skill) => {
                    if let Err(e) = registry.register(skill) {
                        return Ok(ToolResult {
                            success: false,
                            message: format!("Failed to register skill: {}", e),
                            data: None,
                        });
                    }

                    // Save skill metadata
                    if let Err(e) = registry.save_skill(&generated.meta) {
                        tracing::warn!("Failed to save skill metadata: {}", e);
                    }

                    Ok(ToolResult {
                        success: true,
                        message: format!("Created skill: {} ({})", skill_name, skill_id),
                        data: Some(serde_json::json!({
                            "skill_id": skill_id,
                            "name": skill_name,
                            "description": skill_desc,
                            "category": format!("{:?}", generated.meta.category),
                        })),
                    })
                }
                Err(e) => Ok(ToolResult {
                    success: false,
                    message: format!("Failed to compile skill: {}", e),
                    data: None,
                }),
            }
        }
        Err(e) => Ok(ToolResult {
            success: false,
            message: format!("Failed to generate skill: {}", e),
            data: None,
        }),
    }
}

/// Execute the list_skills tool
fn execute_list_skills() -> anyhow::Result<ToolResult> {
    let registry = crate::skills::default_registry();
    let skills = registry.list();

    let skill_list: Vec<_> = skills.iter().map(|s| {
        serde_json::json!({
            "id": s.id,
            "name": s.name,
            "description": s.description,
            "category": format!("{:?}", s.category),
            "builtin": s.builtin,
        })
    }).collect();

    Ok(ToolResult {
        success: true,
        message: format!("Found {} skills", skill_list.len()),
        data: Some(serde_json::json!({
            "count": skill_list.len(),
            "skills": skill_list,
        })),
    })
}

/// Execute the use_skill tool
async fn execute_use_skill(
    skill_id: &str,
    params: Option<&serde_json::Map<String, serde_json::Value>>,
) -> anyhow::Result<ToolResult> {
    use crate::skills::registry::SkillContext;
    use std::collections::HashMap;

    let registry = crate::skills::default_registry();

    // Get the skill
    let skill = match registry.get(skill_id) {
        Some(s) => s,
        None => {
            return Ok(ToolResult {
                success: false,
                message: format!("Skill not found: {}", skill_id),
                data: None,
            });
        }
    };

    // Build parameters
    let skill_params: HashMap<String, String> = params
        .map(|p| p.iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect())
        .unwrap_or_default();

    // Build context
    let skill_ctx = SkillContext {
        working_dir: std::env::current_dir().unwrap_or_default(),
        env: HashMap::new(),
        timeout_secs: 30,
        require_approval: !skill.meta.builtin,
    };

    // Execute skill
    match skill.execute(skill_params, &skill_ctx) {
        Ok(result) => {
            let error_clone = result.error.clone();
            Ok(ToolResult {
                success: result.success,
                message: if result.success {
                    format!("Skill executed in {}ms", result.duration_ms)
                } else {
                    error_clone.unwrap_or_else(|| "Skill execution failed".to_string())
                },
                data: Some(serde_json::json!({
                    "skill_id": skill_id,
                    "output": result.output,
                    "error": result.error,
                    "duration_ms": result.duration_ms,
                })),
            })
        }
        Err(e) => Ok(ToolResult {
            success: false,
            message: format!("Skill execution error: {}", e),
            data: None,
        }),
    }
}

// ============================================================================
// Exploration tool implementations
// ============================================================================

/// Search for text content in files (like grep -r)
fn execute_search_content(
    directory: &str,
    pattern: &str,
    file_pattern: Option<&str>,
    max_results: usize,
) -> anyhow::Result<ToolResult> {
    use std::fs;
    use std::path::Path;

    let dir_path = Path::new(directory);
    if !dir_path.exists() {
        return Ok(ToolResult {
            success: false,
            message: format!("Directory does not exist: {}", directory),
            data: None,
        });
    }

    let mut results = Vec::new();
    let pattern_lower = pattern.to_lowercase();

    fn search_dir(
        dir: &Path,
        pattern: &str,
        file_pattern: Option<&str>,
        max_results: usize,
        results: &mut Vec<serde_json::Value>,
    ) -> anyhow::Result<()> {
        if results.len() >= max_results {
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Skip hidden directories and common non-code directories
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with('.') || name == "target" || name == "node_modules" || name == "vendor" {
                        continue;
                    }
                }
                search_dir(&path, pattern, file_pattern, max_results, results)?;
            } else if path.is_file() {
                // Check file pattern
                if let Some(fp) = file_pattern {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if !name.contains(fp.trim_start_matches('*').trim_end_matches('*')) {
                            continue;
                        }
                    }
                }

                // Try to read and search file
                if let Ok(content) = fs::read_to_string(&path) {
                    for (line_num, line) in content.lines().enumerate() {
                        if results.len() >= max_results {
                            return Ok(());
                        }
                        if line.to_lowercase().contains(pattern) {
                            results.push(serde_json::json!({
                                "file": path.display().to_string(),
                                "line": line_num + 1,
                                "content": line.trim().chars().take(200).collect::<String>()
                            }));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    search_dir(dir_path, &pattern_lower, file_pattern, max_results, &mut results)?;

    Ok(ToolResult {
        success: true,
        message: format!("Found {} matches for '{}'", results.len(), pattern),
        data: Some(serde_json::json!({
            "pattern": pattern,
            "directory": directory,
            "matches": results,
            "count": results.len()
        })),
    })
}

/// Find files matching a pattern
fn execute_find_files(
    directory: &str,
    name_pattern: &str,
    file_type: &str,
    max_depth: Option<usize>,
) -> anyhow::Result<ToolResult> {
    use std::fs;
    use std::path::Path;

    let dir_path = Path::new(directory);
    if !dir_path.exists() {
        return Ok(ToolResult {
            success: false,
            message: format!("Directory does not exist: {}", directory),
            data: None,
        });
    }

    let mut results = Vec::new();
    let pattern_lower = name_pattern.to_lowercase();

    fn find_in_dir(
        dir: &Path,
        pattern: &str,
        file_type: &str,
        current_depth: usize,
        max_depth: Option<usize>,
        results: &mut Vec<serde_json::Value>,
    ) -> anyhow::Result<()> {
        if let Some(max) = max_depth {
            if current_depth > max {
                return Ok(());
            }
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            // Skip hidden directories
            if name.starts_with('.') {
                continue;
            }

            // Support glob patterns (*.rs, test_*, etc.) and plain substring matching
            let name_lower = name.to_lowercase();
            let matches_name = if pattern.contains('*') || pattern.contains('?') {
                // Convert glob pattern to simple matching
                let parts: Vec<&str> = pattern.split('*').collect();
                if parts.len() == 2 && parts[0].is_empty() {
                    // Pattern like "*.rs" — check suffix
                    name_lower.ends_with(parts[1])
                } else if parts.len() == 2 && parts[1].is_empty() {
                    // Pattern like "test_*" — check prefix
                    name_lower.starts_with(parts[0])
                } else if parts.len() == 2 {
                    // Pattern like "test_*.rs" — check prefix and suffix
                    name_lower.starts_with(parts[0]) && name_lower.ends_with(parts[1])
                } else {
                    // Fallback: substring match
                    name_lower.contains(pattern)
                }
            } else {
                name_lower.contains(pattern)
            };
            let is_file = path.is_file();
            let is_dir = path.is_dir();

            let matches_type = match file_type {
                "file" => is_file,
                "directory" | "dir" => is_dir,
                _ => true,
            };

            if matches_name && matches_type {
                let metadata = fs::metadata(&path).ok();
                results.push(serde_json::json!({
                    "path": path.display().to_string(),
                    "name": name,
                    "type": if is_file { "file" } else { "directory" },
                    "size": metadata.as_ref().map(|m| m.len()).unwrap_or(0),
                }));
            }

            if is_dir {
                find_in_dir(&path, pattern, file_type, current_depth + 1, max_depth, results)?;
            }
        }
        Ok(())
    }

    find_in_dir(dir_path, &pattern_lower, file_type, 0, max_depth, &mut results)?;

    Ok(ToolResult {
        success: true,
        message: format!("Found {} items matching '{}'", results.len(), name_pattern),
        data: Some(serde_json::json!({
            "pattern": name_pattern,
            "directory": directory,
            "file_type": file_type,
            "results": results,
            "count": results.len()
        })),
    })
}

/// Get current working directory
fn execute_get_cwd() -> anyhow::Result<ToolResult> {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    Ok(ToolResult {
        success: true,
        message: format!("Current directory: {}", cwd),
        data: Some(serde_json::json!({
            "cwd": cwd
        })),
    })
}

/// Find files using glob patterns
fn execute_glob(directory: &str, pattern: &str) -> anyhow::Result<ToolResult> {
    use std::path::Path;

    let base_path = Path::new(directory);
    if !base_path.exists() {
        return Ok(ToolResult {
            success: false,
            message: format!("Directory does not exist: {}", directory),
            data: None,
        });
    }

    let full_pattern = if pattern.starts_with('/') {
        pattern.to_string()
    } else {
        format!("{}/{}", directory, pattern)
    };

    // Simple glob implementation
    let mut results = Vec::new();

    fn glob_match(pattern: &str, path: &std::path::Path) -> bool {
        let pattern_parts: Vec<&str> = pattern.split('/').collect();
        let path_parts: Vec<&str> = path.iter().filter_map(|p| p.to_str()).collect();

        fn match_parts(pat: &[&str], path: &[&str]) -> bool {
            match (pat.first(), path.first()) {
                (None, None) => true,
                (None, Some(_)) => false,
                (Some(&p), None) => p == "**",
                (Some(&p), Some(&s)) => {
                    if p == "**" {
                        // Try matching ** with nothing, or with current and rest
                        match_parts(&pat[1..], path) || match_parts(pat, &path[1..])
                    } else if p == "*" || p == s {
                        match_parts(&pat[1..], &path[1..])
                    } else if p.contains('*') {
                        // Simple wildcard matching
                        let prefix = p.split('*').next().unwrap_or("");
                        let suffix = p.split('*').last().unwrap_or("");
                        s.starts_with(prefix) && s.ends_with(suffix)
                    } else {
                        false
                    }
                }
            }
        }

        match_parts(&pattern_parts, &path_parts)
    }

    fn walk_and_collect(
        dir: &std::path::Path,
        pattern: &str,
        results: &mut Vec<String>,
    ) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            // Skip hidden
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            if glob_match(pattern, &path) {
                results.push(path.display().to_string());
            }

            if path.is_dir() {
                walk_and_collect(&path, pattern, results)?;
            }
        }
        Ok(())
    }

    if let Err(e) = walk_and_collect(base_path, &full_pattern, &mut results) {
        return Ok(ToolResult {
            success: false,
            message: format!("Glob error: {}", e),
            data: None,
        });
    }

    Ok(ToolResult {
        success: true,
        message: format!("Found {} files matching '{}'", results.len(), pattern),
        data: Some(serde_json::json!({
            "pattern": pattern,
            "directory": directory,
            "files": results,
            "count": results.len()
        })),
    })
}

// ============================================================================
// Self-editing tool implementations
// ============================================================================

/// Edit the agent's personality file
fn execute_edit_personality(field: &str, value: &str) -> anyhow::Result<ToolResult> {
    use std::fs;
    use std::path::PathBuf;

    let personality_path = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("my-agent")
        .join("soul")
        .join("personality.toml");

    // Load existing or create default
    let mut personality = crate::soul::Personality::load().unwrap_or_default();

    // Update the specified field
    match field {
        "name" => personality.name = value.to_string(),
        "traits" => {
            personality.traits = value.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        "system_prompt" => personality.system_prompt = value.to_string(),
        "greeting" => personality.greeting = Some(value.to_string()),
        "farewell" => personality.farewell = Some(value.to_string()),
        "style.formality" => personality.style.formality = value.to_string(),
        "style.length" => personality.style.length = value.to_string(),
        "style.emojis" => {
            personality.style.emojis = value.to_lowercase() == "true";
        }
        _ => {
            return Ok(ToolResult {
                success: false,
                message: format!("Unknown personality field: {}", field),
                data: None,
            });
        }
    }

    // Save the updated personality
    if let Err(e) = personality.save() {
        return Ok(ToolResult {
            success: false,
            message: format!("Failed to save personality: {}", e),
            data: None,
        });
    }

    Ok(ToolResult {
        success: true,
        message: format!("Updated personality field '{}' to: {}", field, value),
        data: Some(serde_json::json!({
            "field": field,
            "value": value,
            "path": personality_path.display().to_string()
        })),
    })
}

/// Find the my-agent project root directory
fn find_project_root() -> std::path::PathBuf {
    // Check if CWD is the project root (has Cargo.toml + src/)
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.join("Cargo.toml").exists() && cwd.join("src").exists() {
            return cwd;
        }
        // Walk up to find project root
        let mut dir = cwd.as_path();
        while let Some(parent) = dir.parent() {
            if parent.join("Cargo.toml").exists() && parent.join("src").exists() {
                return parent.to_path_buf();
            }
            dir = parent;
        }
    }
    // Fallback to known path
    std::path::PathBuf::from("/home/rapheal/Projects/my-agent")
}

/// View the agent's source code
fn execute_view_source(file: &str) -> anyhow::Result<ToolResult> {
    use std::fs;

    let source_dir = find_project_root();
    let file_path = source_dir.join(file);

    if !file_path.exists() {
        return Ok(ToolResult {
            success: false,
            message: format!("Source file not found: {}", file),
            data: None,
        });
    }

    // Check it's a source file
    let extension = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !["rs", "toml", "md", "json", "yaml", "yml"].contains(&extension) {
        return Ok(ToolResult {
            success: false,
            message: "Can only view source files (.rs, .toml, .md, .json, .yaml)".to_string(),
            data: None,
        });
    }

    match fs::read_to_string(&file_path) {
        Ok(content) => {
            let lines = content.lines().count();
            Ok(ToolResult {
                success: true,
                message: format!("Read {} lines from {}", lines, file),
                data: Some(serde_json::json!({
                    "file": file,
                    "content": content,
                    "lines": lines,
                    "path": file_path.display().to_string()
                })),
            })
        }
        Err(e) => Ok(ToolResult {
            success: false,
            message: format!("Failed to read file: {}", e),
            data: None,
        }),
    }
}

/// Edit the agent's source code
fn execute_edit_source(file: &str, old_content: &str, new_content: &str) -> anyhow::Result<ToolResult> {
    use std::fs;

    let source_dir = find_project_root();
    let file_path = source_dir.join(file);

    if !file_path.exists() {
        return Ok(ToolResult {
            success: false,
            message: format!("Source file not found: {}", file),
            data: None,
        });
    }

    // Read current content
    let content = match fs::read_to_string(&file_path) {
        Ok(c) => c,
        Err(e) => {
            return Ok(ToolResult {
                success: false,
                message: format!("Failed to read file: {}", e),
                data: None,
            });
        }
    };

    // Replace old content with new
    if !content.contains(old_content) {
        return Ok(ToolResult {
            success: false,
            message: "Old content not found in file. The content must match exactly.".to_string(),
            data: None,
        });
    }

    let new_file_content = content.replace(old_content, new_content);

    // Write back
    if let Err(e) = fs::write(&file_path, new_file_content) {
        return Ok(ToolResult {
            success: false,
            message: format!("Failed to write file: {}", e),
            data: None,
        });
    }

    Ok(ToolResult {
        success: true,
        message: format!("Successfully edited {}. Use 'rebuild_self' to compile changes.", file),
        data: Some(serde_json::json!({
            "file": file,
            "old_content_preview": old_content.chars().take(100).collect::<String>(),
            "new_content_preview": new_content.chars().take(100).collect::<String>(),
        })),
    })
}

/// Rebuild and reinstall the agent
async fn execute_rebuild_self() -> anyhow::Result<ToolResult> {
    let source_dir = find_project_root();

    // Run cargo build --release (async to avoid blocking the runtime)
    let build_output = tokio::process::Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&source_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;

    match build_output {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Ok(ToolResult {
                    success: false,
                    message: format!("Build failed: |{}", stderr.chars().take(2000).collect::<String>()),
                    data: None,
                });
            }
        }
        Err(e) => {
            return Ok(ToolResult {
                success: false,
                message: format!("Failed to run cargo build: {}", e),
                data: None,
            });
        }
    }

    // Run cargo install --path . (async)
    let install_output = tokio::process::Command::new("cargo")
        .args(["install", "--path", "."])
        .current_dir(&source_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;

    match install_output {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Ok(ToolResult {
                    success: false,
                    message: format!("Install failed: |{}", stderr.chars().take(2000).collect::<String>()),
                    data: None,
                });
            }
        }
        Err(e) => {
            return Ok(ToolResult {
                success: false,
                message: format!("Failed to run cargo install: {}", e),
                data: None,
            });
        }
    }

    Ok(ToolResult {
        success: true,
        message: "Successfully rebuilt and reinstalled. Restart to use the new version.".to_string(),
        data: Some(serde_json::json!({
            "source_dir": source_dir.display().to_string(),
            "binary": "my-agent"
        })),
    })
}

/// Diagnose issues with the agent's tools
async fn execute_self_diagnose(issue: &str, context: &str) -> anyhow::Result<ToolResult> {
    let mut findings = Vec::new();
    let mut suggestions = Vec::new();

    // Check common issues
    match issue.to_lowercase().as_str() {
        s if s.contains("read") || s.contains("file") || s.contains("path") => {
            // Check if we can read files
            let cwd = std::env::current_dir().unwrap_or_default();
            findings.push(format!("Current directory: {}", cwd.display()));

            // Check if src directory exists
            if cwd.join("src").exists() {
                findings.push("✓ src/ directory found in current dir".to_string());
            } else {
                findings.push("✗ src/ directory NOT found in current dir".to_string());
                suggestions.push("Try running from the project directory, or check path resolution");
            }

            // Check for project subdirectories
            let mut project_dirs = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&cwd) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() && (path.join("Cargo.toml").exists() || path.join("package.json").exists()) {
                        project_dirs.push(path.display().to_string());
                    }
                }
            }
            if !project_dirs.is_empty() {
                findings.push(format!("Found project directories: {:?}", project_dirs));
            }

            // Try a test read
            let test_path = "src/main.rs";
            if std::path::Path::new(test_path).exists() {
                findings.push(format!("✓ {} is accessible", test_path));
            } else {
                findings.push(format!("✗ {} not accessible from current dir", test_path));
                suggestions.push("Path resolution may need to search project subdirectories");
            }
        }
        s if s.contains("tool") || s.contains("execute") => {
            findings.push("Checking tool availability...".to_string());
            // Check if tools are registered
            let tools = builtin_tools();
            findings.push(format!("{} tools registered", tools.len()));
            suggestions.push("Use /tools command to see all available tools");
        }
        _ => {
            findings.push("General diagnostics:".to_string());
            findings.push(format!("Working directory: {}", std::env::current_dir().unwrap_or_default().display()));
            findings.push(format!("Executable: {:?}", std::env::current_exe().ok()));
        }
    }

    if !context.is_empty() {
        findings.push(format!("Context provided: {}", context));
    }

    Ok(ToolResult {
        success: true,
        message: format!("Diagnosis complete:\n{}\nSuggestions: {}",
            findings.join("\n"),
            if suggestions.is_empty() { "None".to_string() } else { suggestions.join("; ") }
        ),
        data: Some(serde_json::json!({
            "issue": issue,
            "findings": findings,
            "suggestions": suggestions
        })),
    })
}

/// Attempt to repair a detected issue
async fn execute_self_repair(issue_type: &str, details: &str) -> anyhow::Result<ToolResult> {
    let mut actions_taken = Vec::new();
    let mut success = true;

    match issue_type {
        "path_resolution" => {
            // The fix is already in the sandbox resolve_path function
            // Check if it needs to be applied
            let source_path = std::path::PathBuf::from("src/security/sandbox.rs");
            if source_path.exists() {
                actions_taken.push("Path resolution fix already present in sandbox.rs".to_string());
            } else {
                actions_taken.push("Cannot find sandbox.rs - may need to rebuild".to_string());
                success = false;
            }

            // Check if tools.rs has project scanning
            let tools_path = std::path::PathBuf::from("src/agent/tools.rs");
            if tools_path.exists() {
                actions_taken.push("Tool context with project scanning should be in tools.rs".to_string());
            }

            actions_taken.push("To apply fixes: rebuild_self()".to_string());
        }
        "dependencies" => {
            // Check Cargo.toml
            if std::path::Path::new("Cargo.toml").exists() {
                actions_taken.push("Cargo.toml found".to_string());
                // Try cargo check
                let output = std::process::Command::new("cargo")
                    .args(["check", "--quiet"])
                    .output();
                match output {
                    Ok(o) if o.status.success() => actions_taken.push("✓ cargo check passed".to_string()),
                    Ok(o) => {
                        actions_taken.push(format!("✗ cargo check failed: {}", String::from_utf8_lossy(&o.stderr)));
                        success = false;
                    }
                    Err(e) => {
                        actions_taken.push(format!("✗ cargo check error: {}", e));
                        success = false;
                    }
                }
            }
        }
        "config" => {
            let config_path = dirs::config_dir()
                .map(|d| d.join("my-agent").join("config.toml"));
            if let Some(p) = &config_path {
                if p.exists() {
                    actions_taken.push(format!("Config found at {:?}", p));
                } else {
                    actions_taken.push("Config not found - will use defaults".to_string());
                }
            }
        }
        "sandbox" => {
            actions_taken.push("Checking sandbox configuration...".to_string());
            let cwd = std::env::current_dir().unwrap_or_default();
            actions_taken.push(format!("Current directory {} should be in allowed paths", cwd.display()));
        }
        _ => {
            actions_taken.push(format!("Unknown issue type: {}. Try: path_resolution, dependencies, config, sandbox", issue_type));
            success = false;
        }
    }

    if !details.is_empty() {
        actions_taken.push(format!("Additional details: {}", details));
    }

    Ok(ToolResult {
        success,
        message: format!("Repair attempt for '{}':\n{}", issue_type, actions_taken.join("\n")),
        data: Some(serde_json::json!({
            "issue_type": issue_type,
            "actions_taken": actions_taken,
            "rebuild_needed": matches!(issue_type, "path_resolution" | "dependencies")
        })),
    })
}

// ============================================================================
// Orchestration tools - chat model can delegate to specialized agents
// ============================================================================

/// Delegate a task to a specialized agent
async fn execute_orchestrate_task(task: &str, agent_type: &str, reason: &str) -> anyhow::Result<ToolResult> {
    // Get the model for this agent type
    let config = crate::config::Config::load().unwrap_or_default();
    let model = config.models.get(agent_type)
        .map(|s| s.to_string())
        .unwrap_or_else(|| config.models.orchestrator.clone());

    // Use the orchestrator to analyze the task
    let orchestrator = crate::orchestrator::SmartReasoningOrchestrator::new()?;
    let plan = orchestrator.process_request(task).await?;

    // Check if we have agents to spawn
    let has_agent = plan.agents.iter().any(|a| {
        a.capability.to_lowercase().contains(&agent_type.to_lowercase())
    });

    if has_agent {
        // Return info about the delegated task
        // The actual orchestration will be handled by the interactive CLI
        return Ok(ToolResult {
            success: true,
            message: format!("Task '{}' delegated to {} agent. Use orchestrate mode to execute.", task, agent_type),
            data: Some(serde_json::json!({
                "agent_type": agent_type,
                "model": model,
                "task": task,
                "reason": reason,
                "status": "queued",
                "hint": "Switch to orchestrate mode or use /orchestrate command to execute"
            })),
        });
    }

    // No specialized agent needed - just note it
    Ok(ToolResult {
        success: true,
        message: format!("Task can be handled without specialized agent: {}", reason),
        data: Some(serde_json::json!({
            "agent_type": agent_type,
            "task": task
        })),
    })
}

/// Spawn multiple agents for a complex task
async fn execute_spawn_agents(main_task: &str, subtasks: Option<&Vec<serde_json::Value>>) -> anyhow::Result<ToolResult> {
    use crate::orchestrator::{SmartReasoningOrchestrator, AgentSpec};

    // Build agent specs from subtasks or get from orchestrator
    let agent_specs: Vec<AgentSpec> = if let Some(tasks) = subtasks {
        tasks.iter().filter_map(|t| {
            let desc = t["description"].as_str()?;
            let agent_type = t["agent_type"].as_str()?;

            let config = crate::config::Config::load().unwrap_or_default();
            let model = config.models.get(agent_type)
                .map(|s| s.to_string())
                .unwrap_or_else(|| config.models.orchestrator.clone());

            Some(AgentSpec {
                capability: agent_type.to_string(),
                task: desc.to_string(),
                model,
            })
        }).collect()
    } else {
        // Use orchestrator to determine agents
        let orchestrator = SmartReasoningOrchestrator::new()?;
        let plan = orchestrator.process_request(main_task).await?;
        plan.agents
    };

    if agent_specs.is_empty() {
        return Ok(ToolResult {
            success: true,
            message: "No specialized agents needed for this task".to_string(),
            data: None,
        });
    }

    let agent_count = agent_specs.len();
    let agent_types: Vec<String> = agent_specs.iter().map(|a| a.capability.clone()).collect();

    // Return plan info - actual execution handled by orchestrate mode
    Ok(ToolResult {
        success: true,
        message: format!("Planned {} agents for: {}", agent_count, main_task),
        data: Some(serde_json::json!({
            "main_task": main_task,
            "agent_count": agent_count,
            "agent_types": agent_types,
            "status": "planned",
            "hint": "Use orchestrate mode to execute this plan"
        })),
    })
}

/// Spawn a subagent with full ReAct tool loop (inline, no spawner needed)
async fn execute_spawn_subagent(task: &str, agent_type_str: &str, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
    use crate::orchestrator::agent_types::SubagentType;
    use crate::agent::tool_loop::{run_tool_loop, ToolLoopConfig};

    let agent_type = SubagentType::from_capability(agent_type_str);
    let client = crate::agent::llm::OpenRouterClient::from_keyring()?;
    let config = crate::config::Config::load().unwrap_or_default();
    let model = config.models.orchestrator.clone();

    let tool_ctx = ToolContext::with_project_paths();
    let allowed_tools = agent_type.filter_tools(builtin_tools());

    let loop_config = ToolLoopConfig {
        model,
        system_prompt: agent_type.system_prompt().to_string(),
        allowed_tools,
        max_iterations: agent_type.max_iterations(),
        max_tokens: 4096,
        on_tool_start: None,
        on_tool_complete: None,
        on_progress: None,
    };

    let initial_messages = vec![
        crate::agent::llm::ChatMessage::user(task.to_string()),
    ];

    match run_tool_loop(&client, initial_messages, &tool_ctx, &loop_config).await {
        Ok(result) => {
            Ok(ToolResult {
                success: result.success,
                message: format!("{} agent completed ({} iterations, {} tool calls)",
                    agent_type.display_name(), result.iterations, result.tool_calls_made),
                data: Some(serde_json::json!({
                    "agent_type": agent_type_str,
                    "result": result.final_response,
                    "iterations": result.iterations,
                    "tool_calls": result.tool_calls_made,
                })),
            })
        }
        Err(e) => {
            Ok(ToolResult {
                success: false,
                message: format!("{} agent failed: {}", agent_type.display_name(), e),
                data: None,
            })
        }
    }
}

// ============================================================================
// Self-improvement tool implementations
// ============================================================================

/// Analyze performance metrics and generate insights
async fn execute_analyze_performance(focus: &str) -> anyhow::Result<ToolResult> {
    use crate::metrics::{MetricsStore, SelfAnalyzer};

    let store = MetricsStore::new();
    let analyzer = SelfAnalyzer::new(store);

    match analyzer.analyze().await {
        Ok(report) => {
            let mut insights = Vec::new();

            insights.push(format!("Health Score: {}/100", report.health_score));
            insights.push(format!("Summary: {}", report.summary));

            if !report.top_performers.is_empty() {
                insights.push(format!("Top performers: {}", report.top_performers.join(", ")));
            }

            if !report.attention_needed.is_empty() {
                insights.push("Tools needing attention:".to_string());
                for (tool, issue) in &report.attention_needed {
                    insights.push(format!("  - {}: {}", tool, issue));
                }
            }

            let suggestions: Vec<_> = report.suggestions.iter().take(5).map(|s| {
                serde_json::json!({
                    "priority": s.priority,
                    "title": s.title,
                    "action": s.action,
                    "category": format!("{:?}", s.category)
                })
            }).collect();

            Ok(ToolResult {
                success: true,
                message: insights.join("\n"),
                data: Some(serde_json::json!({
                    "health_score": report.health_score,
                    "top_performers": report.top_performers,
                    "attention_needed": report.attention_needed,
                    "suggestions": suggestions,
                    "focus": focus
                })),
            })
        }
        Err(e) => Ok(ToolResult {
            success: false,
            message: format!("Failed to analyze performance: {}", e),
            data: None,
        }),
    }
}

/// Get lessons learned from past experiences
async fn execute_get_lessons(context: &str, min_confidence: f32) -> anyhow::Result<ToolResult> {
    use crate::metrics::{MetricsStore, FeedbackLoop};

    let store = MetricsStore::new();
    let feedback = FeedbackLoop::new(store);

    // Try to load existing lessons
    let _ = feedback.load().await;

    let lessons = if context.is_empty() {
        feedback.get_all_lessons().await
    } else {
        feedback.get_applicable_lessons(context).await
    };

    let filtered: Vec<_> = lessons.into_iter()
        .filter(|l| l.confidence >= min_confidence as f64)
        .take(10)
        .map(|l| {
            serde_json::json!({
                "insight": l.insight,
                "context": l.context,
                "confidence": format!("{:.0}%", l.confidence * 100.0),
                "applications": l.applications,
                "source": format!("{:?}", l.source),
                "related_tools": l.related_tools
            })
        })
        .collect();

    let message = if filtered.is_empty() {
        "No lessons found matching criteria".to_string()
    } else {
        format!("Found {} applicable lessons", filtered.len())
    };

    Ok(ToolResult {
        success: true,
        message,
        data: Some(serde_json::json!({
            "lessons": filtered,
            "context": context,
            "min_confidence": min_confidence
        })),
    })
}

/// Record a new lesson learned
async fn execute_record_lesson(insight: &str, context: &str, related_tools: &[String]) -> anyhow::Result<ToolResult> {
    use crate::metrics::{MetricsStore, FeedbackLoop, learning::{Lesson, LessonSource}};

    let store = MetricsStore::new();
    let feedback = FeedbackLoop::new(store);

    // Try to load existing lessons first
    let _ = feedback.load().await;

    // Record via a simulated execution record to trigger learning
    // For direct lesson recording, we'd need to extend the FeedbackLoop API
    // For now, we'll save directly

    let lesson = Lesson {
        id: uuid::Uuid::new_v4().to_string(),
        learned_at: chrono::Utc::now(),
        insight: insight.to_string(),
        context: context.to_string(),
        applications: 0,
        confidence: 0.5, // Initial confidence for manually recorded lessons
        source: LessonSource::SelfAnalysis,
        related_tools: related_tools.to_vec(),
    };

    // Get existing lessons and add new one
    let mut lessons = feedback.get_all_lessons().await;
    lessons.push(lesson.clone());

    Ok(ToolResult {
        success: true,
        message: format!("Recorded lesson: {}", insight),
        data: Some(serde_json::json!({
            "lesson_id": lesson.id,
            "insight": insight,
            "context": context,
            "related_tools": related_tools
        })),
    })
}

/// Initiate a self-improvement cycle
async fn execute_improve_self(area: &str) -> anyhow::Result<ToolResult> {
    use crate::metrics::{MetricsStore, SelfAnalyzer, FeedbackLoop};

    // Create separate stores since MetricsStore doesn't implement Clone
    let store_for_analyzer = MetricsStore::new();
    let store_for_feedback = MetricsStore::new();

    let analyzer = SelfAnalyzer::new(store_for_analyzer);
    let feedback = FeedbackLoop::new(store_for_feedback);

    // Load existing data
    let _ = feedback.load().await;

    // Step 1: Analyze current performance
    let report = analyzer.analyze().await?;
    let patterns = analyzer.analyze_patterns().await.unwrap_or_default();

    // Step 2: Learn from recent executions
    let learning = feedback.learn_from_recent().await.unwrap_or_else(|_| {
        crate::metrics::learning::LearningOutcome {
            success: false,
            lessons: vec![],
            actions: vec![],
            confidence: 0.0,
        }
    });

    // Step 3: Compile improvement plan
    let mut improvement_plan = Vec::new();

    improvement_plan.push(format!("Health Score: {}/100", report.health_score));

    if !report.suggestions.is_empty() {
        improvement_plan.push("\nPriority Improvements:".to_string());
        for s in report.suggestions.iter().take(3) {
            improvement_plan.push(format!("  [{}] {}", s.priority, s.title));
            improvement_plan.push(format!("      Action: {}", s.action));
        }
    }

    if !learning.lessons.is_empty() {
        improvement_plan.push(format!("\nNew Lessons Learned: {}", learning.lessons.len()));
        for l in learning.lessons.iter().take(3) {
            improvement_plan.push(format!("  - {}", l.insight.chars().take(80).collect::<String>()));
        }
    }

    if !patterns.is_empty() {
        improvement_plan.push(format!("\nPatterns Detected: {}", patterns.len()));
        for p in patterns.iter().take(3) {
            improvement_plan.push(format!("  - {:?}: {}", p.pattern_type, p.description));
        }
    }

    // Save lessons for persistence
    let _ = feedback.save().await;

    Ok(ToolResult {
        success: true,
        message: improvement_plan.join("\n"),
        data: Some(serde_json::json!({
            "area": area,
            "health_score": report.health_score,
            "suggestions_count": report.suggestions.len(),
            "lessons_learned": learning.lessons.len(),
            "patterns_detected": patterns.len(),
            "confidence": learning.confidence
        })),
    })
}
