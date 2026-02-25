//! Interactive CLI - Claude Code-like experience with orchestrator integration
//!
//! A clean, minimal interface with tool calling and multi-agent orchestration.

use anyhow::Result;
use std::io::{self, Write, IsTerminal};
use std::sync::Arc;
use std::time::{Instant, Duration};
use crossterm::{execute, style::{Color, Print, ResetColor, SetForegroundColor}};
use indicatif::{ProgressBar, ProgressStyle};
use rustyline::completion::{Completer, Pair};
use rustyline::hint::Hinter;
use rustyline::validate::{Validator, ValidationResult, ValidationContext};
use rustyline::highlight::Highlighter;
use rustyline::Helper;

use crate::agent::llm::{ChatMessage, OpenRouterClient, ToolDefinition, FunctionDefinition};
use crate::agent::conversation;
use crate::agent::tools::{ToolContext, builtin_tools, execute_tool, ToolCall};
use crate::agent::context_manager::{ContextManager, context_config_for_model};
use crate::config::Config as AgentConfig;
use crate::orchestrator::SmartReasoningOrchestrator;
use crate::orchestrator::spawner::AgentSpawner;
use crate::orchestrator::context::SharedContext;
use crate::soul::Personality;
use crate::memory::retrieval::SemanticSearch;
use crate::memory::recursive::{RecursiveContextManager, RecursiveConfig};

/// Keyboard shortcuts available in the CLI
const KEYBOARD_SHORTCUTS: &[(char, &str, &str)] = &[
    ('?', "Show keyboard shortcuts", "help"),
    ('c', "Clear conversation", "/clear"),
    ('m', "Change mode", "/mode"),
    ('h', "Show history", "/history"),
    ('s', "Save conversation", "/save"),
    ('q', "Quit", "/exit"),
];

/// Show keyboard shortcuts help
fn show_keyboard_shortcuts() {
    println!();
    println!("\x1b[1m═══════════════════════════════════════\x1b[0m");
    println!("\x1b[1m  Keyboard Shortcuts\x1b[0m");
    println!("\x1b[1m═══════════════════════════════════════\x1b[0m");
    for (key, desc, _cmd) in KEYBOARD_SHORTCUTS {
        println!("  \x1b[36m{}\x1b[0m  {}", key, desc);
    }
    println!();
    println!("  \x1b[90mTab\x1b[0m  Autocomplete");
    println!("  \x1b[90mCtrl+D\x1b[0m  Exit");
    println!("  \x1b[90mCtrl+C\x1b[0m  Cancel current input");
    println!();
}

/// Custom helper for autocomplete and hints
struct AgentHelper {
    commands: Vec<&'static str>,
    actions: Vec<&'static str>,
}

impl AgentHelper {
    fn new() -> Self {
        Self {
            commands: vec![
                "/help", "/clear", "/history", "/mode", "/model", "/tools",
                "/agents", "/soul", "/heartbeat", "/web", "/save", "/exit", "/quit",
                "/conversations", "/load", "/new", "/context", "/memory",
                "/compact", "/cost", "/init", "/status", "/desktop", "/git", "/skills",
                "/mode chat", "/mode tools", "/mode orchestrate", "/mode plan",
                "/soul edit", "/soul reset", "/soul reload",
            ],
            actions: vec![
                "search for", "find files", "read file", "list files",
                "explore codebase", "analyze", "write", "create",
            ],
        }
    }
}

impl Completer for AgentHelper {
    type Candidate = Pair;

    fn complete(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> rustyline::Result<(usize, Vec<Pair>)> {
        let partial = &line[..pos];

        // Command completion (starts with /)
        if partial.starts_with('/') {
            let matches: Vec<Pair> = self.commands
                .iter()
                .filter(|c| c.starts_with(partial))
                .map(|c| Pair {
                    display: c.to_string(),
                    // Complete from current position
                    replacement: c[partial.len()..].to_string(),
                })
                .collect();
            return Ok((pos, matches));
        }

        // Action completion (for tools mode)
        let lower = partial.to_lowercase();
        let mut action_matches = Vec::new();
        for action in &self.actions {
            if action.starts_with(&lower) && action.len() > lower.len() {
                action_matches.push(Pair {
                    display: action.to_string(),
                    replacement: action[partial.len()..].to_string(),
                });
            }
        }
        if !action_matches.is_empty() {
            return Ok((pos, action_matches));
        }

        Ok((pos, Vec::new()))
    }
}

impl Hinter for AgentHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<Self::Hint> {
        if line.is_empty() || pos < line.len() {
            return None;
        }

        // Command hints - show first matching command
        if line.starts_with('/') {
            if let Some(cmd) = self.commands.iter().find(|c| c.starts_with(line) && **c != line) {
                return Some(cmd[line.len()..].to_string());
            }
        }

        // Tool hints
        let lower = line.to_lowercase();
        let hints = [
            ("search for", " text in files"),
            ("find file", "s by pattern"),
            ("read", " <file>"),
            ("list", " <directory>"),
            ("explore", " codebase"),
            ("analyze", " code"),
        ];

        for (prefix, hint) in hints {
            if lower.starts_with(prefix) && lower.len() == prefix.len() {
                return Some(hint.to_string());
            }
        }

        None
    }
}

impl Validator for AgentHelper {
    fn validate(&self, _ctx: &mut ValidationContext<'_>) -> rustyline::Result<ValidationResult> {
        Ok(ValidationResult::Valid(None))
    }
}

impl Highlighter for AgentHelper {}

impl Helper for AgentHelper {}

/// Session mode
#[derive(Debug, Clone, PartialEq)]
enum Mode {
    /// Simple chat mode
    Chat,
    /// Tool-enabled mode (can use exploration/coding tools)
    Tools,
    /// Orchestrator mode (spawns specialized agents)
    Orchestrate,
    /// Plan mode - shows plan before executing
    Plan,
}

/// Interactive session state
struct Session {
    conversation: conversation::Conversation,
    client: OpenRouterClient,
    model: String,
    mode: Mode,
    start_time: Instant,
    persistent: bool,
    memory_store: Option<Arc<crate::memory::MemoryStore>>,
    semantic_search: Option<SemanticSearch>,
    context_manager: ContextManager,
    recursive_manager: RecursiveContextManager,
    tool_context: ToolContext,
    personality: Personality,
}

impl Session {
    fn new(client: OpenRouterClient, persistent: bool) -> Self {
        let personality = Personality::load().unwrap_or_default();
        let model = AgentConfig::load().unwrap_or_default().models.chat.clone();
        let context_config = context_config_for_model(&model);
        let recursive_manager = RecursiveContextManager::with_config(
            client.clone(),
            RecursiveConfig {
                model: model.clone(),
                ..Default::default()
            },
        );
        Self {
            conversation: conversation::Conversation::new(),
            client,
            model: model.clone(),
            mode: Mode::Tools, // Default to tools mode
            start_time: Instant::now(),
            persistent,
            memory_store: None,
            semantic_search: None,
            context_manager: ContextManager::new(context_config),
            recursive_manager,
            tool_context: ToolContext::with_project_paths(),
            personality,
        }
    }

    fn from_conversation(
        client: OpenRouterClient,
        record: crate::memory::ConversationRecord,
        persistent: bool,
    ) -> Self {
        let personality = Personality::load().unwrap_or_default();
        let model = AgentConfig::load().unwrap_or_default().models.chat.clone();
        let context_config = context_config_for_model(&model);
        let recursive_manager = RecursiveContextManager::with_config(
            client.clone(),
            RecursiveConfig {
                model: model.clone(),
                ..Default::default()
            },
        );
        Self {
            conversation: conversation::Conversation::from_record(record),
            client,
            model: model.clone(),
            mode: Mode::Tools,
            start_time: Instant::now(),
            persistent,
            memory_store: None,
            semantic_search: None,
            context_manager: ContextManager::new(context_config),
            recursive_manager,
            tool_context: ToolContext::with_project_paths(),
            personality,
        }
    }

    /// Initialize memory store and semantic search
    async fn init_memory(&mut self) -> Result<()> {
        if !self.persistent {
            return Ok(());
        }

        match crate::memory::MemoryStore::default_store().await {
            Ok(store) => {
                let store_arc = Arc::new(store);
                self.semantic_search = Some(SemanticSearch::new(store_arc.clone()));
                self.memory_store = Some(store_arc);
                Ok(())
            }
            Err(e) => {
                tracing::warn!("Could not initialize memory store: {}", e);
                Err(e)
            }
        }
    }

    /// Get relevant memory context for the current query
    async fn get_memory_context(&self, query: &str) -> Option<String> {
        if let Some(ref search) = self.semantic_search {
            match search.get_context(query, 5).await {
                Ok(context) => {
                    if !context.context_text.is_empty() {
                        return Some(context.to_prompt_context());
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to get memory context: {}", e);
                }
            }
        }
        None
    }

    async fn save(&self) -> Result<()> {
        if let Some(ref store) = self.memory_store {
            store.save_conversation(&self.conversation.to_record()).await?;
        }
        Ok(())
    }
}

/// Print colored output
fn print_colored(text: &str, color: Color) {
    let _ = execute!(
        io::stdout(),
        SetForegroundColor(color),
        Print(text),
        ResetColor
    );
}

/// Print a dimmed line
fn print_dim(text: &str) {
    print_colored(text, Color::DarkGrey);
}

/// Print a success message
fn print_success(text: &str) {
    print_colored(text, Color::Green);
}

/// Print an info message
fn print_info(text: &str) {
    print_colored(text, Color::Cyan);
}

/// Print an error message
fn print_error(text: &str) {
    print_colored(text, Color::Red);
}

/// Print a header
fn print_header(text: &str) {
    print_colored(&format!("\n{}\n", text), Color::Cyan);
}

/// Print the welcome banner
fn print_banner(name: &str, model: &str, mode: &Mode) {
    let mode_str = match mode {
        Mode::Chat => "chat",
        Mode::Tools => "tools",
        Mode::Orchestrate => "orchestrate",
        Mode::Plan => "plan",
    };
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_string());

    println!();
    println!("  \x1b[1m{} v0.1.0\x1b[0m", name);
    println!();
    println!("  \x1b[90mmodel\x1b[0m    \x1b[36m{}\x1b[0m", model);
    println!("  \x1b[90mmode\x1b[0m     \x1b[32m{}\x1b[0m", mode_str);
    println!("  \x1b[90mcwd\x1b[0m      {}", cwd);
    println!();
    println!("  \x1b[90m/help for commands · Tab for autocomplete\x1b[0m");
    println!();
}

/// Print help text
fn print_help() {
    print_header("Commands");
    println!("  /help          Show this help");
    println!("  /clear         Start a new conversation");
    println!("  /history       Show conversation history");
    println!("  /mode          Change mode: chat, tools, orchestrate, plan");
    println!("  /model         Change the model");
    println!("  /tools         List available tools");
    println!("  /agents        Show current agents");
    println!("  /soul          View/edit personality");
    println!("  /heartbeat     Check soul status");
    println!("  /web <url>     Fetch web content");
    println!("  /desktop       Enable desktop automation mode (pre-approve all desktop tools)");
    println!("  /git           Enable git mode (pre-approve all shell commands)");
    println!("  /skills        List available and created skills");
    println!("  /save          Save conversation");
    println!("  /exit          Exit session");
    println!();
    print_header("Conversation Management");
    println!("  /conversations   List saved conversations");
    println!("  /load <id>       Load a saved conversation");
    println!("  /new             Start new conversation");
    println!("  /context         Show context/token usage");
    println!("  /memory          Show memory statistics");
    println!();
    print_header("Keyboard Shortcuts");
    println!("  ?              Show keyboard shortcuts");
    println!("  c              Clear conversation");
    println!("  m              Change mode");
    println!("  h              Show history");
    println!("  s              Save conversation");
    println!("  q              Quit");
    println!();
    print_header("Modes");
    println!("  chat         - Simple conversation (no tools)");
    println!("  tools        - Tool-enabled (search, read, write files)");
    println!("  orchestrate  - Spawn specialized agents for complex tasks");
    println!("  plan         - Show plan before executing, requires approval");
    println!();
    print_header("Soul/Personality");
    println!("  /soul              Show current personality");
    println!("  /soul edit         Edit personality file");
    println!("  /soul reset        Reset to default");
    println!("  /soul reload       Reload from file");
    println!();
}

