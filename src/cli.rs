//! CLI interface for my-agent

use clap::{Parser, Subcommand};
use anyhow::Result;

// Import memory module for conversation persistence
use crate::memory;

#[derive(Parser)]
#[command(name = "my-agent")]
#[command(about = "Personal AI Agent Assistant with persistent memory and semantic search", long_about = None)]
#[command(version)]
struct Cli {
    /// Start interactive chat (default when no command given)
    #[arg(short, long)]
    persistent: bool,

    /// Resume most recent conversation
    #[arg(short, long)]
    resume: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start an interactive chat session (Claude Code-like experience)
    Interactive {
        /// Enable persistent memory (save conversations)
        #[arg(short = 'P', long)]
        persistent: bool,
        /// Resume most recent conversation
        #[arg(short, long)]
        resume: bool,
    },
    /// Start a chat session (voice or text)
    Chat {
        /// Use voice mode (default: text)
        #[arg(short, long)]
        voice: bool,
        /// Enable tool-calling mode (agent can use tools)
        #[arg(long)]
        tools: bool,
        /// Enable persistent memory (save conversations)
        #[arg(short = 'P', long)]
        persistent: bool,
        /// Resume most recent conversation
        #[arg(short, long)]
        resume: bool,
        /// Load specific conversation by ID
        #[arg(short = 'C', long)]
        conversation_id: Option<String>,
    },
    /// Manage conversation history
    History {
        #[command(subcommand)]
        command: HistoryCommands,
    },
    /// Search conversations and memory
    Search {
        /// Search query
        query: String,
        /// Maximum results to return
        #[arg(short, long, default_value = "10")]
        limit: usize,
        /// Use semantic search (embeddings)
        #[arg(short, long)]
        semantic: bool,
    },
    /// Configure the agent
    Config {
        /// Set OpenRouter API key
        #[arg(long)]
        set_api_key: Option<String>,
        /// Set Hugging Face API key (for voice features)
        #[arg(long)]
        set_hf_api_key: Option<String>,
        /// Set NVIDIA NIM API key (for NVIDIA-hosted models)
        #[arg(long)]
        set_nvidia_key: Option<String>,
        /// Set server password for remote access authentication
        #[arg(long)]
        set_password: bool,
        /// Show current configuration
        #[arg(long)]
        show: bool,
        /// Set model for a role (usage: --set-model role model_id)
        #[arg(long, value_names = &["role", "model"])]
        set_model: Option<Vec<String>>,
        /// Get model for a role
        #[arg(long)]
        get_model: Option<String>,
        /// List all model assignments
        #[arg(long)]
        list_models: bool,
    },
    /// Run diagnostics and self-healing
    Doctor {
        /// Fix issues automatically
        #[arg(short, long)]
        fix: bool,
        /// Check for updates
        #[arg(long)]
        update: bool,
    },
    /// Manage memory database (embeddings, stats, cleanup)
    Memory {
        #[command(subcommand)]
        command: MemoryCommands,
    },
    /// Manage dynamic skills
    Skills {
        #[command(subcommand)]
        command: SkillCommands,
    },
    /// Manage the soul/heartbeat system
    Soul {
        #[command(subcommand)]
        command: SoulCommands,
    },
    /// Start the web server
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value = "8080")]
        port: u16,
        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Enable HTTPS
        #[arg(long)]
        https: bool,
        /// Path to SSL certificate
        #[arg(long)]
        cert: Option<String>,
        /// Path to SSL private key
        #[arg(long)]
        key: Option<String>,
        /// Start a Cloudflare Tunnel for public HTTPS access (requires cloudflared)
        #[arg(long)]
        tunnel: bool,
    },
    /// Connect to a remote server as a device agent
    Connect {
        /// Server URL (e.g., https://abc-123.trycloudflare.com)
        #[arg(long)]
        server: String,
        /// Device name (default: hostname)
        #[arg(long)]
        name: Option<String>,
        /// JWT access token for authentication
        #[arg(long)]
        token: String,
    },
    /// Orchestrate complex multi-agent tasks
    Orchestrate {
        /// Task description
        #[arg(short, long)]
        task: String,
        /// Verbose output
        #[arg(short, long)]
        verbose: bool,
        /// Interactive mode
        #[arg(short, long)]
        interactive: bool,
        /// Only show plan, don't execute
        #[arg(long)]
        plan_only: bool,
    },
    /// Start the gateway daemon (always-on service)
    Gateway {
        #[command(subcommand)]
        command: GatewayCommands,
    },
    /// Manage the learning system
    Learning {
        #[command(subcommand)]
        command: LearningCommands,
    },
    /// Run deterministic YAML pipelines
    Pipeline {
        #[command(subcommand)]
        command: PipelineCommands,
    },
}

#[derive(Subcommand)]
enum GatewayCommands {
    /// Start the gateway daemon
    Start {
        /// Port to listen on
        #[arg(short, long, default_value = "18789")]
        port: u16,
        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Disable web server
        #[arg(long)]
        no_web: bool,
        /// Enable messaging integrations
        #[arg(long)]
        messaging: bool,
        /// Don't start soul engine
        #[arg(long)]
        no_soul: bool,
    },
    /// Show gateway status
    Status,
    /// Manage the heartbeat system
    Heartbeat {
        #[command(subcommand)]
        command: HeartbeatCommands,
    },
}

#[derive(Subcommand)]
enum HeartbeatCommands {
    /// Show heartbeat status
    Status,
    /// Manually trigger a single heartbeat tick
    Tick,
    /// Edit the HEARTBEAT.md checklist
    Edit,
}

#[derive(Subcommand)]
enum LearningCommands {
    /// Show learning statistics
    Stats,
    /// Search learnings
    Search {
        /// Search query
        query: String,
    },
    /// Review learnings by status
    Review {
        /// Filter by status: new, validated, promoted, all
        #[arg(short, long, default_value = "all")]
        status: String,
    },
    /// Run a promotion cycle (promote validated learnings)
    Promote,
    /// Seed bootstrap context files with defaults
    Init,
}

