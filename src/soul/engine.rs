//! Soul Engine - unified autonomous agent core
//!
//! Integrates heartbeat, proactive actions, scheduling, and file watching
//! to enable the agent to act autonomously without prompts.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{info, warn};

use super::proactive::{ProactiveAction, ProactiveEngine, ActionResult};
use super::scheduler::{TaskScheduler, ScheduledTask};
use super::watcher::{FileWatcher, WatchConfig, FileSystemEvent};
use crate::tools::{FileSystemTool, ShellTool, WebTool};
use std::path::Path;

/// Tools context for executing actions
#[derive(Clone)]
pub struct ToolContext {
    pub filesystem: FileSystemTool,
    pub shell: ShellTool,
    pub web: WebTool,
}

impl ToolContext {
    /// Create a new tool context with default configurations
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            filesystem: FileSystemTool::new(),
            shell: ShellTool::new(),
            web: WebTool::new()?,
        })
    }

    /// Create with custom tools
    pub fn with_tools(
        filesystem: FileSystemTool,
        shell: ShellTool,
        web: WebTool,
    ) -> Self {
        Self {
            filesystem,
            shell,
            web,
        }
    }
}

impl Default for ToolContext {
    fn default() -> Self {
        Self::new().expect("Failed to create ToolContext")
    }
}

/// Soul engine state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoulState {
    Stopped,
    Starting,
    Running,
    Paused,
    Stopping,
}

impl std::fmt::Display for SoulState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SoulState::Stopped => write!(f, "Stopped"),
            SoulState::Starting => write!(f, "Starting"),
            SoulState::Running => write!(f, "Running"),
            SoulState::Paused => write!(f, "Paused"),
            SoulState::Stopping => write!(f, "Stopping"),
        }
    }
}

/// Soul engine statistics
#[derive(Debug, Clone, Serialize)]
pub struct SoulStats {
    pub state: SoulState,
    pub uptime_secs: u64,
    pub proactive_actions_registered: usize,
    pub scheduled_tasks: usize,
    pub file_watches: usize,
    pub actions_executed: u64,
    pub actions_successful: u64,
    pub last_action: Option<DateTime<Utc>>,
}

/// Messages the soul can process
#[derive(Debug, Clone)]
pub enum SoulMessage {
    /// Trigger a custom action
    TriggerCustom(String),
    /// Trigger a system event
    TriggerSystemEvent(String),
    /// Register a proactive action
    RegisterAction(ProactiveAction),
    /// Unregister an action
    UnregisterAction(String),
    /// Pause the soul
    Pause,
    /// Resume the soul
    Resume,
    /// Shutdown
    Shutdown,
}

/// The soul engine - the autonomous core of the agent
pub struct SoulEngine {
    /// Proactive action engine
    proactive: Arc<ProactiveEngine>,
    /// Task scheduler
    scheduler: Arc<TaskScheduler>,
    /// File watcher
    watcher: Arc<FileWatcher>,
    /// Current state
    state: Arc<Mutex<SoulState>>,
    /// Message sender
    tx: mpsc::Sender<SoulMessage>,
    /// Message receiver (for the run loop)
    rx: Option<mpsc::Receiver<SoulMessage>>,
    /// Stats
    actions_executed: Arc<Mutex<u64>>,
    actions_successful: Arc<Mutex<u64>>,
    last_action: Arc<Mutex<Option<DateTime<Utc>>>>,
    /// Start time
    started_at: Arc<Mutex<Option<std::time::Instant>>>,
    /// Shutdown signal
    shutdown_tx: broadcast::Sender<()>,
    /// Tools context for executing actions
    tools: Option<Arc<ToolContext>>,
}

impl SoulEngine {
    /// Create a new soul engine
    pub fn new() -> Self {
        Self::with_tools(None)
    }