/// Print mode-specific help as a single dim line
fn print_mode_help(mode: &Mode) {
    let hint = match mode {
        Mode::Chat => "chat — simple conversation, no tools.",
        Mode::Tools => "tools — search, read, write files. Type / for commands.",
        Mode::Orchestrate => "orchestrate — spawns specialized agents for complex tasks.",
        Mode::Plan => "plan — shows a plan before executing, requires approval.",
    };
    println!("  \x1b[90m{}\x1b[0m", hint);
    println!();
}

/// Print a compact color-coded status line
fn print_status(session: &Session) {
    let mode_str = match session.mode {
        Mode::Chat => "chat",
        Mode::Tools => "tools",
        Mode::Orchestrate => "orchestrate",
        Mode::Plan => "plan",
    };

    println!("  \x1b[36m{}\x1b[0m  \x1b[32m{}\x1b[0m  \x1b[90m{}\x1b[0m",
        session.model, mode_str, &session.conversation.id[..8]);
}

/// Create an animated thinking spinner with a random verb
fn create_thinking_spinner() -> ProgressBar {
    let verbs = ["Thinking", "Reasoning", "Analyzing", "Working", "Processing"];
    let verb = verbs[std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as usize % verbs.len()];
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("  {spinner:.dim} {msg}")
            .unwrap(),
    );
    pb.set_message(format!("{}...", verb));
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

/// Format a tool call as a compact one-line summary: "tool_name arg_preview"
fn format_tool_call(name: &str, args: &serde_json::Value) -> String {
    let preview = match name {
        "read_file" | "write_file" | "file_info" | "delete_file" | "append_file" =>
            args.get("path").and_then(|v| v.as_str()),
        "execute_command" =>
            args.get("command").and_then(|v| v.as_str()),
        "search_content" =>
            args.get("pattern").and_then(|v| v.as_str()),
        "fetch_url" =>
            args.get("url").and_then(|v| v.as_str()),
        "list_directory" =>
            args.get("path").and_then(|v| v.as_str()),
        "find_files" =>
            args.get("name_pattern").and_then(|v| v.as_str()),
        "glob" =>
            args.get("pattern").and_then(|v| v.as_str()),
        "spawn_subagent" | "orchestrate_task" =>
            args.get("agent_type").and_then(|v| v.as_str()),
        "create_directory" =>
            args.get("path").and_then(|v| v.as_str()),
        "use_skill" =>
            args.get("skill_id").and_then(|v| v.as_str()),
        "search_learnings" =>
            args.get("query").and_then(|v| v.as_str()),
        "record_learning" | "record_lesson" =>
            args.get("content").and_then(|v| v.as_str()),
        "self_diagnose" | "self_repair" =>
            args.get("issue").or_else(|| args.get("issue_type")).and_then(|v| v.as_str()),
        "analyze_performance" =>
            args.get("focus").and_then(|v| v.as_str()),
        _ => None,
    };
    match preview {
        Some(p) if p.len() > 50 => format!("{} {}...", name, &p[..47]),
        Some(p) => format!("{} {}", name, p),
        None => name.to_string(),
    }
}

/// Get suggestions based on partial input
fn get_suggestions(input: &str, mode: &Mode, personality: &Personality) -> Vec<String> {
    let mut suggestions = Vec::new();
    let lower = input.to_lowercase();

    // Slash command suggestions
    if input.starts_with('/') {
        let commands = ["/help", "/clear", "/history", "/mode", "/model", "/tools",
                       "/agents", "/soul", "/heartbeat", "/web", "/save", "/exit"];
        for cmd in commands {
            if cmd.starts_with(input) && cmd != input {
                suggestions.push(cmd.to_string());
            }
        }
        return suggestions;
    }

    // Tool suggestions based on input keywords
    if *mode == Mode::Tools {
        let tool_keywords = [
            ("search for", "search_content - Search for text in files"),
            ("find file", "find_files - Find files by name pattern"),
            ("list files", "glob - List files matching pattern"),
            ("read", "read_file - Read a file's contents"),
            ("show file", "read_file - Read a file's contents"),
            ("where is", "search_content - Find where something is defined"),
            ("explore", "search_content - Explore the codebase"),
        ];

        for (keyword, tool) in tool_keywords {
            if lower.contains(keyword) {
                suggestions.push(tool.to_string());
            }
        }
    }

    // Mode-based suggestions
    if *mode == Mode::Orchestrate && lower.len() > 10 {
        if lower.contains("write") || lower.contains("create") || lower.contains("implement") {
            suggestions.push("💡 This looks like a coding task - spawn code agent?".to_string());
        }
        if lower.contains("search") || lower.contains("find") || lower.contains("research") {
            suggestions.push("💡 This looks like a research task - spawn research agent?".to_string());
        }
    }

    // Personality-based suggestions
    if !personality.preferred_skills.is_empty() && lower.contains("use skill") {
        for skill in &personality.preferred_skills {
            suggestions.push(format!("  → {}", skill));
        }
    }

    suggestions
}

/// Print suggestions in a dimmed style
fn print_suggestions(suggestions: &[String]) {
    if suggestions.is_empty() {
        return;
    }
    print_dim("  Suggestions: ");
    println!("{}", suggestions.iter().take(3).cloned().collect::<Vec<_>>().join(" | "));
}

/// Create a spinner for agent activity, returns ProgressBar to finish later
fn create_agent_spinner(capability: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("  {spinner:.dim} {msg}")
            .unwrap(),
    );
    pb.set_message(format!("{} agent working...", capability));
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

/// Render markdown text with ANSI colors for terminal
fn render_markdown(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    let mut in_code_block = false;
    let mut in_inline_code = false;

    while let Some(ch) = chars.next() {
        // Handle code blocks (```)
        if ch == '`' && chars.peek() == Some(&'`') && chars.nth(1) == Some('`') {
            if in_code_block {
                in_code_block = false;
                result.push_str("\x1b[0m```\n");
            } else {
                in_code_block = true;
                result.push_str("```\n\x1b[36m"); // Cyan for code
            }
            continue;
        }

        if in_code_block {
            result.push(ch);
            continue;
        }

        // Handle inline code (`code`)
        if ch == '`' {
            if in_inline_code {
                in_inline_code = false;
                result.push_str("\x1b[0m`");
            } else {
                in_inline_code = true;
                result.push_str("`");
                result.push_str("\x1b[36m"); // Cyan for inline code
            }
            continue;
        }

        if in_inline_code {
            result.push(ch);
            continue;
        }

        // Handle bold (**text**)
        if ch == '*' && chars.peek() == Some(&'*') {
            chars.next(); // consume second *
            // Look ahead for closing **
            let mut lookahead = chars.clone();
            let mut found_close = false;
            let mut inner = String::new();
            while let Some(c) = lookahead.next() {
                if c == '*' && lookahead.peek() == Some(&'*') {
                    lookahead.next();
                    found_close = true;
                    break;
                }
                inner.push(c);
            }
            if found_close {
                result.push_str("\x1b[1m"); // Bold
                result.push_str(&inner);
                result.push_str("\x1b[0m");
                chars = lookahead;
                continue;
            } else {
                result.push_str("**");
                continue;
            }
        }

        // Handle italic (*text* or _text_)
        if ch == '*' || ch == '_' {
            let close_char = ch;
            let mut lookahead = chars.clone();
            let mut found_close = false;
            let mut inner = String::new();
            while let Some(c) = lookahead.next() {
                if c == close_char {
                    found_close = true;
                    break;
                }
                inner.push(c);
            }
            if found_close && !inner.is_empty() && !inner.contains(close_char) {
                result.push_str("\x1b[3m"); // Italic
                result.push_str(&inner);
                result.push_str("\x1b[0m");
                chars = lookahead;
                continue;
            }
        }

        result.push(ch);
    }

    result
}

/// Print text with markdown rendering
fn print_markdown(text: &str) {
    print!("{}", render_markdown(text));
    let _ = io::stdout().flush();
}

/// Check if a line is a markdown table line (starts with | or │, contains | separators)
fn is_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    (trimmed.starts_with('|') || trimmed.starts_with('│') || trimmed.starts_with("├"))
        && (trimmed.contains('|') || trimmed.contains('│'))
}

/// Check if a line is a table separator (|---|---|)
fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    is_table_line(trimmed) && (trimmed.contains("---") || trimmed.contains("───") || trimmed.contains("─"))
}

/// Normalize a table line: convert box-drawing chars to pipes for uniform parsing
fn normalize_table_line(line: &str) -> String {
    line.replace('│', "|").replace("├", "|").replace("┼", "|").replace("┤", "|")
}

/// Format markdown text with ANSI colors for terminal display
fn format_markdown(text: &str) -> String {
    // Pre-pass: detect table blocks and render them, then inline-format the rest
    let lines: Vec<&str> = text.lines().collect();
    let mut result = String::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Detect table block start
        if is_table_line(line) {
            let mut table_lines: Vec<String> = Vec::new();
            while i < lines.len() && is_table_line(lines[i]) {
                if !is_table_separator(lines[i]) {
                    table_lines.push(normalize_table_line(lines[i]));
                }
                i += 1;
            }
            if !table_lines.is_empty() {
                result.push_str(&format_table(&table_lines));
            }
            continue;
        }

        // Non-table line: apply inline formatting
        result.push_str(&format_inline(line));
        result.push('\n');
        i += 1;
    }

    // Remove trailing newline if the original didn't have one
    if !text.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Format inline markdown (bold, code, inline code) for a single line
fn format_inline(line: &str) -> String {
    let mut result = String::new();
    let mut chars = line.chars().peekable();
    let mut in_code_block = false;
    let mut in_inline_code = false;
    let mut in_bold = false;
    let mut pending_stars = 0;

    while let Some(ch) = chars.next() {
        // Handle code blocks (```)
        if ch == '`' && chars.peek() == Some(&'`') {
            chars.next();
            if chars.peek() == Some(&'`') {
                chars.next();
                in_code_block = !in_code_block;
                if in_code_block {
                    result.push_str("\x1b[90m"); // Gray for code blocks
                } else {
                    result.push_str("\x1b[0m");
                }
                continue;
            }
        }

        if in_code_block {
            result.push(ch);
            continue;
        }

        // Handle inline code
        if ch == '`' {
            if in_inline_code {
                in_inline_code = false;
                result.push_str("\x1b[0m");
            } else {
                in_inline_code = true;
                result.push_str("\x1b[36m");
            }
            continue;
        }

        if in_inline_code {
            result.push(ch);
            continue;
        }

        // Handle bold (**text**)
        if ch == '*' {
            pending_stars += 1;
            if pending_stars == 2 {
                pending_stars = 0;
                if in_bold {
                    in_bold = false;
                    result.push_str("\x1b[0m");
                } else {
                    in_bold = true;
                    result.push_str("\x1b[1m");
                }
            }
            continue;
        } else if pending_stars > 0 {
            for _ in 0..pending_stars {
                result.push('*');
            }
            pending_stars = 0;
        }

        // Handle headers (# at start of line)
        if ch == '#' && result.is_empty() {
            let mut level = 1;
            while chars.peek() == Some(&'#') {
                chars.next();
                level += 1;
            }
            if chars.peek() == Some(&' ') {
                chars.next();
            }
            // Bold for headers
            result.push_str("\x1b[1m");
            let rest: String = chars.collect();
            result.push_str(&rest);
            result.push_str("\x1b[0m");
            return result;
        }

        result.push(ch);
    }

    for _ in 0..pending_stars {
        result.push('*');
    }

    if in_bold || in_inline_code || in_code_block {
        result.push_str("\x1b[0m");
    }

    result
}

