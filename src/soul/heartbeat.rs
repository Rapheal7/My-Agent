//! Heartbeat engine - Autonomous agent core
//!
//! The heartbeat engine is the autonomous core of the agent that operates
//! independently of user prompts. It enables the agent to:
//! - Monitor system health and resources
//! - Execute scheduled tasks
//! - Watch files and react to changes
//! - Take proactive actions based on triggers
//! - Self-heal and maintain operational state

use anyhow::{Result, Context};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use tokio::time::Duration;
use tracing::{info, warn, error, debug};
use uuid::Uuid;

use crate::tools::{FileSystemTool, ShellTool, WebTool};
use super::scheduler::{TaskScheduler, ScheduledTask};
use super::proactive::{ProactiveEngine, ProactiveAction, Trigger, Priority};
use super::watcher::{FileWatcher, WatchConfig, FileSystemEvent};

/// Tool context for executing autonomous actions
#[derive(Clone)]
pub struct ToolContext {
    pub filesystem: FileSystemTool,
    pub shell: ShellTool,
    pub web: WebTool,
}

impl ToolContext {
    /// Create a new tool context with default configurations
    pub fn new() -> Result<Self> {
        Ok(Self {
            filesystem: FileSystemTool::new(),
            shell: ShellTool::new(),
            web: WebTool::new().context("Failed to create WebTool")?,
        })
    }

    /// Check if all tools are healthy
    pub async fn health_check(&self) -> ToolHealth {
        let mut health = ToolHealth::default();

        // Check filesystem by reading a known directory
        match self.filesystem.sandbox().is_allowed(Path::new("/")) {
            true => health.filesystem = ServiceHealth::Healthy,
            false => health.filesystem = ServiceHealth::Degraded("Sandbox restricted".to_string()),
        }

        // Check shell by running a simple command
        match self.shell.execute("echo heartbeat").await {
            Ok(result) if result.exit_code == Some(0) => health.shell = ServiceHealth::Healthy,
            Ok(result) => health.shell = ServiceHealth::Degraded(format!("Exit code: {:?}", result.exit_code)),
            Err(e) => health.shell = ServiceHealth::Unhealthy(e.to_string()),
        }

        // Check web by fetching a reliable endpoint
        match self.web.fetch_text("https://httpbin.org/get").await {
            Ok(_) => health.web = ServiceHealth::Healthy,
            Err(e) => health.web = ServiceHealth::Degraded(format!("Web check failed: {}", e)),
        }

        health.overall = if health.filesystem.is_healthy()
            && health.shell.is_healthy()
            && health.web.is_healthy() {
            ServiceHealth::Healthy
        } else if health.filesystem.is_healthy() && health.shell.is_healthy() {
            ServiceHealth::Degraded("Some services degraded".to_string())
        } else {
            ServiceHealth::Unhealthy("Critical services unavailable".to_string())
        };

        health
    }
}

impl Default for ToolContext {
    fn default() -> Self {
        Self::new().expect("Failed to create default ToolContext")
    }
}

/// Health status of a service
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ServiceHealth {
    Healthy,
    Degraded(String),
    Unhealthy(String),
}

impl ServiceHealth {
    pub fn is_healthy(&self) -> bool {
        matches!(self, ServiceHealth::Healthy)
    }

    pub fn is_operational(&self) -> bool {
        !matches!(self, ServiceHealth::Unhealthy(_))
    }
}

impl Default for ServiceHealth {
    fn default() -> Self {
        ServiceHealth::Healthy
    }
}

/// Health status of all tools
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolHealth {
    pub overall: ServiceHealth,
    pub filesystem: ServiceHealth,
    pub shell: ServiceHealth,
    pub web: ServiceHealth,
    pub checked_at: Option<DateTime<Utc>>,
}

/// Heartbeat engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    /// Base interval between heartbeat ticks (seconds)
    pub interval_secs: u64,
    /// Health check interval (ticks)
    pub health_check_interval: u64,
    /// Max consecutive failures before degrading
    pub max_consecutive_failures: u32,
    /// Enable automatic recovery
    pub auto_recovery: bool,
    /// Enable verbose logging
    pub verbose: bool,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval_secs: 10,
            health_check_interval: 6, // Every minute (6 * 10 seconds)
            max_consecutive_failures: 3,
            auto_recovery: true,
            verbose: false,
        }
    }
}

