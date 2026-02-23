//! Soul module - heartbeat engine and updateable soul
//!
//! The soul is the autonomous core of the agent that can act without prompts.
//! It integrates:
//! - Heartbeat engine (periodic checks)
//! - Proactive actions (trigger-based autonomous actions)
//! - Task scheduler (cron-based scheduling)
//! - File watcher (react to file system changes)
//! - Personality configuration

pub mod engine;
pub mod heartbeat;
pub mod scheduler;
pub mod watcher;
pub mod proactive;
pub mod personality;
pub mod system_prompts;

// Re-export main types from engine
pub use engine::{SoulEngine, SoulState, SoulStats, SoulMessage, ToolContext as EngineToolContext, get_soul_stats};

// Re-export heartbeat types
pub use heartbeat::{
    HeartbeatEngine, HeartbeatConfig, HeartbeatStats, EngineState, EngineCommand, EngineResponse,
    ServiceHealth, ToolHealth,
};

// Re-export other soul types
pub use proactive::{ProactiveAction, Priority, Trigger, ActionResult};
pub use scheduler::{ScheduledTask, TaskSchedule, TaskScheduler, TaskResult};
pub use watcher::{FileWatcher, WatchConfig, FileEvent, FileSystemEvent};

// Re-export personality types
pub use personality::{Personality, CommunicationStyle, BehaviorRule, TaskResponses};

use anyhow::Result;

/// Start the heartbeat engine (runs in foreground until Ctrl+C)
pub async fn start_heartbeat() -> Result<()> {
    use tokio::signal;

    println!("Starting soul engine...");

    // Create tools context
    let tools = EngineToolContext::new()?;

    // Create and start the engine with tools
    let mut engine = SoulEngine::with_tools(Some(tools));
    engine.start().await?;

    // Get stats directly from the engine
    let stats = engine.stats().await;

    println!("Soul engine started successfully:");
    println!("  State: {}", stats.state);
    println!("  Proactive actions: {}", stats.proactive_actions_registered);
    println!();
    println!("Press Ctrl+C to stop.");

    // Wait for Ctrl+C
    match signal::ctrl_c().await {
        Ok(()) => {
            println!("\nStopping soul engine...");
            engine.stop().await?;
            println!("Soul engine stopped.");
        }
        Err(err) => {
            eprintln!("Unable to listen for shutdown signal: {}", err);
        }
    }

    Ok(())
}

/// Stop the heartbeat engine
pub async fn stop_heartbeat() -> Result<()> {
    engine::stop_soul().await
}

/// Show soul status
pub async fn show_status() -> Result<()> {
    println!("Soul Status");
    println!("===========");

    let stats = engine::get_soul_stats().await;

    if let Some(stats) = stats {
        println!("State: {}", stats.state);
        println!("Uptime: {} seconds", stats.uptime_secs);
        println!("Proactive actions: {}", stats.proactive_actions_registered);
        println!("Scheduled tasks: {}", stats.scheduled_tasks);
        println!("File watches: {}", stats.file_watches);
        println!("Actions executed: {}", stats.actions_executed);
        println!("Actions successful: {}", stats.actions_successful);
        if let Some(last) = stats.last_action {
            println!("Last action: {}", last.format("%Y-%m-%d %H:%M:%S UTC"));
        }
    } else {
        println!("Soul engine is not running");
        println!();
        println!("Run 'my-agent soul start' to start the engine.");
    }

    Ok(())
}

/// Review suggested improvements
pub async fn review_improvements() -> Result<()> {
    println!("Reviewing suggested improvements...");
    println!();

    // Check if soul is running
    let stats = engine::get_soul_stats().await;

    if stats.is_none() {
        println!("Soul engine is not running.");
        println!("Start it with 'my-agent soul start' to enable self-improvement.");
        return Ok(());
    }

    // In a full implementation, this would:
    // 1. Analyze action success rates
    // 2. Identify patterns in failed actions
    // 3. Suggest new proactive actions
    // 4. Propose parameter adjustments

    println!("Self-improvement analysis:");
    println!("---------------------------");

    if let Some(s) = stats {
        let success_rate = if s.actions_executed > 0 {
            (s.actions_successful as f64 / s.actions_executed as f64) * 100.0
        } else {
            0.0
        };

        println!("Success rate: {:.1}%", success_rate);
        println!("Total actions: {}", s.actions_executed);

        if success_rate < 80.0 && s.actions_executed > 10 {
            println!();
            println!("Suggestion: Consider reviewing failing actions for issues.");
        }

        if s.proactive_actions_registered < 3 {
            println!();
            println!("Suggestion: Add more proactive actions to increase autonomy.");
        }
    }

    Ok(())
}