/// Strip markdown formatting from a cell value for width calculation
fn strip_markdown(text: &str) -> String {
    text.replace("**", "").replace("*", "").replace("`", "")
        .replace("✅", "Y").replace("❌", "N")  // normalize emoji widths
}

/// Format a markdown table for terminal display
fn format_table(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }

    let mut result = String::new();

    // Parse cells from each line
    let rows: Vec<Vec<String>> = lines.iter().map(|line| {
        line.trim_matches('|')
            .split('|')
            .map(|s| s.trim().to_string())
            .collect()
    }).collect();

    // Calculate column widths using stripped text (without markdown formatting)
    let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut widths = vec![0usize; col_count];

    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                widths[i] = widths[i].max(strip_markdown(cell).len());
            }
        }
    }

    // Print header (bold)
    if let Some(header) = rows.first() {
        result.push_str("  \x1b[1m"); // Bold, indented
        for (i, cell) in header.iter().enumerate() {
            let width = widths.get(i).copied().unwrap_or(0);
            let stripped = strip_markdown(cell);
            let padding = width.saturating_sub(stripped.len());
            result.push_str(&format!("│ {}{} ", format_inline(cell), " ".repeat(padding)));
        }
        result.push_str("│\x1b[0m\n");
    }

    // Print separator
    result.push_str("  ");
    for (i, width) in widths.iter().enumerate() {
        if i == 0 {
            result.push_str("├");
        }
        result.push_str(&"─".repeat(width + 2));
        if i < widths.len() - 1 {
            result.push_str("┼");
        } else {
            result.push_str("┤");
        }
    }
    result.push('\n');

    // Print data rows
    for row in rows.iter().skip(1) {
        result.push_str("  ");
        for (i, cell) in row.iter().enumerate() {
            let width = widths.get(i).copied().unwrap_or(0);
            let stripped = strip_markdown(cell);
            let padding = width.saturating_sub(stripped.len());
            result.push_str(&format!("│ {}{} ", format_inline(cell), " ".repeat(padding)));
        }
        result.push_str("│\n");
    }

    result
}

/// Detect if a task needs orchestration
fn needs_orchestration(input: &str) -> bool {
    let lower = input.to_lowercase();

    // Keywords that suggest complex multi-step tasks
    let orchestration_keywords = [
        "write a", "create a", "build a", "implement a", "develop a",
        "analyze the codebase", "analyze this project",
        "refactor", "restructure",
        "write tests for", "add tests for",
        "create a rest api", "build an api",
        "multi-step", "several tasks",
    ];

    orchestration_keywords.iter().any(|k| lower.contains(k))
}

/// Detect if a task needs tools
fn needs_tools(input: &str) -> bool {
    let lower = input.to_lowercase();

    let tool_keywords = [
        "search", "find", "look for", "grep",
        "read file", "open file", "show file",
        "list files", "list directory", "show directory",
        "explore", "what files", "what's in",
        "search for", "find all",
    ];

    tool_keywords.iter().any(|k| lower.contains(k))
}

/// Handle slash commands, returns true if should continue
/// Resolve a partial slash command to the full command via prefix matching.
/// Returns the original input if no unique match is found.
fn resolve_command(input: &str) -> String {
    let parts: Vec<&str> = input.split_whitespace().collect();
    let cmd = parts.first().copied().unwrap_or("");

    // All known slash commands
    let commands = [
        "/", "/commands", "/help", "/clear", "/new", "/mode", "/model",
        "/tools", "/agents", "/soul", "/heartbeat", "/web", "/save",
        "/history", "/exit", "/conversations", "/load", "/context",
        "/memory", "/compact", "/cost", "/init", "/status", "/desktop", "/git", "/skills",
    ];

    // Exact match — return as-is
    if commands.contains(&cmd) {
        return input.to_string();
    }

    // Prefix match — find all commands that start with the input
    let matches: Vec<&&str> = commands.iter()
        .filter(|c| c.starts_with(cmd) && **c != "/")
        .collect();

    if matches.len() == 1 {
        // Unique prefix match — substitute the resolved command
        let mut resolved = matches[0].to_string();
        if parts.len() > 1 {
            resolved.push(' ');
            resolved.push_str(&parts[1..].join(" "));
        }
        resolved
    } else {
        // Ambiguous or no match — return as-is, handle_command will show error
        input.to_string()
    }
}