    /// Create a new soul engine with tools
    pub fn with_tools(tools: Option<ToolContext>) -> Self {
        let (tx, rx) = mpsc::channel(100);
        let (shutdown_tx, _) = broadcast::channel(1);

        Self {
            proactive: Arc::new(ProactiveEngine::new()),
            scheduler: Arc::new(TaskScheduler::new()),
            watcher: Arc::new(FileWatcher::new()),
            state: Arc::new(Mutex::new(SoulState::Stopped)),
            tx,
            rx: Some(rx),
            actions_executed: Arc::new(Mutex::new(0)),
            actions_successful: Arc::new(Mutex::new(0)),
            last_action: Arc::new(Mutex::new(None)),
            started_at: Arc::new(Mutex::new(None)),
            shutdown_tx,
            tools: tools.map(Arc::new),
        }
    }

    /// Set tools context after creation
    pub fn set_tools(&mut self, tools: ToolContext) {
        self.tools = Some(Arc::new(tools));
    }

    /// Get a sender to send messages to the soul
    pub fn sender(&self) -> mpsc::Sender<SoulMessage> {
        self.tx.clone()
    }

    /// Get current state
    pub async fn state(&self) -> SoulState {
        *self.state.lock().await
    }

    /// Get statistics
    pub async fn stats(&self) -> SoulStats {
        let state = *self.state.lock().await;
        let started_at = *self.started_at.lock().await;
        let uptime_secs = started_at
            .map(|s| s.elapsed().as_secs())
            .unwrap_or(0);

        let proactive_stats = self.proactive.get_all_stats();
        let scheduler_stats = self.scheduler.stats().await;
        let watcher_stats = self.watcher.stats();

        let actions_executed = *self.actions_executed.lock().await;
        let actions_successful = *self.actions_successful.lock().await;
        let last_action = *self.last_action.lock().await;

        SoulStats {
            state,
            uptime_secs,
            proactive_actions_registered: proactive_stats.len(),
            scheduled_tasks: scheduler_stats.total_tasks,
            file_watches: watcher_stats.total_watches,
            actions_executed,
            actions_successful,
            last_action,
        }
    }

    /// Register a proactive action
    pub fn register_action(&self, action: ProactiveAction) -> String {
        self.proactive.register(action)
    }

    /// Register a proactive action with an async executor
    pub fn register_action_with_executor<F, Fut>(&self, action: ProactiveAction, executor: F) -> String
    where
        F: Fn(&ProactiveAction) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<String>> + Send + 'static,
    {
        self.proactive.register_with_executor(action, executor)
    }

    /// Unregister a proactive action
    pub fn unregister_action(&self, id: &str) -> Result<()> {
        self.proactive.unregister(id)
    }

    /// Schedule a task
    pub async fn schedule_task(&self, task: ScheduledTask) -> Result<String> {
        self.scheduler.add_task(task).await
    }

    /// Schedule a task with an executor
    pub async fn schedule_task_with_executor<F, Fut>(&self, task: ScheduledTask, executor: F) -> Result<String>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<String>> + Send + 'static,
    {
        self.scheduler.add_task_with_executor(task, executor).await
    }

    /// Add a file watch
    pub fn add_watch(&self, config: WatchConfig, callback: Box<dyn Fn(&FileSystemEvent) + Send + Sync>) -> Result<String> {
        self.watcher.add_watch(config, callback)
    }

    /// Remove a file watch
    pub fn remove_watch(&self, id: &str) -> Result<()> {
        self.watcher.remove_watch(id)
    }

    /// Start the soul engine (spawns run loop in background)
    pub async fn start(&mut self) -> Result<()> {
        let mut state = self.state.lock().await;
        if *state != SoulState::Stopped {
            warn!("Soul engine is not stopped, cannot start");
            return Ok(());
        }

        *state = SoulState::Starting;
        drop(state);

        info!("Starting soul engine...");

        // Record start time
        *self.started_at.lock().await = Some(std::time::Instant::now());

        // Register built-in actions
        self.register_builtin_actions()?;

        // Start file watcher
        self.watcher.start()?;

        // Update state
        *self.state.lock().await = SoulState::Running;

        info!("Soul engine started");

        // Spawn the run loop in a background task
        self.spawn_run_loop();

        Ok(())
    }