/// Heartbeat engine statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeartbeatStats {
    pub ticks: u64,
    pub actions_executed: u64,
    pub actions_successful: u64,
    pub tasks_executed: u64,
    pub tasks_successful: u64,
    pub events_processed: u64,
    pub health_checks: u64,
    pub consecutive_failures: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub last_tick: Option<DateTime<Utc>>,
    pub current_health: Option<ToolHealth>,
}

/// Engine state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineState {
    Stopped,
    Starting,
    Running,
    Paused,
    Degraded,
    Stopping,
}

impl std::fmt::Display for EngineState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineState::Stopped => write!(f, "Stopped"),
            EngineState::Starting => write!(f, "Starting"),
            EngineState::Running => write!(f, "Running"),
            EngineState::Paused => write!(f, "Paused"),
            EngineState::Degraded => write!(f, "Degraded"),
            EngineState::Stopping => write!(f, "Stopping"),
        }
    }
}

/// Command messages for the heartbeat engine
#[derive(Debug, Clone)]
pub enum EngineCommand {
    /// Pause the engine
    Pause,
    /// Resume the engine
    Resume,
    /// Execute a command immediately
    ExecuteNow(String),
    /// Register a proactive action
    RegisterAction(ProactiveAction),
    /// Schedule a task
    ScheduleTask(ScheduledTask),
    /// Add a file watch
    AddWatch(WatchConfig),
    /// Get current stats
    GetStats,
    /// Shutdown the engine
    Shutdown,
}

/// Response from the engine
#[derive(Debug, Clone)]
pub enum EngineResponse {
    Stats(HeartbeatStats),
    ActionRegistered(String),
    TaskScheduled(String),
    WatchAdded(String),
    Ack,
    Error(String),
}

/// Action registry entry
struct ActionEntry {
    action: ProactiveAction,
    executor: Box<dyn Fn(&ToolContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send>> + Send + Sync>,
}

/// The heartbeat engine - autonomous agent core
pub struct HeartbeatEngine {
    config: HeartbeatConfig,
    state: Arc<RwLock<EngineState>>,
    stats: Arc<RwLock<HeartbeatStats>>,
    tools: Arc<ToolContext>,
    proactive: Arc<ProactiveEngine>,
    scheduler: Arc<TaskScheduler>,
    watcher: Arc<FileWatcher>,
    command_tx: mpsc::Sender<(EngineCommand, Option<mpsc::Sender<EngineResponse>>)>,
    command_rx: Option<mpsc::Receiver<(EngineCommand, Option<mpsc::Sender<EngineResponse>>)>>,
    shutdown_tx: broadcast::Sender<()>,
    registered_actions: Arc<Mutex<HashMap<String, ActionEntry>>>,
}

impl HeartbeatEngine {
    /// Create a new heartbeat engine with default config
    pub fn new() -> Result<Self> {
        Self::with_config(HeartbeatConfig::default())
    }

    /// Create a new heartbeat engine with custom config
    pub fn with_config(config: HeartbeatConfig) -> Result<Self> {
        let tools = Arc::new(ToolContext::new()?);
        Self::with_tools_and_config(tools, config)
    }

    /// Create a new heartbeat engine with tools and config
    pub fn with_tools_and_config(tools: Arc<ToolContext>, config: HeartbeatConfig) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel(100);
        let (shutdown_tx, _) = broadcast::channel(1);