async fn handle_command(cmd: &str, session: &mut Session) -> Result<bool> {
    let resolved = resolve_command(cmd);
    let parts: Vec<&str> = resolved.split_whitespace().collect();
    let command = parts.first().unwrap_or(&"");

    match *command {
        "/" | "/commands" => {
            println!();
            print_dim("═══ Commands ═══");
            println!();
            println!("  /help              Show detailed help");
            println!("  /clear             Start new conversation");
            println!("  /mode <mode>       Switch mode (chat|tools|orchestrate|plan)");
            println!("  /model <name>      Change model");
            println!("  /tools             List available tools");
            println!("  /agents            Show agent roles");
            println!("  /soul              View/edit personality");
            println!("  /web <url>         Fetch web content");
            println!("  /compact           Compact conversation (save tokens)");
            println!("  /cost              Show session cost estimate");
            println!("  /init              Scan and inject project context");
            println!("  /desktop           Pre-approve all desktop automation tools");
            println!("  /git               Pre-approve all git/shell commands");
            println!("  /skills            List available skills");
            println!("  /status            Show model, mode, context usage");
            println!("  /save              Save conversation");
            println!("  /history           Show history");
            println!("  /exit              Exit session");
            println!();
            print_dim("Press Tab after / for autocomplete");
            println!();
        }
        "/help" | "/?" => {
            print_help();
        }
        "/clear" | "/new" => {
            if session.persistent {
                session.save().await?;
            }
            session.conversation = conversation::Conversation::new();
            session.conversation.add_message(
                conversation::Role::System,
                get_system_prompt(&session)
            );
            session.start_time = Instant::now();
            print_success("✓ Started new conversation");
            println!();
            print_mode_help(&session.mode);
        }
        "/mode" => {
            if parts.len() > 1 {
                match parts[1] {
                    "chat" => {
                        session.mode = Mode::Chat;
                        print_success("✓ Switched to chat mode");
                    }
                    "tools" => {
                        session.mode = Mode::Tools;
                        print_success("✓ Switched to tools mode");
                    }
                    "orchestrate" => {
                        session.mode = Mode::Orchestrate;
                        print_success("✓ Switched to orchestrate mode");
                    }
                    "plan" => {
                        session.mode = Mode::Plan;
                        print_success("✓ Switched to plan mode");
                    }
                    _ => {
                        print_error("Unknown mode. Use: chat, tools, orchestrate, plan");
                    }
                }
                println!();
                print_status(&session);
                println!();
                print_mode_help(&session.mode);
            } else {
                println!("Current mode: {:?}", session.mode);
                println!("Usage: /mode <chat|tools|orchestrate|plan>");
            }
        }
        "/history" => {
            print_header("Conversation History");
            for msg in &session.conversation.messages {
                match msg.role {
                    conversation::Role::User => {
                        print_colored("You: ", Color::Green);
                        println!("{}", msg.content);
                    }
                    conversation::Role::Assistant => {
                        print_colored("Assistant: ", Color::Blue);
                        println!("{}", msg.content);
                    }
                    conversation::Role::System => {
                        print_dim(&format!("[System: {}]", msg.content));
                        println!();
                    }
                }
            }
            println!();
        }
        "/model" => {
            if parts.len() > 1 {
                session.model = parts[1].to_string();
                print_success(&format!("✓ Model changed to: {}", session.model));
                println!();
            } else {
                println!("Current model: {}", session.model);
                println!("Usage: /model <model_id>");
            }
        }
        "/tools" => {
            print_header("Available Tools");
            let tools = builtin_tools();
            for tool in tools {
                println!("  {} - {}", tool.name, tool.description.lines().next().unwrap_or(""));
            }
            println!();
        }
        "/agents" => {
            print_header("Agent Roles");
            let config = AgentConfig::load().unwrap_or_default();
            for role in crate::config::ModelsConfig::roles() {
                if let Some(model) = config.models.get(role) {
                    println!("  {:15} → {}", role, model);
                }
            }
            println!();
        }
        "/save" => {
            if session.persistent {
                session.save().await?;
                print_success(&format!("✓ Saved: {}", session.conversation.id));
                println!();
            } else {
                print_dim("Persistence not enabled. Use -P flag.");
                println!();
            }
        }
        "/conversations" | "/convos" => {
            if !session.persistent {
                print_dim("Persistence not enabled. Use -P flag.");
                println!();
            } else if let Some(ref store) = session.memory_store {
                print_header("Recent Conversations");
                match store.list_conversations(10, 0).await {
                    Ok(convs) => {
                        if convs.is_empty() {
                            println!("  No saved conversations found.");
                        } else {
                            for (i, conv) in convs.iter().enumerate() {
                                let title = conv.title.as_deref().unwrap_or("Untitled");
                                let msg_count = conv.messages.len();
                                let date = conv.updated_at.format("%Y-%m-%d %H:%M");
                                let current = if conv.id == session.conversation.id {
                                    " (current)"
                                } else {
                                    ""
                                };
                                println!("  {}. {} [{}] - {} msgs, {}{}",
                                    i + 1,
                                    title,
                                    &conv.id[..8],
                                    msg_count,
                                    date,
                                    current
                                );
                            }
                            println!();
                            print_dim("Use /load <id> to load a conversation");
                            println!();
                        }
                    }
                    Err(e) => {
                        print_error(&format!("Failed to list conversations: {}", e));
                    }
                }
                println!();
            }
        }
        "/load" => {
            if !session.persistent {
                print_dim("Persistence not enabled. Use -P flag.");
                println!();
            } else if parts.len() < 2 {
                print_dim("Usage: /load <conversation-id>");
                println!();
                print_dim("Use /conversations to list available IDs");
                println!();
            } else if let Some(ref store) = session.memory_store {
                let id = parts[1];
                match store.load_conversation(id).await {
                    Ok(Some(record)) => {
                        // Save current conversation first
                        let _ = session.save().await;

                        // Load the new conversation
                        session.conversation = conversation::Conversation::from_record(record);
                        session.context_manager.clear_cache().await;

                        print_success(&format!("✓ Loaded: {}", id));
                        println!("  {} messages loaded", session.conversation.messages.len());
                        println!();
                    }
                    Ok(None) => {
                        print_error(&format!("Conversation not found: {}", id));
                        println!();
                    }
                    Err(e) => {
                        print_error(&format!("Failed to load: {}", e));
                        println!();
                    }
                }
            }
        }
        "/context" | "/tokens" => {
            let tokens = ContextManager::estimate_message_tokens(
                &session.conversation.messages.iter().map(|m| ChatMessage {
                    role: Some(serde_json::json!(match m.role {
                        conversation::Role::User => "user",
                        conversation::Role::Assistant => "assistant",
                        conversation::Role::System => "system",
                    })),
                    content: Some(serde_json::json!(m.content.clone())),
                    reasoning_details: None,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    reasoning: None,
                    refusal: None,
                }).collect::<Vec<_>>()
            );
            let limit = session.context_manager.config.model_context_limit;
            let pct = (tokens as f64 / limit as f64 * 100.0) as usize;

            print_header("Context Status");
            println!("  Model: {}", session.model);
            println!("  Context limit: {} tokens", limit);
            println!("  Current usage: {} tokens ({}%)", tokens, pct);
            println!("  Messages: {}", session.conversation.messages.len());

            if let Some(summary) = session.context_manager.get_summary_stats().await {
                println!();
                println!("  Summary cache:");
                println!("    {} messages compressed", summary.messages_compressed);
                println!("    {} tokens saved", summary.original_tokens - summary.summary_tokens);
            }

            if tokens > session.context_manager.config.warning_threshold {
                println!();
                print_dim("⚠️ Context approaching limit - will auto-summarize soon");
            }
            println!();
        }
        "/memory" => {
            if !session.persistent {
                print_dim("Persistence not enabled. Use -P flag.");
                println!();
            } else if let Some(ref store) = session.memory_store {
                match store.stats().await {
                    Ok(stats) => {
                        print_header("Memory Statistics");
                        println!("  Conversations: {}", stats.total_conversations);
                        println!("  Total messages: {}", stats.total_messages);
                        println!("  Knowledge entries: {}", stats.total_knowledge);
                        println!("  With embeddings: {}", stats.conversations_with_embeddings);
                        if let Some(oldest) = stats.oldest_conversation {
                            println!("  Oldest: {}", oldest);
                        }
                        if let Some(newest) = stats.newest_conversation {
                            println!("  Newest: {}", newest);
                        }
                    }
                    Err(e) => {
                        print_error(&format!("Failed to get stats: {}", e));
                    }
                }
                println!();
            }
        }
        "/soul" | "/personality" => {
            // Reload from file to show changes
            match Personality::load() {
                Ok(p) => {
                    session.personality = p;
                    print_header("Agent Soul/Personality");
                    println!("  Name: {}", session.personality.name);
                    println!("  Traits: {}", session.personality.traits.join(", "));
                    println!("  Style: {} {}", session.personality.style.formality, session.personality.style.length);
                    println!();
                    println!("System Prompt:");
                    println!("  {}", session.personality.system_prompt.chars().take(100).collect::<String>().trim_end());
                    if session.personality.system_prompt.len() > 100 {
                        println!("  ...");
                    }
                    println!();
                    println!("Commands:");
                    println!("  /soul edit    - Edit personality file");
                    println!("  /soul reset   - Reset to default personality");
                    println!("  /soul reload  - Reload from file");
                    println!();
                }
                Err(e) => {
                    print_error(&format!("Failed to load personality: {}", e));
                    println!();
                }
            }
        }
        "/soul edit" | "/personality edit" => {
            let path = dirs::data_local_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("my-agent")
                .join("soul")
                .join("personality.toml");

            println!("Personality file: {}", path.display());
            println!();
            println!("Edit the file to update your agent's personality:");
            println!("  - name: Agent's name");
            println!("  - traits: List of personality traits");
            println!("  - system_prompt: How the agent behaves");
            println!("  - style: Communication style settings");
            println!();
            println!("After editing, use /soul reload to apply changes.");
        }
        "/soul reset" | "/personality reset" => {
            session.personality = Personality::default();
            session.personality.save()?;
            print_success("✓ Personality reset to default");
            println!();
        }
        "/soul reload" | "/personality reload" => {
            match Personality::load() {
                Ok(p) => {
                    session.personality = p;
                    print_success("✓ Personality reloaded from file");
                    println!();
                }
                Err(e) => {
                    print_error(&format!("Failed to load personality: {}", e));
                    println!();
                }
            }
        }
        "/heartbeat" | "/pulse" => {
            match crate::soul::get_soul_stats().await {
                Some(stats) => {
                    print_header("Heartbeat Status");
                    println!("  State: {}", stats.state);
                    println!("  Actions: {}", stats.proactive_actions_registered);
                    println!("  Uptime: {}s", stats.uptime_secs);
                }
                None => {
                    print_dim("Heartbeat not running");
                    println!();
                    println!("Start with: my-agent soul start");
                }
            }
            println!();
        }
        "/web" | "/browse" => {
            if parts.len() > 1 {
                let url = parts[1];
                print_info(&format!("Fetching: {}...", url));
                println!();

                // Use a simple HTTP client
                match reqwest::get(url).await {
                    Ok(resp) => {
                        let text = resp.text().await.unwrap_or_default();
                        // Truncate for display
                        let preview: String = text.chars().take(500).collect();
                        println!("{}", preview);
                        if text.len() > 500 {
                            println!("\n... (truncated, {} total chars)", text.len());
                        }
                    }
                    Err(e) => {
                        print_error(&format!("Failed: {}", e));
                    }
                }
                println!();
            } else {
                println!("Usage: /web <url>");
            }
        }
        "/compact" => {
            let msgs: Vec<ChatMessage> = session.conversation.messages.iter().map(|m| ChatMessage {
                role: Some(serde_json::json!(match m.role {
                    conversation::Role::User => "user",
                    conversation::Role::Assistant => "assistant",
                    conversation::Role::System => "system",
                })),
                content: Some(serde_json::json!(m.content.clone())),
                reasoning_details: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning: None,
                refusal: None,
            }).collect();

            let before_tokens = ContextManager::estimate_message_tokens(&msgs);
            let keep_recent = 4;

            if msgs.len() > keep_recent + 2 {
                let system_msg = msgs.first().cloned();
                let middle = &msgs[1..msgs.len().saturating_sub(keep_recent)];
                let recent: Vec<_> = msgs[msgs.len().saturating_sub(keep_recent)..].to_vec();

                print_dim("🔄 Recursively compressing conversation...");
                println!();

                match session.recursive_manager.process_conversation(middle).await {
                    Ok(result) => {
                        let recent_tokens = ContextManager::estimate_message_tokens(&recent);
                        let after_tokens = result.final_tokens + recent_tokens;

                        // Rebuild conversation from compressed result
                        session.conversation = conversation::Conversation::new();

                        if let Some(sys) = system_msg {
                            let content = sys.content.as_ref()
                                .and_then(|c| c.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !content.is_empty() {
                                session.conversation.add_message(conversation::Role::System, content);
                            }
                        }

                        // Add compressed summary as system context
                        session.conversation.add_message(
                            conversation::Role::System,
                            format!("[Prior conversation summary]\n\n{}", result.final_summary),
                        );

                        // Re-add recent messages
                        for msg in &recent {
                            let role_str = msg.role.as_ref()
                                .and_then(|r| r.as_str())
                                .unwrap_or("user");
                            let content = msg.content.as_ref()
                                .and_then(|c| c.as_str())
                                .unwrap_or("")
                                .to_string();
                            let role = match role_str {
                                "system" => conversation::Role::System,
                                "assistant" => conversation::Role::Assistant,
                                _ => conversation::Role::User,
                            };
                            session.conversation.add_message(role, content);
                        }

                        print_success(&format!(
                            "Compressed: {:.1}x ({} → {} tokens, depth {})",
                            result.compression_ratio, before_tokens, after_tokens, result.depth_reached
                        ));
                    }
                    Err(e) => {
                        print_error(&format!("Compression failed: {}", e));
                    }
                }
            } else {
                print_dim("Conversation too short to compress.");
            }
            println!();
        }
        "/cost" => {
            let msgs: Vec<ChatMessage> = session.conversation.messages.iter().map(|m| ChatMessage {
                role: Some(serde_json::json!(match m.role {
                    conversation::Role::User => "user",
                    conversation::Role::Assistant => "assistant",
                    conversation::Role::System => "system",
                })),
                content: Some(serde_json::json!(m.content.clone())),
                reasoning_details: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning: None,
                refusal: None,
            }).collect();

            let tokens = ContextManager::estimate_message_tokens(&msgs);
            let user_msgs = session.conversation.messages.iter()
                .filter(|m| matches!(m.role, conversation::Role::User)).count();
            let asst_msgs = session.conversation.messages.iter()
                .filter(|m| matches!(m.role, conversation::Role::Assistant)).count();
            let elapsed = session.start_time.elapsed();

            print_header("Session Cost Estimate");
            println!("  Model: {}", session.model);
            println!("  Duration: {}m {}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60);
            println!("  Messages: {} user, {} assistant", user_msgs, asst_msgs);
            println!("  Est. tokens: ~{}", tokens);
            println!();
        }
        "/init" => {
            print_info("Scanning project structure...");
            println!();

            let cwd = std::env::current_dir().unwrap_or_default();
            let mut project_info = Vec::new();

            // Detect project type
            if cwd.join("Cargo.toml").exists() {
                project_info.push("Rust project (Cargo.toml)".to_string());
            }
            if cwd.join("package.json").exists() {
                project_info.push("Node.js project (package.json)".to_string());
            }
            if cwd.join("pyproject.toml").exists() || cwd.join("setup.py").exists() {
                project_info.push("Python project".to_string());
            }
            if cwd.join("go.mod").exists() {
                project_info.push("Go project".to_string());
            }

            // Count files
            let mut file_count = 0;
            let mut dir_count = 0;
            if let Ok(entries) = std::fs::read_dir(&cwd) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        dir_count += 1;
                    } else {
                        file_count += 1;
                    }
                }
            }

            // Inject context
            let context = format!(
                "Project directory: {}\nType: {}\nStructure: {} files, {} directories at root",
                cwd.display(),
                if project_info.is_empty() { "Unknown".to_string() } else { project_info.join(", ") },
                file_count,
                dir_count,
            );

            session.conversation.add_message(
                conversation::Role::System,
                format!("Project context initialized:\n{}", context),
            );

            print_success(&format!("Project: {}", cwd.display()));
            if !project_info.is_empty() {
                println!("  Type: {}", project_info.join(", "));
            }
            println!("  {} files, {} directories", file_count, dir_count);
            println!();
        }
        "/status" => {
            let msgs: Vec<ChatMessage> = session.conversation.messages.iter().map(|m| ChatMessage {
                role: Some(serde_json::json!(match m.role {
                    conversation::Role::User => "user",
                    conversation::Role::Assistant => "assistant",
                    conversation::Role::System => "system",
                })),
                content: Some(serde_json::json!(m.content.clone())),
                reasoning_details: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning: None,
                refusal: None,
            }).collect();

            let tokens = ContextManager::estimate_message_tokens(&msgs);
            let limit = session.context_manager.config.model_context_limit;
            let pct = (tokens as f64 / limit as f64 * 100.0) as usize;
            let elapsed = session.start_time.elapsed();

            print_header("Status");
            println!("  Model:    {}", session.model);
            println!("  Mode:     {:?}", session.mode);
            println!("  Context:  {}/{} tokens ({}%)", tokens, limit, pct);
            println!("  Messages: {}", session.conversation.messages.len());
            println!("  Uptime:   {}m {}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60);
            println!("  CWD:      {}", std::env::current_dir().unwrap_or_default().display());
            println!();
        }
        "/desktop" => {
            use crate::security::approval::{ActionType, SessionApproval};

            // Pre-approve all DesktopControl actions (mouse, keyboard)
            session.tool_context.approver.add_session_approval(SessionApproval {
                action_type: ActionType::Custom("DesktopControl".to_string()),
                target_pattern: "*".to_string(),
                approved_at: chrono::Utc::now(),
                expires_at: Some(chrono::Utc::now() + chrono::Duration::minutes(60)),
            });

            // Pre-approve open_application (CommandExecute)
            session.tool_context.approver.add_session_approval(SessionApproval {
                action_type: ActionType::CommandExecute,
                target_pattern: "*".to_string(),
                approved_at: chrono::Utc::now(),
                expires_at: Some(chrono::Utc::now() + chrono::Duration::minutes(60)),
            });

            print_success("Desktop automation mode enabled (60 min session)");
            println!();
            print_dim("  All desktop control tools pre-approved: mouse, keyboard, applications");
            println!();
        }
        "/skills" => {
            let registry = crate::skills::default_registry();
            let skills = registry.list();

            if skills.is_empty() {
                print_dim("  No skills registered. The agent can create skills on demand.");
                println!();
            } else {
                print_header("Available Skills");
                for skill in &skills {
                    let tag = if skill.builtin { "builtin" } else { "custom" };
                    println!("  {} [{}] - {}", skill.name, tag, skill.description);
                }
                println!();
                println!("  {} total ({} builtin, {} custom)",
                    skills.len(),
                    skills.iter().filter(|s| s.builtin).count(),
                    skills.iter().filter(|s| !s.builtin).count(),
                );
            }

            // Also show saved skills on disk
            if let Ok(saved) = registry.list_saved_skills() {
                if !saved.is_empty() {
                    println!();
                    print_dim(&format!("  {} saved skill files on disk", saved.len()));
                }
            }
            println!();
        }
        "/git" => {
            use crate::security::approval::{ActionType, SessionApproval};

            // Pre-approve all CommandExecute actions (git, shell commands)
            session.tool_context.approver.add_session_approval(SessionApproval {
                action_type: ActionType::CommandExecute,
                target_pattern: "*".to_string(),
                approved_at: chrono::Utc::now(),
                expires_at: Some(chrono::Utc::now() + chrono::Duration::minutes(60)),
            });

            print_success("Git mode enabled (60 min session)");
            println!();
            print_dim("  All shell commands pre-approved: git add, commit, push, etc.");
            println!();
        }
        "/exit" | "/quit" | "/q" => {
            if session.persistent {
                session.save().await?;
            }
            print_success(&session.personality.get_farewell());
            println!();
            return Ok(false);
        }
        _ => {
            // Check for ambiguous prefix matches to give a helpful message
            let all_commands = [
                "/help", "/clear", "/new", "/mode", "/model", "/tools",
                "/agents", "/soul", "/heartbeat", "/web", "/save",
                "/history", "/exit", "/conversations", "/load", "/context",
                "/memory", "/compact", "/cost", "/init", "/status", "/desktop", "/git", "/skills",
            ];
            let matches: Vec<&&str> = all_commands.iter()
                .filter(|c| c.starts_with(command))
                .collect();

            if matches.len() > 1 {
                print_error(&format!("Ambiguous command: {}", command));
                println!("  Did you mean: {}", matches.iter().map(|s| **s).collect::<Vec<_>>().join(", "));
            } else {
                print_error(&format!("Unknown command: {}", command));
                println!("  Type /help for commands.");
            }
        }
    }

    Ok(true)
}