/// Create a simple interval-based proactive action
pub fn create_interval_action(name: &str, interval_secs: u64, description: &str) -> ProactiveAction {
    ProactiveAction::new(name, Trigger::Interval(interval_secs))
        .with_description(description)
}

/// Create a cron-based proactive action
pub fn create_cron_action(name: &str, cron_expr: &str, description: &str) -> ProactiveAction {
    ProactiveAction::new(name, Trigger::Time(cron_expr.to_string()))
        .with_description(description)
}

/// Create a file-watch proactive action
pub fn create_file_watch_action(name: &str, path: &str, event: &str, description: &str) -> ProactiveAction {
    ProactiveAction::new(name, Trigger::FileChange {
        path: path.to_string(),
        event: event.to_string(),
    })
    .with_description(description)
}

/// Create a scheduled task that executes a shell command
pub fn create_shell_task(
    name: &str,
    command: &str,
    schedule: TaskSchedule,
) -> (ScheduledTask, impl Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send>>) {
    use crate::tools::shell::execute;

    let task = ScheduledTask {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        schedule,
        description: Some(format!("Shell task: {}", command)),
        enabled: true,
        last_run: None,
        next_run: None,
        run_count: 0,
        max_runs: None,
        tags: vec!["shell".to_string(), "auto".to_string()],
    };

    let cmd = command.to_string();
    let executor = move || {
        let cmd = cmd.clone();
        Box::pin(async move {
            match execute(&cmd).await {
                Ok(result) => {
                    let status = if result.exit_code == Some(0) { "success" } else { "failed" };
                    Ok(format!("Exit code {:?}: {}", result.exit_code, status))
                }
                Err(e) => Err(e),
            }
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send>>
    };

    (task, executor)
}

/// Create a scheduled task that monitors a directory
pub fn create_directory_monitor_task(
    name: &str,
    path: &str,
    interval_secs: u64,
) -> (ScheduledTask, impl Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send>>) {
    use crate::tools::filesystem::list_directory;

    let task = ScheduledTask {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        schedule: TaskSchedule::Interval(interval_secs),
        description: Some(format!("Monitor directory: {}", path)),
        enabled: true,
        last_run: None,
        next_run: None,
        run_count: 0,
        max_runs: None,
        tags: vec!["filesystem".to_string(), "monitor".to_string()],
    };

    let path = path.to_string();
    let executor = move || {
        let path = path.clone();
        Box::pin(async move {
            match list_directory(&path).await {
                Ok(entries) => Ok(format!(
                    "Directory {} contains {} items",
                    path,
                    entries.len()
                )),
                Err(e) => Err(e),
            }
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send>>
    };

    (task, executor)
}

/// Create a scheduled task that fetches a URL
pub fn create_web_check_task(
    name: &str,
    url: &str,
    interval_secs: u64,
) -> (ScheduledTask, impl Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send>>) {
    use crate::tools::web::check_url;

    let task = ScheduledTask {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        schedule: TaskSchedule::Interval(interval_secs),
        description: Some(format!("Check URL: {}", url)),
        enabled: true,
        last_run: None,
        next_run: None,
        run_count: 0,
        max_runs: None,
        tags: vec!["web".to_string(), "monitor".to_string()],
    };

    let url = url.to_string();
    let executor = move || {
        let url = url.clone();
        Box::pin(async move {
            let start = std::time::Instant::now();
            match check_url(&url).await {
                Ok(status) => {
                    let duration_ms = start.elapsed().as_millis() as u64;
                    let is_up = (200..300).contains(&status);
                    Ok(format!(
                        "URL {} is {} (status: {}, {}ms)",
                        url,
                        if is_up { "up" } else { "down" },
                        status,
                        duration_ms
                    ))
                }
                Err(e) => Err(e),
            }
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send>>
    };

    (task, executor)
}