        Ok(Self {
            config,
            state: Arc::new(RwLock::new(EngineState::Stopped)),
            stats: Arc::new(RwLock::new(HeartbeatStats::default())),
            tools,
            proactive: Arc::new(ProactiveEngine::new()),
            scheduler: Arc::new(TaskScheduler::new()),
            watcher: Arc::new(FileWatcher::new()),
            command_tx,
            command_rx: Some(command_rx),
            shutdown_tx,
            registered_actions: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Get the command sender
    pub fn command_sender(&self) -> mpsc::Sender<(EngineCommand, Option<mpsc::Sender<EngineResponse>>)> {
        self.command_tx.clone()
    }

    /// Get current state
    pub async fn state(&self) -> EngineState {
        *self.state.read().await
    }

    /// Get current stats
    pub async fn stats(&self) -> HeartbeatStats {
        self.stats.read().await.clone()
    }

    /// Start the heartbeat engine
    pub async fn start(&mut self) -> Result<()> {
        // Check if already running
        {
            let state = self.state.read().await;
            if *state != EngineState::Stopped {
                warn!("Heartbeat engine already running (state: {})", state);
                return Ok(());
            }
        }

        // Set starting state
        {
            let mut state = self.state.write().await;
            *state = EngineState::Starting;
        }

        info!("Starting heartbeat engine...");

        // Initialize stats
        {
            let mut stats = self.stats.write().await;
            stats.started_at = Some(Utc::now());
        }

        // Register built-in actions
        self.register_builtin_actions().await?;

        // Start file watcher
        self.watcher.start()?;

        // Start the scheduler loop
        let scheduler = self.scheduler.clone();
        tokio::spawn(async move {
            scheduler.start().await;
        });

        // Set running state
        {
            let mut state = self.state.write().await;
            *state = EngineState::Running;
        }

        info!("Heartbeat engine started");

        // Spawn the main loop
        self.spawn_main_loop();

        Ok(())
    }

    /// Spawn the main heartbeat loop
    fn spawn_main_loop(&mut self) {
        let command_rx = self.command_rx.take().expect("Receiver already taken");
        let config = self.config.clone();
        let state = self.state.clone();
        let stats = self.stats.clone();
        let tools = self.tools.clone();
        let proactive = self.proactive.clone();
        let scheduler = self.scheduler.clone();
        let watcher = self.watcher.clone();
        let shutdown_tx = self.shutdown_tx.clone();
        let registered_actions = self.registered_actions.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(config.interval_secs));
            let mut command_rx = command_rx;
            let mut shutdown_rx = shutdown_tx.subscribe();
            let mut tick_count: u64 = 0;

            info!("Heartbeat main loop started");

            loop {
                tokio::select! {
                    // Shutdown signal
                    _ = shutdown_rx.recv() => {
                        info!("Heartbeat received shutdown signal");
                        break;
                    }

                    // Command received
                    Some((cmd, response_tx)) = command_rx.recv() => {
                        let response = Self::handle_command(
                            cmd,
                            &tools,
                            &proactive,
                            &scheduler,
                            &watcher,
                            &state,
                            &stats,
                            &registered_actions,
                        ).await;

                        if let Some(tx) = response_tx {
                            let _ = tx.send(response).await;
                        }
                    }

                    // Heartbeat tick
                    _ = interval.tick() => {
                        let current_state = *state.read().await;

                        if current_state == EngineState::Running {
                            tick_count += 1;

                            // Update stats
                            {
                                let mut stats = stats.write().await;
                                stats.ticks = tick_count;
                                stats.last_tick = Some(Utc::now());
                            }

                            if config.verbose {
                                debug!("Heartbeat tick {}", tick_count);
                            }

                            // Periodic health check
                            if tick_count % config.health_check_interval == 0 {
                                if let Err(e) = Self::perform_health_check(
                                    &tools,
                                    &state,
                                    &stats,
                                ).await {
                                    error!("Health check failed: {}", e);
                                }
                            }

                            // Check and execute triggered proactive actions
                            if let Err(e) = Self::execute_triggered_actions(
                                &proactive,
                                &stats,
                            ).await {
                                error!("Failed to execute triggered actions: {}", e);
                            }

                            // Execute due scheduled tasks
                            if let Err(e) = Self::execute_due_tasks(
                                &scheduler,
                                &stats,
                            ).await {
                                error!("Failed to execute due tasks: {}", e);
                            }

                            // Process file watcher events
                            if let Err(e) = Self::process_watcher_events(
                                &watcher,
                                &proactive,
                                &stats,
                            ).await {
                                error!("Failed to process watcher events: {}", e);
                            }
                        }
                    }
                }
            }

            info!("Heartbeat main loop exited");
        });
    }