    /// Spawn the run loop in a background task
    fn spawn_run_loop(&mut self) {
        let rx = self.rx.take().expect("Receiver already taken");
        let state = self.state.clone();
        let proactive = self.proactive.clone();
        let scheduler = self.scheduler.clone();
        let _tx = self.tx.clone();
        let shutdown_tx = self.shutdown_tx.clone();
        let actions_executed = self.actions_executed.clone();
        let actions_successful = self.actions_successful.clone();
        let last_action = self.last_action.clone();

        tokio::spawn(async move {
            let mut rx = rx;
            let mut shutdown_rx = shutdown_tx.subscribe();
            let mut heartbeat = tokio::time::interval(tokio::time::Duration::from_secs(10));

            loop {
                tokio::select! {
                    // Handle shutdown signal
                    _ = shutdown_rx.recv() => {
                        info!("Soul engine received shutdown signal");
                        break;
                    }

                    // Handle messages
                    Some(msg) = rx.recv() => {
                        handle_message_inner(
                            msg,
                            &proactive,
                            &state,
                        ).await;
                    }

                    // Heartbeat tick
                    _ = heartbeat.tick() => {
                        let current_state = *state.lock().await;
                        if current_state == SoulState::Running {
                            // Get triggered proactive actions
                            let triggered = proactive.get_triggered_actions();

                            for action_id in triggered {
                                match proactive.execute(&action_id).await {
                                    Ok(result) => {
                                        *actions_executed.lock().await += 1;
                                        if result.success {
                                            *actions_successful.lock().await += 1;
                                        }
                                        *last_action.lock().await = Some(Utc::now());

                                        info!(
                                            "Proactive action {} {} ({}ms): {}",
                                            action_id,
                                            if result.success { "succeeded" } else { "failed" },
                                            result.duration_ms,
                                            result.message
                                        );
                                    }
                                    Err(e) => {
                                        warn!("Proactive action {} failed: {}", action_id, e);
                                    }
                                }
                            }

                            // Execute due scheduled tasks
                            let due = scheduler.get_due_tasks().await;
                            for task_id in due {
                                match scheduler.execute_now(&task_id).await {
                                    Ok(result) => {
                                        *actions_executed.lock().await += 1;
                                        if result.success {
                                            *actions_successful.lock().await += 1;
                                        }
                                        *last_action.lock().await = Some(Utc::now());

                                        info!(
                                            "Scheduled task {} {} ({}ms): {}",
                                            result.task_id,
                                            if result.success { "succeeded" } else { "failed" },
                                            result.duration_ms,
                                            result.message
                                        );
                                    }
                                    Err(e) => {
                                        warn!("Scheduled task {} failed: {}", task_id, e);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            info!("Soul engine run loop exited");
        });
    }

    /// Stop the soul engine
    pub async fn stop(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        if *state != SoulState::Running && *state != SoulState::Paused {
            return Ok(());
        }

        *state = SoulState::Stopping;
        drop(state);

        info!("Stopping soul engine...");

        // Stop file watcher
        self.watcher.stop();

        // Stop scheduler
        self.scheduler.stop().await;

        // Send shutdown signal
        let _ = self.shutdown_tx.send(());

        // Update state
        *self.state.lock().await = SoulState::Stopped;

        info!("Soul engine stopped");

        Ok(())
    }

    /// Pause the soul engine
    pub async fn pause(&self) -> Result<()> {
        let state = self.state.lock().await;
        if *state == SoulState::Running {
            drop(state);
            *self.state.lock().await = SoulState::Paused;
            info!("Soul engine paused");
        }
        Ok(())
    }

    /// Resume the soul engine
    pub async fn resume(&self) -> Result<()> {
        let state = self.state.lock().await;
        if *state == SoulState::Paused {
            drop(state);
            *self.state.lock().await = SoulState::Running;
            info!("Soul engine resumed");
        }
        Ok(())
    }

    /// Trigger a custom action
    pub async fn trigger_custom(&self, condition: &str) -> Vec<ActionResult> {
        self.proactive.trigger_by_custom(condition).await
    }

    /// Trigger a system event
    pub async fn trigger_system(&self, event: &str) -> Vec<ActionResult> {
        self.proactive.trigger_by_event(event).await
    }

    /// Register built-in proactive actions
    fn register_builtin_actions(&self) -> Result<()> {
        use super::proactive::create_builtin_actions;

        let actions = create_builtin_actions();
        let tools = self.tools.clone();

        for action in actions {
            let action_name = action.name.clone();
            let tools_clone = tools.clone();

            self.proactive.register_with_executor(action, move |_action| {
                let tools = tools_clone.clone();
                let name = action_name.clone();
                async move {
                    // Built-in action handlers with tool integration
                    match name.as_str() {
                        "health_check" => {
                            Self::execute_health_check(tools.as_ref()).await
                        }
                        "cleanup_temp" => {
                            Self::execute_cleanup_temp(tools.as_ref()).await
                        }
                        "sync_state" => {
                            Self::execute_sync_state(tools.as_ref()).await
                        }
                        "check_updates" => {
                            Self::execute_check_updates(tools.as_ref()).await
                        }
                        "promote_learnings" => {
                            Self::execute_promote_learnings().await
                        }
                        _ => {
                            Ok(format!("Action {} executed", name))
                        }
                    }
                }
            });
        }

        Ok(())
    }

    /// Execute health check using system tools
    async fn execute_health_check(tools: Option<&Arc<ToolContext>>) -> Result<String> {
        info!("Running health check...");

        let mut results = Vec::new();

        if let Some(ctx) = tools {
            // Check disk space
            match ctx.shell.execute_unsafe("df -h /").await {
                Ok(output) => {
                    results.push(format!("Disk usage:\n{}", output.stdout));
                }
                Err(e) => {
                    warn!("Failed to check disk space: {}", e);
                    results.push("Disk check failed".to_string());
                }
            }

            // Check memory
            match ctx.shell.execute_unsafe("free -h").await {
                Ok(output) => {
                    results.push(format!("Memory usage:\n{}", output.stdout));
                }
                Err(e) => {
                    warn!("Failed to check memory: {}", e);
                }
            }

            // Check load average
            match ctx.shell.execute_unsafe("uptime").await {
                Ok(output) => {
                    results.push(format!("System load:\n{}", output.stdout));
                }
                Err(e) => {
                    warn!("Failed to check uptime: {}", e);
                }
            }
        } else {
            results.push("Health check (no tools available)".to_string());
        }

        Ok(format!("Health check completed\n{}", results.join("\n")))
    }

    /// Execute temp file cleanup
    async fn execute_cleanup_temp(tools: Option<&Arc<ToolContext>>) -> Result<String> {
        info!("Cleaning up temp files...");

        if let Some(ctx) = tools {
            // Find and clean old temp files (older than 7 days)
            let temp_dirs = ["/tmp", "/var/tmp"];
            let mut cleaned = 0;
            let mut errors = 0;

            for temp_dir in &temp_dirs {
                // Check if directory exists and is accessible
                if ctx.filesystem.sandbox().is_allowed(Path::new(temp_dir)) {
                    match ctx.shell.execute_unsafe(&format!(
                        "find {} -type f -atime +7 -delete 2>/dev/null || true",
                        temp_dir
                    )).await {
                        Ok(_) => {
                            // Count files that would be deleted (for dry run)
                            match ctx.shell.execute_unsafe(&format!(
                                "find {} -type f -atime +7 2>/dev/null | wc -l",
                                temp_dir
                            )).await {
                                Ok(output) => {
                                    let count: usize = output.stdout.trim().parse().unwrap_or(0);
                                    cleaned += count;
                                }
                                Err(_) => {}
                            }
                        }
                        Err(e) => {
                            warn!("Failed to clean {}: {}", temp_dir, e);
                            errors += 1;
                        }
                    }
                }
            }

            // Clean user cache if accessible
            let cache_dir = dirs::cache_dir();
            if let Some(ref cache) = cache_dir {
                let cache_str = cache.to_string_lossy();
                if ctx.filesystem.sandbox().is_allowed(Path::new(&*cache_str)) {
                    match ctx.shell.execute_unsafe(&format!(
                        "find {} -type f -atime +30 -delete 2>/dev/null || true",
                        cache_str
                    )).await {
                        Ok(_) => {}
                        Err(e) => {
                            warn!("Failed to clean cache: {}", e);
                        }
                    }
                }
            }

            Ok(format!(
                "Temp cleanup completed: {} old files removed, {} errors",
                cleaned, errors
            ))
        } else {
            Ok("Temp cleanup skipped (no tools available)".to_string())
        }
    }

    /// Execute state synchronization
    async fn execute_sync_state(tools: Option<&Arc<ToolContext>>) -> Result<String> {
        info!("Syncing state...");

        if let Some(_ctx) = tools {
            // Ensure state directory exists
            let state_dir = dirs::data_dir()
                .map(|d| d.join("my-agent/state"))
                .unwrap_or_else(|| std::path::PathBuf::from("./state"));

            // Create state directory if needed
            if !state_dir.exists() {
                match tokio::fs::create_dir_all(&state_dir).await {
                    Ok(_) => {}
                    Err(e) => {
                        warn!("Failed to create state directory: {}", e);
                    }
                }
            }

            // Write sync timestamp
            let sync_file = state_dir.join("last_sync");
            let timestamp = Utc::now().to_rfc3339();

            match tokio::fs::write(&sync_file, timestamp).await {
                Ok(_) => {}
                Err(e) => {
                    warn!("Failed to write sync timestamp: {}", e);
                }
            }

            Ok(format!("State synced at {}", Utc::now().format("%Y-%m-%d %H:%M:%S UTC")))
        } else {
            Ok("State synced (no tools available)".to_string())
        }
    }

    /// Execute learning promotion cycle
    async fn execute_promote_learnings() -> Result<String> {
        info!("Running learning promotion cycle...");

        let store = match crate::learning::LearningStore::new() {
            Ok(s) => Arc::new(s),
            Err(e) => return Ok(format!("Promotion skipped: {}", e)),
        };

        let bootstrap = match crate::learning::BootstrapContext::new() {
            Ok(b) => Arc::new(b),
            Err(e) => return Ok(format!("Promotion skipped: {}", e)),
        };

        let engine = crate::learning::PromotionEngine::new(store, bootstrap);
        match engine.run_promotion_cycle() {
            Ok(count) => Ok(format!("Promotion cycle complete: {} entries promoted", count)),
            Err(e) => Ok(format!("Promotion cycle failed: {}", e)),
        }
    }

    /// Execute update check
    async fn execute_check_updates(tools: Option<&Arc<ToolContext>>) -> Result<String> {
        info!("Checking for updates...");

        if let Some(ctx) = tools {
            // Check if cargo is available and check for updates
            match ctx.shell.execute_unsafe("which cargo").await {
                Ok(_) => {
                    // Check for outdated packages in the project
                    match ctx.shell.execute_unsafe("cargo outdated --root-deps-only 2>/dev/null || echo 'cargo-outdated not installed'").await {
                        Ok(output) => {
                            let stdout = output.stdout.trim();
                            if stdout.contains("cargo-outdated") {
                                Ok("No updates available (install cargo-outdated for detailed checks)".to_string())
                            } else if stdout.is_empty() || stdout == "All dependencies are up to date" {
                                Ok("All dependencies are up to date".to_string())
                            } else {
                                Ok(format!("Update check complete:\n{}", stdout))
                            }
                        }
                        Err(e) => {
                            Ok(format!("Update check completed with warnings: {}", e))
                        }
                    }
                }
                Err(_) => {
                    Ok("No updates available (cargo not found)".to_string())
                }
            }
        } else {
            Ok("No updates available (no tools)".to_string())
        }
    }
}

/// Handle a soul message (inner function for spawned task)
async fn handle_message_inner(
    msg: SoulMessage,
    proactive: &ProactiveEngine,
    state: &Arc<Mutex<SoulState>>,
) {
    match msg {
        SoulMessage::TriggerCustom(condition) => {
            let results = proactive.trigger_by_custom(&condition).await;
            info!("Custom trigger '{}' executed: {} results", condition, results.len());
        }
        SoulMessage::TriggerSystemEvent(event) => {
            let results = proactive.trigger_by_event(&event).await;
            info!("System event '{}' triggered: {} results", event, results.len());
        }
        SoulMessage::RegisterAction(action) => {
            let id = proactive.register(action);
            info!("Action registered: {}", id);
        }
        SoulMessage::UnregisterAction(id) => {
            if let Err(e) = proactive.unregister(&id) {
                warn!("Failed to unregister action {}: {}", id, e);
            }
        }
        SoulMessage::Pause => {
            let current = *state.lock().await;
            if current == SoulState::Running {
                *state.lock().await = SoulState::Paused;
                info!("Soul engine paused");
            }
        }
        SoulMessage::Resume => {
            let current = *state.lock().await;
            if current == SoulState::Paused {
                *state.lock().await = SoulState::Running;
                info!("Soul engine resumed");
            }
        }
        SoulMessage::Shutdown => {
            *state.lock().await = SoulState::Stopping;
            info!("Soul engine shutdown requested");
        }
    }
}

impl Default for SoulEngine {
    fn default() -> Self {
        Self::with_tools(None)
    }
}

/// Global soul engine instance
static SOUL_ENGINE: once_cell::sync::Lazy<Arc<tokio::sync::Mutex<Option<SoulEngine>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(tokio::sync::Mutex::new(None)));

/// Get the global soul engine
pub async fn get_soul() -> Arc<tokio::sync::Mutex<Option<SoulEngine>>> {
    SOUL_ENGINE.clone()
}

/// Start the global soul engine
pub async fn start_soul() -> Result<()> {
    let mut engine_guard = SOUL_ENGINE.lock().await;
    if engine_guard.is_some() {
        warn!("Soul engine already started");
        return Ok(());
    }

    let mut engine = SoulEngine::new();
    engine.start().await?;
    *engine_guard = Some(engine);

    Ok(())
}

/// Stop the global soul engine
pub async fn stop_soul() -> Result<()> {
    let mut engine_guard = SOUL_ENGINE.lock().await;
    if let Some(engine) = engine_guard.take() {
        engine.stop().await?;
    }

    Ok(())
}

/// Get soul stats
pub async fn get_soul_stats() -> Option<SoulStats> {
    let engine_guard = SOUL_ENGINE.lock().await;
    if let Some(engine) = engine_guard.as_ref() {
        Some(engine.stats().await)
    } else {
        None
    }
}

/// Send a message to the soul
pub async fn send_soul_message(msg: SoulMessage) -> Result<()> {
    let engine_guard = SOUL_ENGINE.lock().await;
    if let Some(engine) = engine_guard.as_ref() {
        engine.tx.send(msg).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_soul_engine_create() {
        let engine = SoulEngine::new();
        let state = engine.state().await;
        assert_eq!(state, SoulState::Stopped);
    }

    #[tokio::test]
    async fn test_soul_stats() {
        let engine = SoulEngine::new();
        let stats = engine.stats().await;
        assert_eq!(stats.state, SoulState::Stopped);
        assert_eq!(stats.uptime_secs, 0);
    }

    #[test]
    fn test_soul_state_display() {
        assert_eq!(format!("{}", SoulState::Running), "Running");
        assert_eq!(format!("{}", SoulState::Stopped), "Stopped");
    }
}