/// Get system prompt based on mode and personality
fn get_system_prompt(session: &Session) -> String {
    let mode_str = match session.mode {
        Mode::Chat => "chat",
        Mode::Tools => "tools",
        Mode::Orchestrate => "orchestrate",
        Mode::Plan => "plan",
    };

    let base_prompt = session.personality.get_system_prompt(mode_str);

    // Load bootstrap context
    let bootstrap_context = crate::learning::BootstrapContext::new()
        .ok()
        .map(|ctx| {
            let _ = ctx.seed_defaults();
            ctx.load_all()
        })
        .unwrap_or_default();

    if bootstrap_context.is_empty() {
        base_prompt
    } else {
        crate::soul::system_prompts::get_full_system_prompt(&bootstrap_context)
    }
}

/// Process input with tools - implements the agentic tool-calling loop
async fn process_with_tools(session: &mut Session, input: &str) -> Result<String> {
    // Detect if the user wants to use a specific tool directly
    let lower = input.to_lowercase();

    // Direct tool execution for common patterns (shortcut for common commands)
    if lower.starts_with("read ") || lower.starts_with("open ") || lower.starts_with("cat ") {
        let path = input.split_whitespace().nth(1).unwrap_or("");
        if !path.is_empty() {
            return execute_direct_tool("read_file", &[("path", path)], &session.tool_context).await;
        }
    }

    if lower.starts_with("search for ") || lower.starts_with("search ") || lower.starts_with("find ") || lower.starts_with("grep ") {
        let pattern = input.split(":").nth(1)
            .or_else(|| input.split("for").nth(1))
            .or_else(|| input.split_whitespace().nth(1))
            .map(|s| s.trim())
            .unwrap_or("");
        if !pattern.is_empty() {
            return execute_direct_tool("search_content", &[("pattern", pattern)], &session.tool_context).await;
        }
    }

    if lower.starts_with("list ") || lower.starts_with("ls ") || lower.starts_with("dir ") {
        let dir = input.split_whitespace().nth(1).unwrap_or(".");
        return execute_direct_tool("list_directory", &[("path", dir)], &session.tool_context).await;
    }

    if lower.starts_with("write ") || lower.starts_with("edit ") {
        // Extract path and content
        let rest = input.split_whitespace().skip(1).collect::<Vec<_>>().join(" ");
        if let Some(colon_pos) = rest.find(':') {
            let path = rest[..colon_pos].trim();
            let content = rest[colon_pos + 1..].trim();
            return execute_direct_tool("write_file", &[("path", path), ("content", content)], &session.tool_context).await;
        }
    }

    // For other inputs, run the full tool-calling loop
    // Note: /heartbeat command is handled separately in handle_command()
    session.conversation.add_message(conversation::Role::User, input.to_string());

    run_tool_calling_loop(session).await
}

/// Default maximum number of tool-calling iterations to prevent infinite loops.
/// Configurable via `max_tool_iterations` in config.toml.
const DEFAULT_MAX_TOOL_ITERATIONS: usize = 15;