    /// Handle a command
    async fn handle_command(
        cmd: EngineCommand,
        tools: &ToolContext,
        proactive: &ProactiveEngine,
        scheduler: &TaskScheduler,
        watcher: &FileWatcher,
        state: &Arc<RwLock<EngineState>>,
        stats: &Arc<RwLock<HeartbeatStats>>,
        registered_actions: &Arc<Mutex<HashMap<String, ActionEntry>>>,
    ) -> EngineResponse {
        match cmd {
            EngineCommand::Pause => {
                let mut s = state.write().await;
                if *s == EngineState::Running {
                    *s = EngineState::Paused;
                    info!("Heartbeat engine paused");
                }
                EngineResponse::Ack
            }

            EngineCommand::Resume => {
                let mut s = state.write().await;
                if *s == EngineState::Paused {
                    *s = EngineState::Running;
                    info!("Heartbeat engine resumed");
                }
                EngineResponse::Ack
            }

            EngineCommand::ExecuteNow(action_name) => {
                match Self::execute_action_by_name(
                    &action_name,
                    tools,
                    registered_actions,
                ).await {
                    Ok(result) => EngineResponse::Ack,
                    Err(e) => EngineResponse::Error(e.to_string()),
                }
            }

            EngineCommand::RegisterAction(action) => {
                let id = proactive.register(action);
                EngineResponse::ActionRegistered(id)
            }

            EngineCommand::ScheduleTask(task) => {
                match scheduler.add_task(task).await {
                    Ok(id) => EngineResponse::TaskScheduled(id),
                    Err(e) => EngineResponse::Error(e.to_string()),
                }
            }

            EngineCommand::AddWatch(config) => {
                let id = Uuid::new_v4().to_string();
                // Store watch info and set up callback
                EngineResponse::WatchAdded(id)
            }

            EngineCommand::GetStats => {
                let s = stats.read().await.clone();
                EngineResponse::Stats(s)
            }

            EngineCommand::Shutdown => {
                let mut s = state.write().await;
                *s = EngineState::Stopping;
                info!("Heartbeat engine shutdown requested");
                EngineResponse::Ack
            }
        }
    }

    /// Perform a health check
    async fn perform_health_check(
        tools: &ToolContext,
        state: &Arc<RwLock<EngineState>>,
        stats: &Arc<RwLock<HeartbeatStats>>,
    ) -> Result<()> {
        let health = tools.health_check().await;

        // Update stats
        {
            let mut s = stats.write().await;
            s.health_checks += 1;
            s.current_health = Some(health.clone());
        }

        // Update state based on health
        match &health.overall {
            ServiceHealth::Healthy => {
                let mut s = state.write().await;
                if *s == EngineState::Degraded {
                    *s = EngineState::Running;
                    info!("Engine recovered from degraded state");
                }
            }
            ServiceHealth::Degraded(reason) => {
                let mut s = state.write().await;
                if *s == EngineState::Running {
                    *s = EngineState::Degraded;
                    warn!("Engine degraded: {}", reason);
                }
            }
            ServiceHealth::Unhealthy(reason) => {
                warn!("Engine unhealthy: {}", reason);
                let mut s = stats.write().await;
                s.consecutive_failures += 1;
            }
        }

        if health.overall.is_healthy() {
            debug!("Health check passed");
        } else {
            warn!("Health check issues: filesystem={:?}, shell={:?}, web={:?}",
                health.filesystem, health.shell, health.web);
        }

        Ok(())
    }

    /// Execute triggered proactive actions
    async fn execute_triggered_actions(
        proactive: &ProactiveEngine,
        stats: &Arc<RwLock<HeartbeatStats>>,
    ) -> Result<()> {
        let triggered = proactive.get_triggered_actions();

        for action_id in triggered {
            debug!("Executing triggered action: {}", action_id);

            match proactive.execute(&action_id).await {
                Ok(result) => {
                    let mut s = stats.write().await;
                    s.actions_executed += 1;
                    if result.success {
                        s.actions_successful += 1;
                    }

                    if result.success {
                        info!("Action {} succeeded: {}", action_id, result.message);
                    } else {
                        warn!("Action {} failed: {}", action_id, result.message);
                    }
                }
                Err(e) => {
                    error!("Failed to execute action {}: {}", action_id, e);
                }
            }
        }

        Ok(())
    }