#[derive(Subcommand)]
enum PipelineCommands {
    /// Run a pipeline from a YAML file
    Run {
        /// Path to the YAML pipeline file
        yaml: String,
    },
    /// List available pipelines
    List,
    /// Approve a waiting pipeline step and continue
    Approve {
        /// Pipeline name
        name: String,
    },
    /// Show pipeline status
    Status {
        /// Pipeline name
        name: String,
    },
}

#[derive(Subcommand)]
enum HistoryCommands {
    /// List recent conversations
    List {
        /// Maximum conversations to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Show a specific conversation
    Show {
        /// Conversation ID
        id: String,
    },
    /// Delete a conversation
    Delete {
        /// Conversation ID
        id: String,
    },
    /// Clear all history
    Clear {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum SkillCommands {
    /// List installed skills
    List,
    /// Create a new skill from a template
    Create {
        /// Skill name (used as filename and ID)
        name: String,
        /// Skill description
        #[arg(short, long, default_value = "")]
        description: String,
    },
    /// Show details of a specific skill
    Show {
        /// Skill ID
        id: String,
    },
    /// Install a skill
    Install {
        /// Skill name
        name: String,
    },
    /// Remove a skill
    Remove {
        /// Skill name/ID
        name: String,
    },
}

#[derive(Subcommand)]
enum SoulCommands {
    /// Start the heartbeat/soul
    Start,
    /// Stop the heartbeat/soul
    Stop,
    /// Show current status
    Status,
    /// Review improvements and learnings
    Review,
}

#[derive(Subcommand)]
enum MemoryCommands {
    /// Show memory statistics
    Stats,
    /// Search memory with semantic search
    Search {
        /// Search query
        query: String,
        /// Maximum results
        #[arg(short, long, default_value = "5")]
        limit: usize,
    },
    /// Add knowledge to the database
    Add {
        /// Content to remember
        #[arg(short, long)]
        content: String,
        /// Source (e.g., "user", "document")
        #[arg(short, long, default_value = "user")]
        source: String,
        /// Importance (0.0-1.0)
        #[arg(short, long, default_value = "0.5")]
        importance: f32,
    },
    /// List knowledge entries
    List {
        /// Maximum entries to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Clean up old conversations
    Cleanup {
        /// Keep last N conversations
        #[arg(short, long, default_value = "100")]
        keep: usize,
    },
    /// Initialize embedding model
    InitEmbeddings {
        /// Force re-initialization
        #[arg(short, long)]
        force: bool,
    },
    /// View daily logs
    Logs {
        /// Specific date to view (YYYY-MM-DD), or "today"/"yesterday"
        date: Option<String>,
    },
    /// Clean up old daily logs
    LogsCleanup {
        /// Keep logs from the last N days
        #[arg(short, long, default_value = "30")]
        days: u32,
    },
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    // Default to interactive mode if no command given
    match cli.command {
        None => {
            crate::agent::interactive::run_interactive(cli.persistent, cli.resume).await?;
        }
        Some(Commands::Interactive { persistent, resume }) => {
            crate::agent::interactive::run_interactive(persistent, resume).await?;
        }
        Some(Commands::Chat { voice, tools, persistent, resume, conversation_id }) => {
            if tools {
                println!("Starting tool-enabled chat session...");
                crate::agent::tool_conversation::start_tool_chat().await?;
            } else if voice {
                println!("Starting voice chat session...");
                println!("⚠️ Voice chat not yet fully implemented. Use text mode for now.");
                crate::agent::start_text_chat_with_options(persistent, conversation_id, resume).await?;
            } else {
                println!("Starting text chat session...");
                if persistent || resume || conversation_id.is_some() {
                    println!("💾 Persistence enabled - conversations will be saved.");
                }
                crate::agent::start_text_chat_with_options(persistent, conversation_id, resume).await?;
            }
        }
        Some(Commands::History { command }) => {
            match command {
                HistoryCommands::List { limit } => {
                    crate::agent::list_conversations(limit).await?;
                }
                HistoryCommands::Show { id } => {
                    show_conversation(&id).await?;
                }
                HistoryCommands::Delete { id } => {
                    delete_conversation(&id).await?;
                }
                HistoryCommands::Clear { yes } => {
                    clear_history(yes).await?;
                }
            }
        }
        Some(Commands::Search { query, limit, semantic }) => {
            if semantic {
                println!("🔍 Semantic search for: {}", query);
                // Fall back to regular search for now, semantic needs embedding model
                crate::agent::search_conversations(&query, limit).await?;
            } else {
                println!("🔍 Searching for: {}", query);
                crate::agent::search_conversations(&query, limit).await?;
            }
        }
        Some(Commands::Config { set_api_key, set_hf_api_key, set_nvidia_key, set_password, show, set_model, get_model, list_models }) => {
            if let Some(key) = set_api_key {
                crate::security::set_api_key(&key)?;
                println!("OpenRouter API key stored securely in keyring.");
            } else if let Some(key) = set_hf_api_key {
                crate::security::set_hf_api_key(&key)?;
                println!("Hugging Face API key stored securely in keyring.");
            } else if let Some(key) = set_nvidia_key {
                crate::security::set_nvidia_api_key(&key)?;
                println!("NVIDIA NIM API key stored securely in keyring.");
            } else if set_password {
                // Prompt for password with echo disabled
                use std::io::Write;
                eprint!("Enter server password: ");
                std::io::stderr().flush()?;
                let password = rpassword_read()?;
                eprint!("Confirm server password: ");
                std::io::stderr().flush()?;
                let confirm = rpassword_read()?;
                if password != confirm {
                    eprintln!("Passwords do not match.");
                    std::process::exit(1);
                }
                if password.is_empty() {
                    eprintln!("Password cannot be empty.");
                    std::process::exit(1);
                }
                crate::security::set_server_password(&password)?;
                println!("Server password stored securely.");
            } else if let Some(args) = set_model {
                if args.len() >= 2 {
                    crate::config::set_model(&args[0], &args[1])?;
                } else {
                    eprintln!("Usage: --set-model <role> <model_id>");
                    println!("Available roles: {}", crate::config::ModelsConfig::roles().join(", "));
                }
            } else if let Some(role) = get_model {
                crate::config::get_model(&role)?;
            } else if list_models {
                crate::config::list_models()?;
            } else if show {
                crate::config::show_config()?;
            } else {
                println!("Configuration options:");
                println!("  --set-api-key <key>      Set your OpenRouter API key");
                println!("  --set-hf-api-key <key>   Set your Hugging Face API key");
                println!("  --set-nvidia-key <key>   Set your NVIDIA NIM API key");
                println!("  --set-password           Set server password for remote access");
                println!("  --show                   Display current configuration");
                println!("  --set-model <role> <id>  Set model for a role");
                println!("  --get-model <role>       Get model for a role");
                println!("  --list-models            List all model assignments");
                println!();
                println!("Model roles: orchestrator, code, research, analysis,");
                println!("             reasoning, file, general, skill-creator, chat");
                println!();
                println!("Example models:");
                println!("  moonshotai/kimi-k2.5          (orchestrator default)");
                println!("  qwen/qwen-2.5-coder-32b-instruct (code)");
                println!("  perplexity/sonar              (research)");
                println!("  deepseek/deepseek-r1          (reasoning)");
            }
        }
        Some(Commands::Doctor { fix, update }) => {
            crate::doctor::run_diagnostics(fix, update).await?;
        }
        Some(Commands::Memory { command }) => {
            match command {
                MemoryCommands::Stats => {
                    show_memory_stats().await?;
                }
                MemoryCommands::Search { query, limit } => {
                    semantic_search_memory(&query, limit).await?;
                }
                MemoryCommands::Add { content, source, importance } => {
                    add_knowledge(&content, &source, importance).await?;
                }
                MemoryCommands::List { limit } => {
                    list_knowledge(limit).await?;
                }
                MemoryCommands::Cleanup { keep } => {
                    cleanup_memory(keep).await?;
                }
                MemoryCommands::InitEmbeddings { force } => {
                    init_embeddings(force).await?;
                }
                MemoryCommands::Logs { date } => {
                    show_daily_logs(date.as_deref()).await?;
                }
                MemoryCommands::LogsCleanup { days } => {
                    cleanup_daily_logs(days)?;
                }
            }
        }
        Some(Commands::Skills { command }) => {
            match command {
                SkillCommands::List => {
                    crate::skills::list_skills()?;
                }
                SkillCommands::Create { name, description } => {
                    let desc = if description.is_empty() {
                        format!("{} skill", name)
                    } else {
                        description.clone()
                    };

                    let skills_dir = dirs::data_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join("my-agent")
                        .join("skills");
                    std::fs::create_dir_all(&skills_dir)?;

                    let file_path = skills_dir.join(format!("{}.skill.md", name));
                    if file_path.exists() {
                        println!("Skill '{}' already exists at: {}", name, file_path.display());
                    } else {
                        let template = crate::skills::markdown::generate_template(&name, &desc);
                        std::fs::write(&file_path, &template)?;
                        println!("Created skill template: {}", file_path.display());
                        println!("Edit the file to add your instructions, then use `my-agent skills list` to verify.");
                    }
                }
                SkillCommands::Show { id } => {
                    // Try to find in markdown skills first
                    let md_skills = crate::skills::markdown::load_markdown_skills();
                    if let Some(skill) = md_skills.iter().find(|s| s.id == id) {
                        println!("Skill: {} [markdown]", skill.frontmatter.name);
                        println!("  ID: {}", skill.id);
                        println!("  Description: {}", skill.frontmatter.description);
                        println!("  Version: {}", skill.frontmatter.version.as_deref().unwrap_or("1.0.0"));
                        if let Some(ref author) = skill.frontmatter.author {
                            println!("  Author: {}", author);
                        }
                        if let Some(ref cat) = skill.frontmatter.category {
                            println!("  Category: {}", cat);
                        }
                        if !skill.frontmatter.tags.is_empty() {
                            println!("  Tags: {}", skill.frontmatter.tags.join(", "));
                        }
                        if !skill.frontmatter.parameters.is_empty() {
                            println!("  Parameters:");
                            for p in &skill.frontmatter.parameters {
                                let req = if p.required { "required" } else { "optional" };
                                println!("    - {} ({}): {} [{}]", p.name, p.param_type, p.description, req);
                            }
                        }

                        // Check requirements
                        let missing = crate::skills::markdown::check_requirements(skill);
                        if missing.is_empty() {
                            println!("  Requirements: all met");
                        } else {
                            println!("  Requirements: UNMET");
                            for m in &missing {
                                println!("    - {}", m);
                            }
                        }

                        println!("  File: {}", skill.file_path.display());
                        println!("\n--- Body Preview ---");
                        // Show first 20 lines of the body
                        for (i, line) in skill.body.lines().enumerate() {
                            if i >= 20 {
                                println!("  ... ({} more lines)", skill.body.lines().count() - 20);
                                break;
                            }
                            println!("  {}", line);
                        }
                    } else {
                        // Check in the registry (built-in or Rhai skills)
                        let registry = crate::skills::default_registry();
                        if let Some(skill) = registry.get(&id) {
                            println!("Skill: {} [{}]", skill.meta.name,
                                if skill.meta.builtin { "built-in" } else { "rhai" });
                            println!("  ID: {}", skill.meta.id);
                            println!("  Description: {}", skill.meta.description);
                            println!("  Version: {}", skill.meta.version);
                            println!("  Category: {:?}", skill.meta.category);
                            println!("  Permissions: {:?}", skill.meta.permissions);
                            println!("  Tags: {}", skill.meta.tags.join(", "));
                        } else {
                            println!("Skill '{}' not found.", id);
                        }
                    }
                }
                SkillCommands::Install { name } => {
                    crate::skills::install_skill(&name).await?;
                }
                SkillCommands::Remove { name } => {
                    crate::skills::remove_skill(&name)?;
                }
            }
        }
        Some(Commands::Soul { command }) => {
            match command {
                SoulCommands::Start => {
                    crate::soul::start_heartbeat().await?;
                }
                SoulCommands::Stop => {
                    crate::soul::stop_heartbeat().await?;
                }
                SoulCommands::Status => {
                    crate::soul::show_status().await?;
                }
                SoulCommands::Review => {
                    crate::soul::review_improvements().await?;
                }
            }
        }
        Some(Commands::Serve { port, host, https, cert, key, tunnel }) => {
            println!("Starting web server on {}:{}", host, port);
            if https {
                println!("✓ HTTPS enabled");
                if let Some(ref cert_path) = cert {
                    println!("  Certificate: {}", cert_path);
                }
                if let Some(ref key_path) = key {
                    println!("  Private key: {}", key_path);
                }
            }

            if tunnel {
                // Spawn cloudflared tunnel in background
                let tunnel_port = port;
                tokio::spawn(async move {
                    // Small delay to let the server start binding
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let url = format!("http://localhost:{}", tunnel_port);
                    println!("Starting Cloudflare Tunnel to {}...", url);
                    match tokio::process::Command::new("cloudflared")
                        .args(["tunnel", "--url", &url])
                        .stderr(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::null())
                        .spawn()
                    {
                        Ok(mut child) => {
                            // Parse the tunnel URL from cloudflared stderr
                            if let Some(stderr) = child.stderr.take() {
                                use tokio::io::{AsyncBufReadExt, BufReader};
                                let mut reader = BufReader::new(stderr);
                                let mut line = String::new();
                                while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                                    if let Some(pos) = line.find("https://") {
                                        if line.contains(".trycloudflare.com") {
                                            let end = line[pos..].find(|c: char| c.is_whitespace() || c == '"').unwrap_or(line.len() - pos);
                                            let tunnel_url = &line[pos..pos + end];
                                            println!();
                                            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                                            println!("  Tunnel URL: {}", tunnel_url);
                                            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                                            println!();
                                            break;
                                        }
                                    }
                                    line.clear();
                                }
                            }
                            // Keep the child alive — it will be killed when the server exits
                            let _ = child.wait().await;
                        }
                        Err(e) => {
                            eprintln!("Failed to start cloudflared: {}", e);
                            eprintln!("Install it: https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/");
                        }
                    }
                });
            }

            crate::server::start(&host, port, https, cert, key).await?;
        }
        Some(Commands::Connect { server, name, token }) => {
            let device_name = name.unwrap_or_else(|| {
                std::process::Command::new("hostname")
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| "device".to_string())
            });
            println!("Connecting to {} as '{}'...", server, device_name);
            run_device_agent(&server, &device_name, &token).await?;
        }
        Some(Commands::Orchestrate { task, verbose, interactive, plan_only }) => {
            crate::orchestrator::cli::run_orchestrator(Some(task), verbose, interactive, plan_only).await?;
        }

        Some(Commands::Gateway { command }) => {
            match command {
                GatewayCommands::Start { port, host, no_web, messaging, no_soul } => {
                    let gw_config = crate::config::Config::load().unwrap_or_default().gateway;
                    let config = crate::gateway::GatewayConfig {
                        port,
                        host,
                        enable_web: !no_web,
                        enable_messaging: messaging,
                        auto_start_soul: !no_soul,
                        enable_heartbeat: gw_config.enable_heartbeat,
                        heartbeat: gw_config.heartbeat,
                    };
                    let mut gateway = crate::gateway::Gateway::with_config(config);
                    gateway.run().await?;
                }
                GatewayCommands::Status => {
                    let gateway = crate::gateway::Gateway::new();
                    let stats = gateway.stats().await;
                    println!("Gateway Status");
                    println!("==============");
                    println!("State: {}", stats.state);
                    println!("Uptime: {}s", stats.uptime_secs);
                    println!("Web: {}", if stats.web_enabled { "enabled" } else { "disabled" });
                    println!("Messaging: {}", if stats.messaging_enabled { "enabled" } else { "disabled" });
                    println!("Soul: {}", if stats.soul_running { "running" } else { "stopped" });
                    println!("Port: {}", stats.port);
                }
                GatewayCommands::Heartbeat { command } => {
                    match command {
                        HeartbeatCommands::Status => {
                            let config = crate::config::Config::load()?.gateway;
                            println!("Heartbeat Configuration");
                            println!("=======================");
                            println!("Enabled: {}", config.enable_heartbeat);
                            println!("Interval: {} minutes", config.heartbeat.interval_minutes);
                            println!("Active window: {} - {}", config.heartbeat.active_start, config.heartbeat.active_end);
                            println!("Max errors: {}", config.heartbeat.max_consecutive_errors);
                        }
                        HeartbeatCommands::Tick => {
                            println!("Running manual heartbeat tick...");
                            let config = crate::config::Config::load()?.gateway.heartbeat;
                            let mut hb = crate::gateway::heartbeat::CronHeartbeat::new(config)?;
                            let outcome = hb.tick().await;
                            println!("Result: {}", outcome);
                        }
                        HeartbeatCommands::Edit => {
                            let data_dir = crate::config::data_dir()?;
                            let path = data_dir.join("HEARTBEAT.md");
                            // Ensure default exists
                            let hb_config = crate::gateway::heartbeat::CronHeartbeatConfig::default();
                            let mut hb = crate::gateway::heartbeat::CronHeartbeat::new(hb_config)?;
                            let _ = hb.load_checklist()?;
                            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
                            let status = std::process::Command::new(&editor)
                                .arg(&path)
                                .status();
                            match status {
                                Ok(s) if s.success() => println!("Checklist updated."),
                                _ => eprintln!("Failed to open editor. Edit manually: {}", path.display()),
                            }
                        }
                    }
                }
            }
        }

        Some(Commands::Learning { command }) => {
            match command {
                LearningCommands::Stats => {
                    let store = crate::learning::LearningStore::new()?;
                    let learnings = store.get_all(&crate::learning::EntryType::Learning)?;
                    let errors = store.get_all(&crate::learning::EntryType::Error)?;
                    let features = store.get_all(&crate::learning::EntryType::FeatureRequest)?;
                    println!("Learning Statistics");
                    println!("==================");
                    println!("Learnings: {}", learnings.len());
                    println!("Errors: {}", errors.len());
                    println!("Feature Requests: {}", features.len());
                    println!();
                    let promoted = store.get_by_status(&crate::learning::EntryStatus::Promoted)?;
                    let validated = store.get_by_status(&crate::learning::EntryStatus::Validated)?;
                    println!("Promoted: {}", promoted.len());
                    println!("Validated: {}", validated.len());
                    println!();
                    println!("Store: {}", store.base_dir().display());
                }
                LearningCommands::Search { query } => {
                    let store = crate::learning::LearningStore::new()?;
                    let results = store.search(&query)?;
                    if results.is_empty() {
                        println!("No results for '{}'", query);
                    } else {
                        println!("Found {} results:", results.len());
                        for entry in &results {
                            println!("  {} [{}] ({}) — {}", entry.id, entry.status, entry.priority, entry.title);
                        }
                    }
                }
                LearningCommands::Review { status } => {
                    let store = crate::learning::LearningStore::new()?;
                    let entries = match status.as_str() {
                        "new" => store.get_by_status(&crate::learning::EntryStatus::New)?,
                        "validated" => store.get_by_status(&crate::learning::EntryStatus::Validated)?,
                        "promoted" => store.get_by_status(&crate::learning::EntryStatus::Promoted)?,
                        _ => {
                            let mut all = store.get_all(&crate::learning::EntryType::Learning)?;
                            all.extend(store.get_all(&crate::learning::EntryType::Error)?);
                            all.extend(store.get_all(&crate::learning::EntryType::FeatureRequest)?);
                            all
                        }
                    };
                    if entries.is_empty() {
                        println!("No entries found.");
                    } else {
                        println!("{} entries:", entries.len());
                        for entry in &entries {
                            println!("  {} [{}] ({}, {}) — {}", entry.id, entry.status, entry.priority, entry.area, entry.title);
                            if !entry.description.is_empty() {
                                let desc = crate::truncate_safe(&entry.description, 77);
                                println!("    {}", desc);
                            }
                        }
                    }
                }
                LearningCommands::Promote => {
                    let store = std::sync::Arc::new(crate::learning::LearningStore::new()?);
                    let bootstrap = std::sync::Arc::new(crate::learning::BootstrapContext::new()?);
                    let engine = crate::learning::PromotionEngine::new(store, bootstrap);
                    let count = engine.run_promotion_cycle()?;
                    println!("Promotion cycle complete: {} entries promoted", count);
                }
                LearningCommands::Init => {
                    let bootstrap = crate::learning::BootstrapContext::new()?;
                    bootstrap.seed_defaults()?;
                    println!("Bootstrap context files initialized at: {}", bootstrap.base_dir().display());
                }
            }
        }

        Some(Commands::Pipeline { command }) => {
            match command {
                PipelineCommands::Run { yaml } => {
                    let path = std::path::Path::new(&yaml);
                    if !path.exists() {
                        // Try in pipelines directory
                        let alt = crate::config::data_dir()?.join("pipelines").join(&yaml);
                        if alt.exists() {
                            run_pipeline(&alt).await?;
                        } else {
                            eprintln!("Pipeline file not found: {}", yaml);
                            eprintln!("Try placing it in ~/.local/share/my-agent/pipelines/");
                        }
                    } else {
                        run_pipeline(path).await?;
                    }
                }
                PipelineCommands::List => {
                    let files = crate::orchestrator::pipeline::list_pipeline_files()?;
                    if files.is_empty() {
                        println!("No pipelines found.");
                        println!("Place YAML pipeline files in ~/.local/share/my-agent/pipelines/");
                    } else {
                        println!("Available Pipelines:");
                        for f in &files {
                            let name = f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                            println!("  {}", name);
                        }
                    }
                }
                PipelineCommands::Approve { name } => {
                    let dir = crate::config::data_dir()?.join("pipelines");
                    let yaml_path = dir.join(format!("{}.yaml", name));
                    let state_path = dir.join(format!("{}.state.json", name));

                    if !yaml_path.exists() {
                        eprintln!("Pipeline '{}' not found", name);
                    } else if !state_path.exists() {
                        eprintln!("No saved state for pipeline '{}'. Run it first.", name);
                    } else {
                        let def_content = std::fs::read_to_string(&yaml_path)?;
                        let definition: crate::orchestrator::pipeline::PipelineDefinition = serde_yaml::from_str(&def_content)?;
                        let mut executor = crate::orchestrator::pipeline::PipelineExecutor::resume_from_state(&state_path, definition)?;
                        let state = executor.approve_and_continue().await?;
                        println!("Pipeline '{}' status: {:?}", name, state.status);
                    }
                }
                PipelineCommands::Status { name } => {
                    let state_path = crate::config::data_dir()?.join("pipelines").join(format!("{}.state.json", name));
                    if state_path.exists() {
                        let content = std::fs::read_to_string(&state_path)?;
                        let state: crate::orchestrator::pipeline::PipelineState = serde_json::from_str(&content)?;
                        println!("Pipeline: {}", state.pipeline_name);
                        println!("Status: {:?}", state.status);
                        println!("Step: {}/{}", state.current_step_index, state.step_results.len());
                        println!("Started: {}", state.started_at);
                        println!("Updated: {}", state.updated_at);
                        for (id, result) in &state.step_results {
                            println!("  {}: {} ({:.1}s)", id, if result.success { "OK" } else { "FAIL" }, result.duration_secs);
                        }
                    } else {
                        println!("No state found for pipeline '{}'. Run it first.", name);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Run a pipeline from a YAML file
async fn run_pipeline(path: &std::path::Path) -> Result<()> {
    println!("Loading pipeline from {}...", path.display());
    let mut executor = crate::orchestrator::pipeline::PipelineExecutor::from_yaml(path)?;
    let def = executor.definition().clone();
    println!("Pipeline: {} ({} steps)", def.name, def.steps.len());
    if !def.description.is_empty() {
        println!("  {}", def.description);
    }
    println!();

    let state = executor.execute().await?;
    println!();
    println!("Pipeline '{}' finished with status: {:?}", def.name, state.status);
    for (id, result) in &state.step_results {
        let status = if result.success { "OK" } else { "FAIL" };
        println!("  {} [{}] ({:.1}s)", id, status, result.duration_secs);
        if !result.success {
            println!("    {}", result.output);
        }
    }
    Ok(())
}

/// Show a specific conversation
async fn show_conversation(id: &str) -> Result<()> {
    let store = crate::memory::MemoryStore::default_store().await?;

    match store.load_conversation(id).await? {
        Some(record) => {
            println!("\n=== Conversation: {} ===", id);
            if let Some(ref title) = record.title {
                println!("Title: {}", title);
            }
            println!("Messages: {}", record.messages.len());
            println!("Created: {}", record.created_at.format("%Y-%m-%d %H:%M:%S"));
            println!("Updated: {}", record.updated_at.format("%Y-%m-%d %H:%M:%S"));
            println!();

            for msg in &record.messages {
                let role = match msg.role {
                    crate::agent::conversation::Role::User => "👤 User",
                    crate::agent::conversation::Role::Assistant => "🤖 Assistant",
                    crate::agent::conversation::Role::System => "⚙️ System",
                };
                println!("{}: {}", role, msg.content);
                println!();
            }
        }
        None => {
            eprintln!("Conversation not found: {}", id);
            eprintln!("Use 'my-agent history list' to see available conversations.");
        }
    }

    Ok(())
}

/// Delete a conversation
async fn delete_conversation(id: &str) -> Result<()> {
    let store = crate::memory::MemoryStore::default_store().await?;

    // Verify the conversation exists first
    match store.load_conversation(id).await? {
        Some(record) => {
            let title = record.title.as_deref().unwrap_or("Untitled");
            println!("About to delete conversation: {} ({})", title, crate::truncate_safe(id, 8));
            println!("  Messages: {}", record.messages.len());
            println!("  Created: {}", record.created_at.format("%Y-%m-%d %H:%M:%S"));
            println!();
            println!("Are you sure? [y/N]:");

            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;

            if input.trim().to_lowercase() != "y" && input.trim().to_lowercase() != "yes" {
                println!("Cancelled.");
                return Ok(());
            }

            match store.delete_conversation(id).await {
                Ok(()) => println!("Conversation deleted: {}", crate::truncate_safe(id, 8)),
                Err(e) => eprintln!("Failed to delete conversation: {}", e),
            }
        }
        None => {
            eprintln!("Conversation not found: {}", id);
            eprintln!("Use 'my-agent history list' to see available conversations.");
        }
    }

    Ok(())
}

/// Clear all conversation history
async fn clear_history(skip_confirm: bool) -> Result<()> {
    let store = crate::memory::MemoryStore::default_store().await?;
    let stats = store.sqlite().stats().await?;

    if stats.total_conversations == 0 {
        println!("No conversation history found.");
        return Ok(());
    }

    if !skip_confirm {
        println!("This will delete ALL {} conversation(s) and {} message(s)!",
            stats.total_conversations, stats.total_messages);
        println!("Knowledge entries ({}) will be preserved.", stats.total_knowledge);
        println!();
        println!("Type 'yes' to confirm:");

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        if input.trim().to_lowercase() != "yes" {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let deleted = store.sqlite().cleanup_old(0).await?;
    println!("Cleared {} conversation(s).", deleted);

    Ok(())
}

/// Show memory statistics
async fn show_memory_stats() -> Result<()> {
    let store = crate::memory::MemoryStore::default_store().await?;
    let stats = store.sqlite().stats().await?;

    println!("\n📊 Memory Database Statistics");
    println!("═══════════════════════════════════════");
    println!("  Total conversations:   {}", stats.total_conversations);
    println!("  Total messages:        {}", stats.total_messages);
    println!("  Knowledge entries:     {}", stats.total_knowledge);
    println!("  With embeddings:       {}", stats.conversations_with_embeddings);
    println!();

    if let Some(ref oldest) = stats.oldest_conversation {
        println!("  Oldest: {}", oldest);
    }
    if let Some(ref newest) = stats.newest_conversation {
        println!("  Newest: {}", newest);
    }

    // Check embedding model status
    if store.has_embeddings() {
        println!("\n✅ Embedding model initialized");
        if let Some(model) = store.embedding_model() {
            println!("   Model: {}", model.model_name());
            println!("   Dimension: {}", model.dimension());
            if model.uses_real_embeddings() {
                println!("   Mode: Real API embeddings");
            } else {
                println!("   Mode: Local hash-based (fallback)");
            }
        }
    } else {
        println!("\n⚠️  Embedding model not initialized");
        println!("   Run 'my-agent memory init-embeddings' to enable semantic search");
    }

    Ok(())
}

/// Semantic search in memory
async fn semantic_search_memory(query: &str, limit: usize) -> Result<()> {
    let store = crate::memory::MemoryStore::default_store().await?;

    if !store.has_embeddings() {
        println!("⚠️  Embedding model not initialized. Using keyword search.");
        let results = store.search_conversations(query, limit).await?;
        display_search_results(&results);
        return Ok(());
    }

    println!("🔍 Semantic search for: \"{}\"", query);
    println!();

    // Search conversations
    let results = store.semantic_search(query, limit).await?;
    display_semantic_results(&results);

    // Also search knowledge
    println!("\n📚 Knowledge base:");
    let knowledge = store.search_knowledge(query, 3).await?;
    for (entry, score) in &knowledge {
        println!("  [{:.2}] {} (from: {})", score, entry.content, entry.source);
    }

    Ok(())
}

/// Display search results
fn display_search_results(results: &[crate::memory::ConversationRecord]) {
    if results.is_empty() {
        println!("No results found.");
        return;
    }

    println!("Found {} conversation(s):\n", results.len());
    for (i, record) in results.iter().enumerate() {
        println!("{}. {} ({})",
            i + 1,
            record.title.as_ref().map(|s| s.as_str()).unwrap_or("Untitled"),
            &record.id[..8]
        );
        if !record.messages.is_empty() {
            let preview: String = record.messages[0].content.chars().take(80).collect();
            println!("   \"{}...\"", preview);
        }
        println!();
    }
}

/// Display semantic search results with scores
fn display_semantic_results(results: &[(crate::memory::ConversationRecord, f32)]) {
    if results.is_empty() {
        println!("No results found.");
        return;
    }

    println!("Found {} conversation(s):\n", results.len());
    for (i, (record, score)) in results.iter().enumerate() {
        println!("{}. [{:.2}] {} ({})",
            i + 1,
            score,
            record.title.as_ref().map(|s| s.as_str()).unwrap_or("Untitled"),
            &record.id[..8]
        );
        if !record.messages.is_empty() {
            let preview: String = record.messages[0].content.chars().take(80).collect();
            println!("   \"{}...\"", preview);
        }
        println!();
    }
}

/// Add knowledge to the database
async fn add_knowledge(content: &str, source: &str, importance: f32) -> Result<()> {
    let store = crate::memory::MemoryStore::default_store().await?;

    let id = store.add_knowledge(content, source, importance).await?;
    println!("✓ Knowledge added with ID: {}", &id[..8]);
    println!("  Content: {}", content);
    println!("  Source: {}", source);
    println!("  Importance: {}", importance);

    Ok(())
}

/// List knowledge entries
async fn list_knowledge(limit: usize) -> Result<()> {
    let store = crate::memory::MemoryStore::default_store().await?;
    let stats = store.sqlite().stats().await?;

    println!("\nKnowledge Base ({} entries)", stats.total_knowledge);
    println!("=======================================");

    if stats.total_knowledge == 0 {
        println!("  No knowledge entries yet.");
        println!("  Use 'my-agent memory add --content \"...\"' to add knowledge.");
        return Ok(());
    }

    let entries = store.list_knowledge(limit, 0).await?;

    for (i, entry) in entries.iter().enumerate() {
        let content_preview = if entry.content.len() > 80 {
            format!("{}...", entry.content.chars().take(80).collect::<String>())
        } else {
            entry.content.clone()
        };
        println!("{}. [{}] (importance: {:.1}, source: {})",
            i + 1, crate::truncate_safe(&entry.id, 8), entry.importance, entry.source);
        println!("   {}", content_preview);
        println!("   Created: {}  Accessed: {} times",
            entry.created_at.format("%Y-%m-%d %H:%M"), entry.access_count);
        println!();
    }

    if stats.total_knowledge > limit {
        println!("Showing {} of {}. Use --limit to see more.", limit, stats.total_knowledge);
    }

    Ok(())
}

/// Clean up old conversations
async fn cleanup_memory(keep: usize) -> Result<()> {
    let store = crate::memory::MemoryStore::default_store().await?;
    let stats = store.sqlite().stats().await?;

    println!("Current conversations: {}", stats.total_conversations);
    println!("Will keep: {} most recent", keep);

    if stats.total_conversations <= keep {
        println!("No cleanup needed.");
        return Ok(());
    }

    let deleted = store.sqlite().cleanup_old(keep).await?;
    println!("✓ Cleaned up {} old conversation(s)", deleted);

    Ok(())
}

/// Initialize embedding model
async fn init_embeddings(force: bool) -> Result<()> {
    if force {
        println!("Force re-initializing embedding model...");
    } else {
        println!("Initializing embedding model...");
    }

    let model = crate::memory::EmbeddingModel::with_keyring_key().await?;

    // Test embedding
    println!("\nTesting embedding...");
    let test_embedding = model.embed("Hello, world!").await?;
    println!("✓ Embedding generated successfully");
    println!("  Dimension: {}", test_embedding.len());
    println!("  Model: {}", model.model_name());
    println!("  Uses real embeddings: {}", model.uses_real_embeddings());

    Ok(())
}

/// Run as a device agent, connecting to a remote server
async fn run_device_agent(server_url: &str, device_name: &str, token: &str) -> Result<()> {
    use tokio_tungstenite::{connect_async, tungstenite::Message as WsMsg};
    use futures_util::{SinkExt, StreamExt};

    // Build WebSocket URL
    let ws_scheme = if server_url.starts_with("https://") { "wss" } else { "ws" };
    let host = server_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let platform = std::env::consts::OS;
    let ws_url = format!(
        "{}://{}/ws/device-agent?name={}&platform={}&token={}",
        ws_scheme, host, device_name, platform, token
    );

    println!("Connecting to WebSocket...");
    let (ws_stream, _) = connect_async(&ws_url).await
        .map_err(|e| anyhow::anyhow!("Failed to connect to server: {}", e))?;

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Determine capabilities — what tools this device supports
    let mut capabilities: Vec<&str> = vec![
        "read_file", "write_file", "list_directory", "search_files",
        "run_command", "open_application",
    ];
    #[cfg(feature = "desktop")]
    {
        capabilities.extend_from_slice(&[
            "capture_screen", "mouse_click", "mouse_double_click",
            "mouse_scroll", "mouse_drag", "keyboard_type",
            "keyboard_press", "keyboard_hotkey",
        ]);
    }
    let caps: Vec<String> = capabilities.iter().map(|s| s.to_string()).collect();

    // Send capabilities as first message
    let caps_json = serde_json::to_string(&caps)?;
    ws_tx.send(WsMsg::Text(caps_json.into())).await
        .map_err(|e| anyhow::anyhow!("Failed to send capabilities: {}", e))?;

    println!("Waiting for confirmation...");

    // Wait for connection confirmation
    if let Some(Ok(msg)) = ws_rx.next().await {
        if let WsMsg::Text(text) = msg {
            println!("Server: {}", text);
        }
    }

    println!("Connected as '{}' ({}) with {} capabilities", device_name, platform, caps.len());
    println!("Capabilities: {}", caps.join(", "));
    println!("Waiting for tool calls... (Ctrl+C to disconnect)");

    // Create local tool context for executing tools
    let tool_ctx = crate::agent::tools::ToolContext::with_project_paths();

    // Listen for tool requests and execute them
    while let Some(Ok(msg)) = ws_rx.next().await {
        if let WsMsg::Text(text) = msg {
            let text_str: &str = &text;
            match serde_json::from_str::<crate::server::device::DeviceToolRequest>(text_str) {
                Ok(request) => {
                    println!("[{}] Executing: {} ({})",
                        crate::truncate_safe(&request.request_id, 8),
                        request.tool_name,
                        request.arguments);

                    // Execute the tool locally
                    let call = crate::agent::tools::ToolCall {
                        name: request.tool_name.clone(),
                        arguments: request.arguments,
                    };

                    let response = match crate::agent::tools::execute_tool(&call, &tool_ctx).await {
                        Ok(result) => crate::server::device::DeviceToolResponse {
                            request_id: request.request_id,
                            success: result.success,
                            message: result.message,
                            data: result.data,
                        },
                        Err(e) => crate::server::device::DeviceToolResponse {
                            request_id: request.request_id,
                            success: false,
                            message: format!("Error: {}", e),
                            data: None,
                        },
                    };

                    println!("[{}] Result: {} — {}",
                        crate::truncate_safe(&response.request_id, 8),
                        if response.success { "OK" } else { "FAIL" },
                        crate::truncate_safe(&response.message, 100));

                    let response_json = serde_json::to_string(&response)?;
                    ws_tx.send(WsMsg::Text(response_json.into())).await
                        .map_err(|e| anyhow::anyhow!("Failed to send response: {}", e))?;
                }
                Err(e) => {
                    eprintln!("Invalid message from server: {}", e);
                }
            }
        }
    }

    println!("Disconnected from server.");
    Ok(())
}

/// Read a password from stdin with echo disabled (Unix) or simple fallback
fn rpassword_read() -> Result<String> {
    #[cfg(unix)]
    {
        use std::io::BufRead;
        // Disable echo
        let fd = 0; // stdin
        unsafe {
            let mut termios: libc::termios = std::mem::zeroed();
            libc::tcgetattr(fd, &mut termios);
            let original = termios;
            termios.c_lflag &= !libc::ECHO;
            libc::tcsetattr(fd, libc::TCSANOW, &termios);

            let mut line = String::new();
            let result = std::io::stdin().lock().read_line(&mut line);

            // Restore echo
            libc::tcsetattr(fd, libc::TCSANOW, &original);
            eprintln!(); // newline after hidden input

            result?;
            Ok(line.trim().to_string())
        }
    }
    #[cfg(not(unix))]
    {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line.trim().to_string())
    }
}

/// Show daily logs
async fn show_daily_logs(date: Option<&str>) -> Result<()> {
    let mgr = crate::memory::daily_log::DailyLogManager::new()?;

    match date {
        Some("today") | None => {
            let content = mgr.load_today();
            if content.is_empty() {
                println!("No log entries for today.");
            } else {
                println!("{}", content);
            }
        }
        Some("yesterday") => {
            let content = mgr.load_yesterday();
            if content.is_empty() {
                println!("No log entries for yesterday.");
            } else {
                println!("{}", content);
            }
        }
        Some("list") => {
            let dates = mgr.list_logs()?;
            if dates.is_empty() {
                println!("No daily logs found.");
            } else {
                println!("Daily Logs ({} files):", dates.len());
                for date in &dates {
                    println!("  {}", date.format("%Y-%m-%d"));
                }
            }
        }
        Some(date_str) => {
            match chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                Ok(date) => {
                    let content = mgr.load_date(date);
                    if content.is_empty() {
                        println!("No log entries for {}.", date_str);
                    } else {
                        println!("{}", content);
                    }
                }
                Err(_) => {
                    eprintln!("Invalid date format. Use YYYY-MM-DD, 'today', 'yesterday', or 'list'.");
                }
            }
        }
    }

    Ok(())
}

/// Clean up old daily logs
fn cleanup_daily_logs(keep_days: u32) -> Result<()> {
    let mgr = crate::memory::daily_log::DailyLogManager::new()?;
    let removed = mgr.cleanup(keep_days)?;
    println!("Removed {} old log file(s), keeping last {} days.", removed, keep_days);
    Ok(())
}