/// Run the agentic tool-calling loop
///
/// This implements the ReAct pattern:
/// 1. Send message to LLM with tools
/// 2. If LLM makes tool calls, execute them
/// 3. Feed results back to LLM
/// 4. Repeat until LLM responds without tool calls
async fn run_tool_calling_loop(session: &mut Session) -> Result<String> {
    let tools: Vec<ToolDefinition> = builtin_tools()
        .iter()
        .map(|t| ToolDefinition {
            r#type: "function".to_string(),
            function: FunctionDefinition {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            },
        })
        .collect();

    // System prompt for tool-calling, includes dynamic context
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());

    let tool_system_prompt = format!(
r#"You are My Agent, an AI assistant with tool capabilities.

## Environment
- Working directory: {cwd}
- Home directory: {home}
- IMPORTANT: Always use absolute paths based on the working directory above. Never guess paths — use get_cwd or list_directory to verify if unsure.

## Your Tools
You have access to these tools. Use them when needed to help the user:

### File Operations
- read_file(path): Read file contents
- write_file(path, content): Write to a file
- append_file(path, content): Append to a file
- list_directory(path): List directory contents
- search_content(pattern, directory): Search for text in files
- find_files(name_pattern): Find files by name
- glob(pattern): Find files by glob pattern
- file_info(path): Get file metadata
- create_directory(path): Create a directory
- delete_file(path): Delete a file

### Shell & Web
- execute_command(command): Run a shell command (requires approval)
- fetch_url(url): Fetch web content

### Skills
- list_skills(): List available skills
- create_skill(description, name?, category?): Generate a new skill dynamically
- use_skill(skill_id, params): Execute a skill

Note: Memory context from past conversations is automatically injected — you don't need to call a tool for it.

### Self-Improvement Tools
- analyze_performance(focus?): Analyze your performance metrics and get suggestions
- get_lessons(context?, min_confidence?): Retrieve lessons learned from past experiences
- record_lesson(insight, context, related_tools?): Record a new lesson
- improve_self(area?): Initiate a self-improvement cycle

### Learning System
- record_learning(content, category, source): Record a learning (correction, error, feature_request)
- review_learnings(category?): Review stored learnings
- search_learnings(query): Search learnings by keyword
- promote_learning(id): Promote a validated learning
- demote_learning(id): Demote a learning

### Self-Modification
- view_source(file): View your own source code
- edit_source(file, old_content, new_content): Edit your source code (requires approval)
- rebuild_self(): Rebuild and reinstall yourself
- self_diagnose(issue): Diagnose issues with your tools
- self_repair(issue_type): Attempt to repair issues

### Orchestration
- orchestrate_task(task, agent_type): Delegate to specialized agents
- spawn_agents(main_task, subtasks): Spawn multiple agents

## Guidelines
1. Use tools when you need information or need to perform actions
2. After using tools, synthesize the results into a helpful response
3. If a tool fails, read the error message carefully and fix the issue (wrong path, missing file, etc.) — do NOT retry the same failing call
4. Don't pretend to use tools — actually call them via the function API
5. NEVER return an empty response. If you hit errors, explain what happened and what you'll try next
6. If you're unsure of a path, use get_cwd or list_directory to verify before using it

## Response Format
- When using tools, call them and wait for results before responding
- When done with tools, provide a clear summary or answer
- Call multiple independent tools in the SAME response to save iterations
- For bulk file operations (move, delete, copy), use execute_command with a single combined shell command (e.g. `mv file1 file2 file3 dest/` or chain with `&&`) rather than calling tools one file at a time
- You have a limited number of tool iterations per task — work efficiently by batching operations

## Error Recovery
- If a tool call fails, READ the error output to understand why
- Common causes: wrong path (check with get_cwd), file doesn't exist (check with list_directory), permission denied (try a different approach)
- If stuck after 2-3 failed attempts, explain the situation to the user instead of silently failing
- ALWAYS respond to the user's messages — never ignore them"#);


    // Get the last user message for memory context
    let last_user_msg = session.conversation.messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, conversation::Role::User))
        .map(|m| m.content.clone())
        .unwrap_or_default();

    // Get memory context if available
    let memory_context = session.get_memory_context(&last_user_msg).await;
    if memory_context.is_some() {
        print_dim("💭 Injected relevant context from memory");
        println!();
    }

    // Build initial messages
    let base_messages: Vec<ChatMessage> = session.conversation.messages.iter().map(|m| ChatMessage {
        role: Some(serde_json::json!(match m.role {
            conversation::Role::User => "user",
            conversation::Role::Assistant => "assistant",
            conversation::Role::System => "system",
        })),
        content: Some(serde_json::json!(m.content.clone())),
        reasoning_details: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        reasoning: None,
        refusal: None,
    }).collect();

    // Manage context with context manager
    let managed = session.context_manager.manage_context(
        base_messages,
        Some(tool_system_prompt.clone()),
        memory_context,
    ).await?;

    // Show warning if context is getting full
    if let Some(ref warning) = managed.warning {
        print_dim(&format!("⚠️ {}", warning));
        println!();
    }

    // If naive trim happened, try recursive compression instead
    let managed_messages = if managed.was_trimmed && managed.messages.len() > 8 {
        let keep_recent = 6;
        let system_msg = managed.messages[0].clone();
        let middle = &managed.messages[1..managed.messages.len() - keep_recent];
        let recent: Vec<_> = managed.messages[managed.messages.len() - keep_recent..].to_vec();

        match session.recursive_manager.process_conversation(middle).await {
            Ok(result) => {
                print_dim(&format!("✨ Context compressed: {:.1}x ({} → {} tokens)",
                    result.compression_ratio, result.original_tokens, result.final_tokens));
                println!();
                let mut msgs = vec![system_msg];
                msgs.push(ChatMessage::system(format!(
                    "[Prior conversation summary]\n\n{}", result.final_summary
                )));
                msgs.extend(recent);
                msgs
            }
            Err(e) => {
                tracing::warn!("Recursive compression failed: {}, using naive trim", e);
                print_dim("📝 Context trimmed - older messages summarized");
                println!();
                managed.messages
            }
        }
    } else {
        if managed.was_trimmed {
            print_dim("📝 Context trimmed - older messages summarized");
            println!();
        }
        managed.messages
    };

    // Build final messages with system prompt
    let mut messages = if managed_messages.first().map(|m| m.role.as_ref().and_then(|r: &serde_json::Value| r.as_str()) == Some("system")).unwrap_or(false) {
        managed_messages
    } else {
        let mut msgs = vec![ChatMessage::system(tool_system_prompt.clone())];
        msgs.extend(managed_messages);
        msgs
    };

    let max_iterations = crate::config::Config::load()
        .map(|c| c.max_tool_iterations)
        .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS);

    let mut iteration = 0;
    let mut final_response = String::new();
    let mut empty_retries = 0;
    const MAX_EMPTY_RETRIES: usize = 2;
    // Track tool calls to detect repeated identical calls
    let mut seen_tool_calls: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut consecutive_dupes = 0;
    const MAX_CONSECUTIVE_DUPES: usize = 2;

    loop {
        iteration += 1;
        if iteration > max_iterations {
            print_dim(&format!("⚠️ Maximum tool iterations ({}) reached, stopping.", max_iterations));
            println!();
            break;
        }

        // Check context before each LLM call
        let current_tokens = ContextManager::estimate_message_tokens(&messages);
        if current_tokens > session.context_manager.config.max_context_tokens {
            print_dim(&format!("🔄 Compressing context ({} tokens)...", current_tokens));
            println!();

            // Keep system prompt + last 6 messages, compress the middle
            let keep_recent = 6;
            if messages.len() > keep_recent + 2 {
                let system_msg = messages[0].clone();
                let middle = &messages[1..messages.len() - keep_recent];
                let recent: Vec<_> = messages[messages.len() - keep_recent..].to_vec();

                match session.recursive_manager.process_conversation(middle).await {
                    Ok(result) => {
                        print_dim(&format!("✨ Compressed: {} → {} tokens ({:.1}x)",
                            result.original_tokens, result.final_tokens, result.compression_ratio));
                        println!();

                        messages = vec![system_msg];
                        messages.push(ChatMessage::system(format!(
                            "[Prior conversation summary]\n\n{}", result.final_summary
                        )));
                        messages.extend(recent);
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!("Recursive compression failed: {}, falling back to trim", e);
                    }
                }
            }

            // Fallback: naive trim
            let managed = session.context_manager.manage_context(
                messages.clone(),
                Some(tool_system_prompt.clone()),
                None,
            ).await?;
            messages = managed.messages;

            if managed.was_trimmed {
                print_dim("📝 Context trimmed - older messages summarized");
                println!();
            }
        }

        // Call LLM with tools (with thinking spinner)
        let thinking = create_thinking_spinner();
        let response = match session.client.complete_with_tools(
            &session.model,
            messages.clone(),
            tools.clone(),
            Some(4096),
        ).await {
            Ok(r) => {
                thinking.finish_and_clear();
                r
            }
            Err(e) => {
                thinking.finish_and_clear();
                // Fall back to simple chat on error
                print_dim(&format!("Tool calling failed, using simple chat: {}", e));
                println!();
                return process_simple(session, "").await;
            }
        };

        // Check if there are tool calls to execute
        // Some models (like z-ai/glm-5) may return finish_reason="tool_calls" but no actual tool_calls
        let tool_calls = response.tool_calls.clone();
        let has_tool_calls = tool_calls.as_ref().map(|tc| !tc.is_empty()).unwrap_or(false);

        // If the model claims tool_calls but didn't provide any, treat as regular response
        if !has_tool_calls {
            // Check if we have content - even if the model said tool_calls, use the content
            // Use content_as_text() to handle both string and array-of-content-parts formats
            let content = response.content_as_text().unwrap_or_default();

            if !content.is_empty() {
                // Print the final response with markdown
                println!();
                println!("{}", format_markdown(&content));
                println!();

                // Add to conversation
                session.conversation.add_message(
                    conversation::Role::Assistant,
                    content.clone()
                );
                final_response = content;
                break;
            } else {
                // No content and no tool calls — nudge the model to respond
                empty_retries += 1;
                if empty_retries > MAX_EMPTY_RETRIES {
                    print_dim("Model keeps returning empty responses. Try a different model or rephrase your request.");
                    println!();
                    break;
                }
                print_dim("Model returned empty response, retrying...");
                println!();
                messages.push(ChatMessage::system(
                    "Your last response was empty. You MUST respond to the user. \
                     If you encountered errors, explain what went wrong and what you'll try next. \
                     If you're stuck, ask the user for clarification. Never return an empty response."
                ));
                continue;
            }
        }

        // We have actual tool calls - execute them
        let tool_calls = tool_calls.unwrap();

        // Check for repeated identical tool calls (deduplication)
        let call_keys: Vec<String> = tool_calls.iter()
            .map(|tc| format!("{}:{}", tc.function.name, tc.function.arguments))
            .collect();
        let all_dupes = call_keys.iter().all(|k| seen_tool_calls.contains(k));
        if all_dupes {
            consecutive_dupes += 1;
            if consecutive_dupes >= MAX_CONSECUTIVE_DUPES {
                print_dim("Stopping: model is repeating the same tool calls.");
                println!();
                break;
            }
        } else {
            consecutive_dupes = 0;
        }
        for key in &call_keys {
            seen_tool_calls.insert(key.clone());
        }

        let mut tool_results_messages: Vec<ChatMessage> = Vec::new();

        // Add the assistant message with tool calls to messages
        let assistant_msg = ChatMessage {
            role: Some(serde_json::json!("assistant")),
            content: response.content.clone(),
            reasoning_details: None,
            tool_calls: Some(tool_calls.clone()),
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        };
        messages.push(assistant_msg);

        // Execute each tool call with compact display
        for tc in &tool_calls {
            let call = ToolCall {
                name: tc.function.name.clone(),
                arguments: serde_json::from_str(&tc.function.arguments).unwrap_or_default(),
            };

            let summary = format_tool_call(&call.name, &call.arguments);

            // Print a static status line instead of a spinner during tool execution.
            // Tools may trigger interactive approval prompts that require clean
            // stdin/stdout — a ticking spinner corrupts the terminal in that case.
            print!("  \x1b[90m◦\x1b[0m {}", summary);
            io::stdout().flush().unwrap_or_default();

            match execute_tool(&call, &session.tool_context).await {
                Ok(result) => {
                    // Overwrite the status line with the result
                    print!("\r\x1b[2K");
                    if result.success {
                        println!("  \x1b[32m✓\x1b[0m {}", summary);
                    } else {
                        // Show the first line of the error for compact display,
                        // the full stderr is in result.data for the LLM
                        let first_line = result.message.lines().take(2).collect::<Vec<_>>().join(" | ");
                        println!("  \x1b[31m✗\x1b[0m {}: {}", call.name, first_line);
                    }

                    // Create tool result message (truncate large results to prevent context explosion)
                    const MAX_TOOL_RESULT_CHARS: usize = 30000;
                    let tool_result_content = if let Some(data) = &result.data {
                        let full = serde_json::to_string(data).unwrap_or_else(|_| result.message.clone());
                        if full.len() > MAX_TOOL_RESULT_CHARS {
                            format!("{}...\n[truncated: {} total chars]", &full[..MAX_TOOL_RESULT_CHARS], full.len())
                        } else {
                            full
                        }
                    } else {
                        result.message.clone()
                    };

                    let tool_result_msg = ChatMessage {
                        role: Some(serde_json::json!("tool")),
                        content: Some(serde_json::json!(tool_result_content)),
                        reasoning_details: None,
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        name: Some(call.name.clone()),
                        reasoning: None,
                        refusal: None,
                    };
                    tool_results_messages.push(tool_result_msg);
                }
                Err(e) => {
                    print!("\r\x1b[2K");
                    println!("  \x1b[31m✗\x1b[0m {}: {}", call.name, e);

                    // Create error result message
                    let error_msg = ChatMessage {
                        role: Some(serde_json::json!("tool")),
                        content: Some(serde_json::json!(format!("Error: {}", e))),
                        reasoning_details: None,
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        name: Some(call.name.clone()),
                        reasoning: None,
                        refusal: None,
                    };
                    tool_results_messages.push(error_msg);
                }
            }
        }

        // Add tool results to messages for next iteration
        messages.extend(tool_results_messages);
    }

    // If the loop exited without a final text response (max iterations, dupes,
    // empty retries), save a summary of tool work done to the conversation so
    // the user can say "continue" and the LLM sees what was already done.
    if final_response.is_empty() && iteration > 1 {
        let mut summary_parts: Vec<String> = Vec::new();

        // Extract tool call summaries from the messages that were built during the loop
        // (skip the first few which are system/user messages from before the loop)
        for msg in &messages {
            let role = msg.role.as_ref().and_then(|r| r.as_str()).unwrap_or("");
            if role == "assistant" {
                if let Some(ref tcs) = msg.tool_calls {
                    for tc in tcs {
                        summary_parts.push(format!("- Called {}({})",
                            tc.function.name,
                            if tc.function.arguments.len() > 100 {
                                format!("{}...", &tc.function.arguments[..100])
                            } else {
                                tc.function.arguments.clone()
                            }
                        ));
                    }
                }
            } else if role == "tool" {
                if let Some(ref name) = msg.name {
                    let content = msg.content.as_ref()
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    let preview = if content.len() > 200 {
                        format!("{}...", &content[..200])
                    } else {
                        content.to_string()
                    };
                    summary_parts.push(format!("  Result from {}: {}", name, preview));
                }
            }
        }

        if !summary_parts.is_empty() {
            let summary = format!(
                "[Tool loop stopped after {} iterations. Work done so far:]\n{}",
                iteration - 1,
                summary_parts.join("\n")
            );
            session.conversation.add_message(
                conversation::Role::Assistant,
                summary,
            );
        }
    }

    Ok(final_response)
}

/// Execute a tool directly
async fn execute_direct_tool(name: &str, args: &[(&str, &str)], ctx: &ToolContext) -> Result<String> {
    let mut arguments = serde_json::Map::new();
    for (key, value) in args {
        arguments.insert(key.to_string(), serde_json::json!(value));
    }

    let call = ToolCall {
        name: name.to_string(),
        arguments: serde_json::Value::Object(arguments),
    };

    let summary = format_tool_call(name, &call.arguments);

    // Static status line — no spinner, to avoid interfering with approval prompts
    print!("  \x1b[90m◦\x1b[0m {}", summary);
    io::stdout().flush().unwrap_or_default();

    match execute_tool(&call, ctx).await {
        Ok(result) => {
            print!("\r\x1b[2K");
            if result.success {
                println!("  \x1b[32m✓\x1b[0m {}", summary);
            } else {
                println!("  \x1b[31m✗\x1b[0m {}: {}", name, result.message);
            }

            // Print file content directly for read operations
            if let Some(data) = &result.data {
                if let Some(content) = data.get("content").and_then(|c| c.as_str()) {
                    // Print content directly, not as JSON
                    println!("{}", content);
                    return Ok(content.to_string());
                }
                if let Some(files) = data.get("files").and_then(|f| f.as_array()) {
                    // Print directory listing as a tree
                    println!(".");
                    let total = files.len();
                    for (i, file) in files.iter().enumerate() {
                        if let Some(name) = file.get("name").and_then(|n| n.as_str()) {
                            let is_dir = file.get("is_dir").and_then(|d| d.as_bool()).unwrap_or(false);
                            let is_last = i == total - 1;
                            let prefix = if is_last { "└── " } else { "├── " };

                            if is_dir {
                                println!("{}  \x1b[34;1m{}/\x1b[0m", prefix, name);
                            } else {
                                println!("{}  {}", prefix, name);
                            }
                        }
                    }
                    println!();
                    return Ok(format!("{} entries", total));
                }
                if let Some(matches) = data.get("matches").and_then(|m| m.as_array()) {
                    // Print search results cleanly
                    for m in matches {
                        if let (Some(file), Some(line)) = (
                            m.get("file").and_then(|f| f.as_str()),
                            m.get("line").and_then(|l| u64::from_str_radix(&l.to_string(), 10).ok())
                        ) {
                            let content = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
                            println!("\x1b[90m{}:{}\x1b[0m {}", file, line, content.trim());
                        }
                    }
                    return Ok(format!("{} matches", matches.len()));
                }
                // Fallback to JSON for other data
                let output = serde_json::to_string_pretty(&data).unwrap_or_default();
                println!("{}", output);
                return Ok(output);
            }
            Ok(result.message)
        }
        Err(e) => {
            print!("\r\x1b[2K");
            println!("  \x1b[31m✗\x1b[0m Error: {}", e);
            Ok(format!("Error: {}", e))
        }
    }
}