    /// Execute due scheduled tasks
    async fn execute_due_tasks(
        scheduler: &TaskScheduler,
        stats: &Arc<RwLock<HeartbeatStats>>,
    ) -> Result<()> {
        let due = scheduler.get_due_tasks().await;

        for task_id in due {
            debug!("Executing scheduled task: {}", task_id);

            match scheduler.execute_now(&task_id).await {
                Ok(result) => {
                    let mut s = stats.write().await;
                    s.tasks_executed += 1;
                    if result.success {
                        s.tasks_successful += 1;
                    }

                    if result.success {
                        info!("Task {} completed: {}", task_id, result.message);
                    } else {
                        warn!("Task {} failed: {}", task_id, result.message);
                    }
                }
                Err(e) => {
                    error!("Failed to execute task {}: {}", task_id, e);
                }
            }
        }

        Ok(())
    }

    /// Process file watcher events
    async fn process_watcher_events(
        watcher: &FileWatcher,
        proactive: &ProactiveEngine,
        stats: &Arc<RwLock<HeartbeatStats>>,
    ) -> Result<()> {
        // Get and process events from the watcher
        // This is a simplified version - in production you'd have a channel
        // for events and process them here

        // Trigger file change actions
        let _results = proactive.trigger_by_custom("file_changed").await;

        if !_results.is_empty() {
            let mut s = stats.write().await;
            s.events_processed += _results.len() as u64;
        }

        Ok(())
    }

    /// Execute an action by name
    async fn execute_action_by_name(
        name: &str,
        tools: &ToolContext,
        registered_actions: &Arc<Mutex<HashMap<String, ActionEntry>>>,
    ) -> Result<String> {
        match name {
            "health_check" => Self::action_health_check(tools).await,
            "cleanup_temp" => Self::action_cleanup_temp(tools).await,
            "sync_state" => Self::action_sync_state(tools).await,
            "check_updates" => Self::action_check_updates(tools).await,
            _ => {
                // Check registered actions
                let actions = registered_actions.lock().await;
                if let Some(entry) = actions.get(name) {
                    (entry.executor)(tools).await
                } else {
                    Err(anyhow::anyhow!("Unknown action: {}", name))
                }
            }
        }
    }

    /// Register built-in actions
    async fn register_builtin_actions(&self) -> Result<()> {
        let actions = vec![
            ProactiveAction::new("health_check", Trigger::Interval(300))
                .with_description("Periodic health check")
                .with_priority(Priority::High)
                .with_cooldown(60)
                .with_tag("system"),

            ProactiveAction::new("cleanup_temp", Trigger::Interval(3600))
                .with_description("Clean up temporary files")
                .with_priority(Priority::Low)
                .with_cooldown(1800)
                .with_tag("maintenance"),

            ProactiveAction::new("sync_state", Trigger::Interval(60))
                .with_description("Sync agent state")
                .with_priority(Priority::Normal)
                .with_cooldown(30)
                .with_tag("sync"),

            ProactiveAction::new("check_updates", Trigger::Interval(86400))
                .with_description("Check for updates")
                .with_priority(Priority::Low)
                .with_cooldown(43200)
                .with_tag("update"),
        ];

        for action in actions {
            let tools = self.tools.clone();
            let name = action.name.clone();

            self.proactive.register_with_executor(action, move |_action| {
                let tools = tools.clone();
                let name = name.clone();
                async move {
                    match name.as_str() {
                        "health_check" => Self::action_health_check(&tools).await,
                        "cleanup_temp" => Self::action_cleanup_temp(&tools).await,
                        "sync_state" => Self::action_sync_state(&tools).await,
                        "check_updates" => Self::action_check_updates(&tools).await,
                        _ => Ok(format!("Action {} executed", name)),
                    }
                }
            });
        }

        info!("Registered {} built-in actions", 4);
        Ok(())
    }