/// Plan structure for plan mode
struct Plan {
    task_type: String,
    approach: String,
    steps: Vec<String>,
    tools_needed: Vec<String>,
    agents_needed: Vec<String>,
    estimated_complexity: String,
}

/// Create a plan for the given input
async fn create_plan(session: &Session, input: &str) -> Result<Plan> {
    // Use the orchestrator to analyze the task
    let orchestrator = SmartReasoningOrchestrator::new()?;
    let orchestration_plan = orchestrator.process_request(input).await?;

    // Build a human-readable plan
    let mut steps = Vec::new();

    // Determine approach based on task type
    let approach = match orchestration_plan.task_type {
        crate::orchestrator::TaskType::Simple => {
            steps.push("Analyze the request".to_string());
            steps.push("Process and respond".to_string());
            "Simple task processing"
        }
        crate::orchestrator::TaskType::Conversation => {
            steps.push("Understand the conversation context".to_string());
            steps.push("Generate appropriate response".to_string());
            "Conversation handling"
        }
        crate::orchestrator::TaskType::Complex => {
            steps.push("Break down the complex task".to_string());
            steps.push("Analyze requirements".to_string());
            steps.push("Execute sub-tasks".to_string());
            steps.push("Synthesize results".to_string());
            "Complex task decomposition"
        }
        crate::orchestrator::TaskType::MultiStep => {
            steps.push("Identify task steps".to_string());
            steps.push("Plan execution order".to_string());
            steps.push("Execute each step sequentially".to_string());
            steps.push("Combine results".to_string());
            "Multi-step task execution"
        }
    };

    // Add agents to steps
    for agent in &orchestration_plan.agents {
        steps.push(format!("Spawn {} agent: {}", agent.capability, agent.task));
    }

    let tools_needed = if orchestration_plan.needs_agents {
        vec!["orchestrator".to_string()]
    } else {
        builtin_tools().iter()
            .filter(|t| {
                let name = t.name.as_str();
                input.to_lowercase().contains("search") && name == "search_content" ||
                input.to_lowercase().contains("find") && name == "find_files" ||
                input.to_lowercase().contains("read") && name == "read_file" ||
                input.to_lowercase().contains("list") && name == "glob"
            })
            .map(|t| t.name.clone())
            .collect()
    };

    let agents_needed: Vec<String> = orchestration_plan.agents
        .iter()
        .map(|a| a.capability.clone())
        .collect();

    let estimated_complexity = if orchestration_plan.agents.len() > 2 {
        "high"
    } else if !orchestration_plan.agents.is_empty() {
        "medium"
    } else {
        "low"
    };

    Ok(Plan {
        task_type: format!("{:?}", orchestration_plan.task_type),
        approach: approach.to_string(),
        steps,
        tools_needed,
        agents_needed,
        estimated_complexity: estimated_complexity.to_string(),
    })
}

/// Display a plan for approval
fn display_plan(plan: &Plan) {
    print_header("Task Plan");

    println!("  Task Type: {}", plan.task_type);
    println!("  Approach: {}", plan.approach);
    println!("  Complexity: {}", plan.estimated_complexity);
    println!();

    println!("  Steps:");
    for (i, step) in plan.steps.iter().enumerate() {
        println!("    {}. {}", i + 1, step);
    }
    println!();

    if !plan.tools_needed.is_empty() {
        println!("  Tools: {}", plan.tools_needed.join(", "));
    }
    if !plan.agents_needed.is_empty() {
        println!("  Agents: {}", plan.agents_needed.join(", "));
    }
    println!();

    print_dim("  Options: 'yes' to execute | 'no' to cancel | 'modify <feedback>' to revise");
    println!();
}

/// Process with planning (shows plan first, then executes on approval)
async fn process_with_plan(session: &mut Session, input: &str) -> Result<Option<String>> {
    // Create the plan
    let spinner = create_thinking_spinner();
    let plan = create_plan(session, input).await?;
    spinner.finish_and_clear();

    // Display the plan
    display_plan(&plan);

    // Ask for approval
    print_colored("❯ ", Color::Yellow);
    print_colored("Approve plan? ", Color::Yellow);
    let _ = io::stdout().flush();

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;
    let response = response.trim().to_lowercase();

    match response.as_str() {
        "yes" | "y" | "ok" | "sure" | "go" | "execute" => {
            println!();
            print_success("✓ Executing plan...");
            println!();

            // Execute based on plan complexity
            if !plan.agents_needed.is_empty() {
                process_with_orchestrator(session, input).await.map(Some)
            } else if !plan.tools_needed.is_empty() {
                process_with_tools(session, input).await.map(Some)
            } else {
                process_simple(session, input).await.map(Some)
            }
        }
        "no" | "n" | "cancel" | "stop" => {
            println!();
            print_dim("Plan cancelled.");
            println!();
            Ok(None)
        }
        resp if resp.starts_with("modify") || resp.starts_with("change") => {
            println!();
            print_dim("Plan modification not yet implemented. Please describe your modified request.");
            println!();
            Ok(None)
        }
        _ => {
            println!();
            print_dim("Unknown response. Plan cancelled.");
            println!();
            Ok(None)
        }
    }
}

/// Process with orchestrator (spawn agents)
async fn process_with_orchestrator(session: &mut Session, input: &str) -> Result<String> {
    // Create orchestrator
    let orchestrator = SmartReasoningOrchestrator::new()?;

    // Get plan
    let plan = orchestrator.process_request(input).await?;

    let mut results = Vec::new();

    if plan.agents.is_empty() {
        // No agents needed, just use the chat model
        return process_simple(session, input).await;
    }

    // Show plan
    print_dim(&format!("  Task type: {:?} · {} agent(s)", plan.task_type, plan.agents.len()));
    println!();

    // Create context, bus, and spawner
    let context = Arc::new(SharedContext::new(session.client.clone())?);
    let bus = Arc::new(crate::orchestrator::bus::AgentBus::new());
    let mut spawner = AgentSpawner::new(context.clone(), bus.clone());

    for spec in &plan.agents {
        let agent_type = crate::orchestrator::SubagentType::from_capability(&spec.capability);
        let label = agent_type.display_name();
        let agent_spinner = create_agent_spinner(&label);

        let id = spawner.spawn_typed(spec.clone(), agent_type.clone()).await?;

        let context_json = serde_json::json!({
            "original_request": input,
            "agent_type": spec.capability,
        });

        // Assign task and WAIT for result
        match spawner.assign_and_wait(
            &id,
            spec.task.clone(),
            context_json,
            Duration::from_secs(120),
        ).await {
            Ok(result) => {
                agent_spinner.finish_with_message(format!("\x1b[32m✓\x1b[0m {} agent completed", label));
                results.push(format!("## {} Agent Result\n{}", label, result));
            }
            Err(e) => {
                agent_spinner.finish_with_message(format!("\x1b[31m✗\x1b[0m {} agent failed: {}", label, e));
                results.push(format!("## Agent Error\n{}", e));
            }
        }
    }

    spawner.shutdown_all().await?;

    // Combine all results and synthesize with LLM
    let combined = results.join("\n\n---\n\n");
    let summary_prompt = format!(
        "The user asked: {}\n\nHere are the results from specialized agents:\n\n{}\n\n\
         Synthesize these results into a clear, actionable response.",
        input, combined
    );

    session.conversation.add_message(conversation::Role::User, input.to_string());

    let messages: Vec<ChatMessage> = vec![
        ChatMessage::system("You are synthesizing results from specialized agents. Be concise and actionable."),
        ChatMessage::user(summary_prompt),
    ];

    let summary = session.client.complete(&session.model, messages, Some(2048)).await?;

    println!();
    println!("{}", summary);
    println!();

    session.conversation.add_message(conversation::Role::Assistant, summary.clone());

    Ok(summary)
}

/// Simple chat without tools - with streaming
async fn process_simple(session: &mut Session, input: &str) -> Result<String> {
    // Only add message if not already added (check last message)
    let should_add = session.conversation.messages.last()
        .map(|m| m.role != conversation::Role::User || m.content != input)
        .unwrap_or(true);

    if should_add {
        session.conversation.add_message(conversation::Role::User, input.to_string());
    }

    // Get memory context
    let memory_context = session.get_memory_context(input).await;
    if memory_context.is_some() {
        print_dim("💭 Injected relevant context from memory");
        println!();
    }

    // Build messages
    let base_messages: Vec<ChatMessage> = session.conversation.messages
        .iter()
        .map(|m| ChatMessage {
            role: Some(serde_json::json!(match m.role {
                conversation::Role::User => "user",
                conversation::Role::Assistant => "assistant",
                conversation::Role::System => "system",
            })),
            content: Some(serde_json::json!(m.content.clone())),
            reasoning_details: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        })
        .collect();

    // Manage context
    let managed = session.context_manager.manage_context(
        base_messages,
        None::<String>,
        memory_context,
    ).await?;

    // Show warning if context is getting full
    if let Some(ref warning) = managed.warning {
        print_dim(&format!("⚠️ {}", warning));
        println!();
    }

    // If naive trim happened, try recursive compression instead
    let final_messages = if managed.was_trimmed && managed.messages.len() > 8 {
        let keep_recent = 6;
        let system_msg = managed.messages[0].clone();
        let middle = &managed.messages[1..managed.messages.len() - keep_recent];
        let recent: Vec<_> = managed.messages[managed.messages.len() - keep_recent..].to_vec();

        match session.recursive_manager.process_conversation(middle).await {
            Ok(result) => {
                print_dim(&format!("✨ Context compressed: {:.1}x ({} → {} tokens)",
                    result.compression_ratio, result.original_tokens, result.final_tokens));
                println!();
                let mut msgs = vec![system_msg];
                msgs.push(ChatMessage::system(format!(
                    "[Prior conversation summary]\n\n{}", result.final_summary
                )));
                msgs.extend(recent);
                msgs
            }
            Err(e) => {
                tracing::warn!("Recursive compression failed in chat mode: {}", e);
                if managed.was_trimmed {
                    print_dim("📝 Context trimmed - older messages summarized");
                    println!();
                }
                managed.messages
            }
        }
    } else {
        managed.messages
    };

    // Use streaming for real-time display
    println!();
    let response = session.client.stream_complete(
        &session.model,
        final_messages,
        Some(4096),
        |chunk| {
            // Strip markdown during streaming for cleaner output
            let clean = chunk.replace("**", "").replace("`", "");
            print!("{}", clean);
            let _ = io::stdout().flush();
        }
    ).await?;
    println!();
    println!();

    Ok(response)
}