    /// Health check action
    async fn action_health_check(tools: &ToolContext) -> Result<String> {
        let health = tools.health_check().await;

        let status = match &health.overall {
            ServiceHealth::Healthy => "healthy",
            ServiceHealth::Degraded(_) => "degraded",
            ServiceHealth::Unhealthy(_) => "unhealthy",
        };

        Ok(format!(
            "Health check: {} (filesystem={:?}, shell={:?}, web={:?})",
            status, health.filesystem, health.shell, health.web
        ))
    }

    /// Cleanup temp files action
    async fn action_cleanup_temp(tools: &ToolContext) -> Result<String> {
        let mut cleaned = 0;

        // Get temp directory
        let temp_dir = std::env::temp_dir();

        if tools.filesystem.sandbox().is_allowed(&temp_dir) {
            // Use shell to find and clean old temp files
            let cmd = format!("find {} -type f -atime +7 2>/dev/null | head -100", temp_dir.display());
            match tools.shell.execute(&cmd).await {
                Ok(result) if result.exit_code == Some(0) => {
                    let files: Vec<&str> = result.stdout.lines().collect();
                    cleaned = files.len();
                }
                _ => {}
            }
        }

        Ok(format!("Temp cleanup: {} old files found", cleaned))
    }

    /// Sync state action
    async fn action_sync_state(tools: &ToolContext) -> Result<String> {
        // Ensure state directory exists
        let state_dir = dirs::data_dir()
            .map(|d| d.join("my-agent/state"))
            .unwrap_or_else(|| std::path::PathBuf::from("./state"));

        if !state_dir.exists() {
            tokio::fs::create_dir_all(&state_dir).await?;
        }

        // Write sync timestamp
        let sync_file = state_dir.join("last_sync");
        let timestamp = Utc::now().to_rfc3339();
        tokio::fs::write(&sync_file, timestamp).await?;

        Ok(format!("State synced at {}", Utc::now().format("%Y-%m-%d %H:%M:%S UTC")))
    }

    /// Check updates action
    async fn action_check_updates(tools: &ToolContext) -> Result<String> {
        // Check if cargo is available
        match tools.shell.execute("which cargo").await {
            Ok(result) if result.exit_code == Some(0) => {
                // Try to check for outdated packages
                match tools.shell.execute("cargo outdated --root-deps-only 2>&1 || echo 'cargo-outdated not installed'").await {
                    Ok(output) => {
                        let stdout = output.stdout.trim();
                        if stdout.contains("cargo-outdated") {
                            Ok("Update check: cargo-outdated not installed".to_string())
                        } else if stdout.is_empty() {
                            Ok("Update check: All dependencies up to date".to_string())
                        } else {
                            Ok(format!("Update check: {}", stdout))
                        }
                    }
                    Err(e) => Ok(format!("Update check error: {}", e)),
                }
            }
            _ => Ok("Update check: cargo not found".to_string()),
        }
    }

    /// Stop the heartbeat engine
    pub async fn stop(&self) -> Result<()> {
        let state = self.state.read().await;
        if *state == EngineState::Stopped {
            return Ok(());
        }
        drop(state);

        info!("Stopping heartbeat engine...");

        // Send shutdown signal
        let _ = self.shutdown_tx.send(());

        // Stop scheduler
        self.scheduler.stop().await;

        // Stop file watcher
        self.watcher.stop();

        // Update state
        let mut state = self.state.write().await;
        *state = EngineState::Stopped;

        info!("Heartbeat engine stopped");
        Ok(())
    }

    /// Pause the engine
    pub async fn pause(&self) -> Result<()> {
        let mut state = self.state.write().await;
        if *state == EngineState::Running {
            *state = EngineState::Paused;
            info!("Heartbeat engine paused");
        }
        Ok(())
    }

    /// Resume the engine
    pub async fn resume(&self) -> Result<()> {
        let mut state = self.state.write().await;
        if *state == EngineState::Paused {
            *state = EngineState::Running;
            info!("Heartbeat engine resumed");
        }
        Ok(())
    }

    /// Register a custom action
    pub async fn register_action<F, Fut>(&self, action: ProactiveAction, executor: F) -> String
    where
        F: Fn(&ToolContext) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<String>> + Send + 'static,
    {
        let tools = self.tools.clone();
        let executor = Arc::new(executor);

        self.proactive.register_with_executor(action, move |_action| {
            let tools = tools.clone();
            let executor = executor.clone();
            async move {
                executor(&tools).await
            }
        })
    }

    /// Schedule a task
    pub async fn schedule_task<F, Fut>(&self, task: ScheduledTask, executor: F) -> Result<String>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<String>> + Send + 'static,
    {
        self.scheduler.add_task_with_executor(task, executor).await
    }

    /// Add a file watch with callback
    pub fn add_watch<F>(&self, config: WatchConfig, callback: F) -> Result<String>
    where
        F: Fn(&FileSystemEvent) + Send + Sync + 'static,
    {
        self.watcher.add_watch(config, Box::new(callback))
    }
}

impl Default for HeartbeatEngine {
    fn default() -> Self {
        Self::new().expect("Failed to create default HeartbeatEngine")
    }
}

/// Global heartbeat engine instance
static GLOBAL_HEARTBEAT: once_cell::sync::Lazy<Arc<tokio::sync::Mutex<Option<HeartbeatEngine>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(tokio::sync::Mutex::new(None)));

/// Start the global heartbeat engine
pub async fn start_global_heartbeat() -> Result<()> {
    let mut guard = GLOBAL_HEARTBEAT.lock().await;
    if guard.is_some() {
        warn!("Global heartbeat already started");
        return Ok(());
    }

    let mut engine = HeartbeatEngine::new()?;
    engine.start().await?;
    *guard = Some(engine);

    info!("Global heartbeat engine started");
    Ok(())
}

/// Stop the global heartbeat engine
pub async fn stop_global_heartbeat() -> Result<()> {
    let mut guard = GLOBAL_HEARTBEAT.lock().await;
    if let Some(engine) = guard.take() {
        engine.stop().await?;
    }
    Ok(())
}

/// Get global heartbeat stats
pub async fn get_global_stats() -> Option<HeartbeatStats> {
    let guard = GLOBAL_HEARTBEAT.lock().await;
    if let Some(engine) = guard.as_ref() {
        Some(engine.stats().await)
    } else {
        None
    }
}

/// Send command to global heartbeat
pub async fn send_global_command(cmd: EngineCommand) -> Result<()> {
    let guard = GLOBAL_HEARTBEAT.lock().await;
    if let Some(engine) = guard.as_ref() {
        let sender = engine.command_sender();
        sender.send((cmd, None)).await?;
        Ok(())
    } else {
        Err(anyhow::anyhow!("Global heartbeat not running"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tool_context_creation() {
        let tools = ToolContext::new();
        assert!(tools.is_ok());
    }

    #[tokio::test]
    #[ignore = "Requires interactive approval - run manually"]
    async fn test_health_check() {
        let tools = ToolContext::new().unwrap();
        let health = tools.health_check().await;

        // At least filesystem should be healthy
        assert!(health.filesystem.is_operational());
    }

    #[tokio::test]
    async fn test_heartbeat_engine_creation() {
        let engine = HeartbeatEngine::new();
        assert!(engine.is_ok());
    }

    #[tokio::test]
    async fn test_heartbeat_stats() {
        let engine = HeartbeatEngine::new().unwrap();
        let stats = engine.stats().await;
        assert_eq!(stats.ticks, 0);
        assert_eq!(stats.actions_executed, 0);
    }

    #[test]
    fn test_service_health() {
        assert!(ServiceHealth::Healthy.is_healthy());
        assert!(ServiceHealth::Healthy.is_operational());
        assert!(!ServiceHealth::Unhealthy("test".to_string()).is_operational());
        assert!(!ServiceHealth::Degraded("test".to_string()).is_healthy());
        assert!(ServiceHealth::Degraded("test".to_string()).is_operational());
    }
}