/// Run a future with Ctrl+C cancellation support.
/// Returns `Some(result)` if the future completed, `None` if cancelled.
async fn cancellable<F, T>(fut: F) -> Option<T>
where
    F: std::future::Future<Output = T>,
{
    tokio::pin!(fut);
    tokio::select! {
        biased;
        _ = tokio::signal::ctrl_c() => None,
        r = &mut fut => Some(r),
    }
}

/// Run the interactive chat session
pub async fn run_interactive(persistent: bool, resume: bool) -> Result<()> {
    if !io::stdin().is_terminal() {
        return run_non_interactive(persistent).await;
    }

    // Check API key
    if !crate::security::keyring::has_api_key() {
        print_error("✗ No API key configured");
        println!("Run: my-agent config --set-api-key YOUR_KEY");
        return Ok(());
    }

    // Create client
    let client = OpenRouterClient::from_keyring()?;

    // Always enable persistence in interactive mode — memory is a sensible default
    let persistent = true;

    // Initialize session
    let mut session = if resume {
        if let Ok(store) = crate::memory::MemoryStore::default_store().await {
            let recent = store.list_conversations(1, 0).await?;
            if let Some(record) = recent.first() {
                print_success(&format!("✓ Resumed: {}", &record.id[..8]));
                println!();
                let mut s = Session::from_conversation(client, record.clone(), persistent);
                s.memory_store = Some(Arc::new(store.clone()));
                s.semantic_search = Some(SemanticSearch::new(Arc::new(store)));
                s
            } else {
                let mut s = Session::new(client, persistent);
                let _ = s.init_memory().await;
                s
            }
        } else {
            Session::new(client, persistent)
        }
    } else {
        let mut s = Session::new(client, persistent);
        let _ = s.init_memory().await;
        s
    };

    // Initialize memory store (fallback if not done above)
    if session.memory_store.is_none() {
        if let Ok(store) = crate::memory::MemoryStore::default_store().await {
            let store_arc = Arc::new(store);
            session.memory_store = Some(store_arc.clone());
            session.semantic_search = Some(SemanticSearch::new(store_arc));
        }
    }

    // Add system prompt if new conversation
    if session.conversation.messages.is_empty() {
        session.conversation.add_message(
            conversation::Role::System,
            get_system_prompt(&session)
        );
    }

    // Auto-start the soul heartbeat engine in the background
    if let Err(e) = crate::soul::engine::start_soul().await {
        tracing::debug!("Soul engine auto-start skipped: {}", e);
    }

    print_banner(&session.personality.name, &session.model, &session.mode);
    print_mode_help(&session.mode);

    // Setup rustyline with autocomplete and proper config
    let config = rustyline::Config::builder()
        .completion_type(rustyline::CompletionType::List)
        .edit_mode(rustyline::EditMode::Emacs)
        .auto_add_history(true)
        .tab_stop(4)
        .build();

    let mut rl = rustyline::Editor::<AgentHelper, rustyline::history::DefaultHistory>::with_config(config).unwrap();
    rl.set_helper(Some(AgentHelper::new()));

    // Main loop with rustyline
    loop {
        // Simple, clean prompt
        let prompt = "\x1b[32m❯\x1b[0m ".to_string();

        let readline = rl.readline(&prompt);

        match readline {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(input);

                // Handle keyboard shortcuts (single character commands)
                if input.len() == 1 {
                    let ch = input.chars().next().unwrap();
                    if ch == '?' {
                        show_keyboard_shortcuts();
                        continue;
                    }
                    // Check for other single-key shortcuts
                    for (key, _desc, cmd) in KEYBOARD_SHORTCUTS {
                        if ch == *key && !cmd.is_empty() {
                            if !handle_command(cmd, &mut session).await? {
                                break;
                            }
                            continue;
                        }
                    }
                }

                // Handle slash commands
                if input.starts_with('/') {
                    if !handle_command(input, &mut session).await? {
                        break;
                    }
                    continue;
                }

        // Quick exit
                if input == "exit" || input == "quit" {
                    if session.persistent {
                        session.save().await?;
                    }
                    print_success(&session.personality.get_farewell());
                    println!();
                    break;
                }

                // Process based on mode and task complexity
                // Each processing path is wrapped with cancellable() so Ctrl+C
                // during LLM calls or tool execution returns to the prompt.
                let result = match session.mode {
                    Mode::Chat => {
                        let spinner = create_thinking_spinner();
                        match cancellable(process_simple(&mut session, input)).await {
                            Some(r) => { spinner.finish_and_clear(); r }
                            None => {
                                spinner.finish_and_clear();
                                print_dim("\n⚠ Cancelled.");
                                println!();
                                continue;
                            }
                        }
                    }
                    Mode::Tools => {
                        // Auto-detect if orchestration is needed
                        if needs_orchestration(input) {
                            print_dim("  Complex task detected, switching to orchestrate mode...");
                            println!();
                            let spinner = create_thinking_spinner();
                            match cancellable(process_with_orchestrator(&mut session, input)).await {
                                Some(r) => { spinner.finish_and_clear(); r }
                                None => {
                                    spinner.finish_and_clear();
                                    print_dim("\n⚠ Cancelled.");
                                    println!();
                                    continue;
                                }
                            }
                        } else {
                            // Spinner created inside run_tool_calling_loop
                            match cancellable(process_with_tools(&mut session, input)).await {
                                Some(r) => r,
                                None => {
                                    print_dim("\n⚠ Cancelled.");
                                    println!();
                                    continue;
                                }
                            }
                        }
                    }
                    Mode::Orchestrate => {
                        if needs_tools(input) && !needs_orchestration(input) {
                            print_dim("  Simple task, using tools...");
                            println!();
                            // Spinner created inside run_tool_calling_loop
                            match cancellable(process_with_tools(&mut session, input)).await {
                                Some(r) => r,
                                None => {
                                    print_dim("\n⚠ Cancelled.");
                                    println!();
                                    continue;
                                }
                            }
                        } else {
                            let spinner = create_thinking_spinner();
                            match cancellable(process_with_orchestrator(&mut session, input)).await {
                                Some(r) => { spinner.finish_and_clear(); r }
                                None => {
                                    spinner.finish_and_clear();
                                    print_dim("\n⚠ Cancelled.");
                                    println!();
                                    continue;
                                }
                            }
                        }
                    }
                    Mode::Plan => {
                        // Plan mode - show plan first, then execute on approval
                        match cancellable(process_with_plan(&mut session, input)).await {
                            Some(Ok(Some(response))) => Ok(response),
                            Some(Ok(None)) => {
                                // Plan was cancelled by user
                                continue;
                            }
                            Some(Err(e)) => Err(e),
                            None => {
                                print_dim("\n⚠ Cancelled.");
                                println!();
                                continue;
                            }
                        }
                    }
                };

                match result {
                    Ok(response) => {
                        // Response already printed during streaming
                        // Just save to conversation history
                        session.conversation.add_message(conversation::Role::Assistant, response);

                        if session.persistent {
                            session.save().await?;
                        }
                    }
                    Err(e) => {
                        print_error(&format!("✗ Error: {}", e));
                        println!();
                    }
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                print_success(&session.personality.get_farewell());
                println!();
                break;
            }
            Err(err) => {
                print_error(&format!("Error: {}", err));
                break;
            }
        }
    }

    // Gracefully stop the soul heartbeat engine
    let _ = crate::soul::engine::stop_soul().await;

    Ok(())
}

/// Non-interactive mode
async fn run_non_interactive(_persistent: bool) -> Result<()> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        return Ok(());
    }

    if !crate::security::keyring::has_api_key() {
        eprintln!("Error: No API key configured");
        return Ok(());
    }

    let client = OpenRouterClient::from_keyring()?;
    let model = AgentConfig::load().unwrap_or_default().models.chat;
    let tool_context = ToolContext::new();

    // Check if it's a direct tool command
    let lower = input.to_lowercase();
    if lower.starts_with("read ") || lower.starts_with("open ") {
        let path = input.split_whitespace().nth(1).unwrap_or("");
        if !path.is_empty() {
            let result = execute_direct_tool("read_file", &[("path", path)], &tool_context).await?;
            println!("{}", result);
            return Ok(());
        }
    }

    if lower.starts_with("search for ") || lower.starts_with("find ") {
        let pattern = input.split(":").nth(1)
            .or_else(|| input.split("for").nth(1))
            .map(|s| s.trim())
            .unwrap_or("");
        if !pattern.is_empty() {
            let result = execute_direct_tool("search_content", &[("pattern", pattern)], &tool_context).await?;
            println!("{}", result);
            return Ok(());
        }
    }

    if lower.starts_with("list ") || lower.starts_with("ls ") {
        let dir = input.split_whitespace().nth(1).unwrap_or(".");
        let result = execute_direct_tool("list_directory", &[("path", dir)], &tool_context).await?;
        println!("{}", result);
        return Ok(());
    }

    // Fall back to chat
    let messages = vec![
        ChatMessage::system("You are a helpful AI assistant. Be concise."),
        ChatMessage::user(input.to_string()),
    ];

    match client.complete(&model, messages, Some(2048)).await {
        Ok(response) => println!("{}", response),
        Err(e) => eprintln!("Error: {}", e),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_command_exact() {
        assert_eq!(resolve_command("/help"), "/help");
        assert_eq!(resolve_command("/clear"), "/clear");
        assert_eq!(resolve_command("/exit"), "/exit");
    }

    #[test]
    fn test_resolve_command_prefix() {
        // Unique prefix matches
        assert_eq!(resolve_command("/hel"), "/help");
        assert_eq!(resolve_command("/cl"), "/clear");
        assert_eq!(resolve_command("/to"), "/tools");
        assert_eq!(resolve_command("/ag"), "/agents");
        assert_eq!(resolve_command("/hi"), "/history");
        assert_eq!(resolve_command("/sa"), "/save");
        assert_eq!(resolve_command("/w"), "/web");
        assert_eq!(resolve_command("/ini"), "/init");
        assert_eq!(resolve_command("/sta"), "/status");
    }

    #[test]
    fn test_resolve_command_prefix_with_args() {
        assert_eq!(resolve_command("/mode tools"), "/mode tools");
        assert_eq!(resolve_command("/model chat"), "/model chat");
    }

    #[test]
    fn test_resolve_command_ambiguous() {
        // /he matches /help and /heartbeat
        assert_eq!(resolve_command("/he"), "/he");
        // /mo matches /mode and /model
        assert_eq!(resolve_command("/mo"), "/mo");
        // /co matches /commands, /compact, /context, /conversations, /cost
        assert_eq!(resolve_command("/co"), "/co");
    }

    #[test]
    fn test_resolve_command_no_match() {
        assert_eq!(resolve_command("/xyz"), "/xyz");
    }

    #[test]
    fn test_format_tool_call_with_path() {
        let args = serde_json::json!({"path": "/home/user/test.rs"});
        assert_eq!(format_tool_call("read_file", &args), "read_file /home/user/test.rs");
    }

    #[test]
    fn test_format_tool_call_truncation() {
        let long_path = "a".repeat(60);
        let args = serde_json::json!({"path": long_path});
        let result = format_tool_call("read_file", &args);
        assert!(result.len() <= 60); // name + space + 47 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_format_tool_call_no_args() {
        let args = serde_json::json!({});
        assert_eq!(format_tool_call("list_skills", &args), "list_skills");
    }
}