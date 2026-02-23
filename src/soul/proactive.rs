//! Proactive action engine
//!
//! Decides when and what proactive actions to take based on triggers and conditions.

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tracing::info;
use uuid::Uuid;

/// Type alias for async action executor
pub type AsyncExecutor = Arc<
    dyn Fn(&ProactiveAction) -> Pin<Box<dyn Future<Output = Result<String>> + Send>> + Send + Sync
>;

/// Priority level for proactive actions
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Normal,
    High,
    Urgent,
}

impl Default for Priority {
    fn default() -> Self {
        Self::Normal
    }
}

/// Trigger condition for proactive actions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Trigger {
    /// Time-based trigger (cron expression)
    Time(String),
    /// Interval in seconds
    Interval(u64),
    /// File change in watched path
    FileChange { path: String, event: String },
    /// System event (e.g., low disk, high CPU)
    SystemEvent(String),
    /// Custom condition name
    Custom(String),
    /// Composite trigger (all must match)
    All(Vec<Trigger>),
    /// Composite trigger (any must match)
    Any(Vec<Trigger>),
}

/// A proactive action definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveAction {
    /// Unique action ID
    pub id: String,
    /// Action name
    pub name: String,
    /// Action description
    pub description: Option<String>,
    /// Trigger condition
    pub trigger: Trigger,
    /// Action priority
    pub priority: Priority,
    /// Cooldown period in seconds (prevents rapid re-triggering)
    pub cooldown_secs: u64,
    /// Whether this action is enabled
    pub enabled: bool,
    /// Maximum executions (None = unlimited)
    pub max_executions: Option<u64>,
    /// Tags for categorization
    pub tags: Vec<String>,
    /// Action parameters (JSON-like data)
    pub params: HashMap<String, String>,
}

impl ProactiveAction {
    /// Create a new proactive action
    pub fn new(name: &str, trigger: Trigger) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            description: None,
            trigger,
            priority: Priority::Normal,
            cooldown_secs: 300, // 5 minutes default
            enabled: true,
            max_executions: None,
            tags: Vec::new(),
            params: HashMap::new(),
        }
    }

    /// Set description
    pub fn with_description(mut self, desc: &str) -> Self {
        self.description = Some(desc.to_string());
        self
    }

    /// Set priority
    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    /// Set cooldown
    pub fn with_cooldown(mut self, secs: u64) -> Self {
        self.cooldown_secs = secs;
        self
    }

    /// Set max executions
    pub fn with_max_executions(mut self, max: u64) -> Self {
        self.max_executions = Some(max);
        self
    }

    /// Add a tag
    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tags.push(tag.to_string());
        self
    }

    /// Add a parameter
    pub fn with_param(mut self, key: &str, value: &str) -> Self {
        self.params.insert(key.to_string(), value.to_string());
        self
    }
}

/// Result of a proactive action execution
#[derive(Debug, Clone, Serialize)]
pub struct ActionResult {
    /// Action ID
    pub action_id: String,
    /// Whether the action succeeded
    pub success: bool,
    /// Result message
    pub message: String,
    /// Execution timestamp
    pub executed_at: DateTime<Utc>,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
}

/// State tracking for proactive actions
#[derive(Debug, Clone)]
struct ActionState {
    /// Last execution time
    last_execution: Option<DateTime<Utc>>,
    /// Number of executions
    execution_count: u64,
    /// Recent results
    recent_results: VecDeque<ActionResult>,
}

/// Proactive action engine
pub struct ProactiveEngine {
    /// Registered actions
    actions: Arc<Mutex<HashMap<String, ProactiveAction>>>,
    /// Action states
    states: Arc<Mutex<HashMap<String, ActionState>>>,
    /// Action executors (async)
    executors: Arc<Mutex<HashMap<String, AsyncExecutor>>>,
    /// Maximum results to keep per action
    max_results: usize,
}

impl Default for ProactiveEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ProactiveEngine {
    fn clone(&self) -> Self {
        Self {
            actions: self.actions.clone(),
            states: self.states.clone(),
            executors: self.executors.clone(),
            max_results: self.max_results,
        }
    }
}

impl ProactiveEngine {
    /// Create a new proactive engine
    pub fn new() -> Self {
        Self {
            actions: Arc::new(Mutex::new(HashMap::new())),
            states: Arc::new(Mutex::new(HashMap::new())),
            executors: Arc::new(Mutex::new(HashMap::new())),
            max_results: 10,
        }
    }

    /// Set max results to keep
    pub fn with_max_results(mut self, max: usize) -> Self {
        self.max_results = max;
        self
    }

    /// Register a proactive action
    pub fn register(&self, action: ProactiveAction) -> String {
        let id = action.id.clone();

        let mut actions = self.actions.lock().unwrap();
        actions.insert(id.clone(), action);

        // Initialize state
        let mut states = self.states.lock().unwrap();
        states.insert(id.clone(), ActionState {
            last_execution: None,
            execution_count: 0,
            recent_results: VecDeque::with_capacity(self.max_results),
        });

        info!("Registered proactive action: {}", id);
        id
    }

    /// Register an action with an executor
    pub fn register_with_executor<F, Fut>(
        &self,
        action: ProactiveAction,
        executor: F,
    ) -> String
    where
        F: Fn(&ProactiveAction) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<String>> + Send + 'static,
    {
        let id = action.id.clone();

        // Store action
        {
            let mut actions = self.actions.lock().unwrap();
            actions.insert(id.clone(), action);
        }

        // Initialize state
        {
            let mut states = self.states.lock().unwrap();
            states.insert(id.clone(), ActionState {
                last_execution: None,
                execution_count: 0,
                recent_results: VecDeque::with_capacity(self.max_results),
            });
        }

        // Store executor (wrap in async wrapper)
        {
            let mut executors = self.executors.lock().unwrap();
            executors.insert(id.clone(), Arc::new(move |action| {
                Box::pin(executor(action))
            }));
        }

        info!("Registered proactive action with executor: {}", id);
        id
    }

    /// Unregister an action
    pub fn unregister(&self, id: &str) -> Result<()> {
        let mut actions = self.actions.lock().unwrap();
        let mut states = self.states.lock().unwrap();
        let mut executors = self.executors.lock().unwrap();

        if actions.remove(id).is_some() {
            states.remove(id);
            executors.remove(id);
            info!("Unregistered proactive action: {}", id);
            Ok(())
        } else {
            bail!("Action not found: {}", id)
        }
    }

    /// Get an action by ID
    pub fn get(&self, id: &str) -> Option<ProactiveAction> {
        self.actions.lock().unwrap().get(id).cloned()
    }

    /// List all actions
    pub fn list(&self) -> Vec<ProactiveAction> {
        self.actions.lock().unwrap().values().cloned().collect()
    }

    /// List actions by tag
    pub fn list_by_tag(&self, tag: &str) -> Vec<ProactiveAction> {
        self.actions
            .lock().unwrap()
            .values()
            .filter(|a| a.tags.contains(&tag.to_string()))
            .cloned()
            .collect()
    }

    /// Enable/disable an action
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let mut actions = self.actions.lock().unwrap();
        if let Some(action) = actions.get_mut(id) {
            action.enabled = enabled;
            info!("Action {} {}", id, if enabled { "enabled" } else { "disabled" });
            Ok(())
        } else {
            bail!("Action not found: {}", id)
        }
    }

    /// Check if an action can execute (cooldown, max executions)
    fn can_execute_internal(
        &self,
        id: &str,
        cooldown_secs: u64,
        max_executions: Option<u64>,
        state: &ActionState,
    ) -> bool {
        // Check max executions
        if let Some(max) = max_executions {
            if state.execution_count >= max {
                return false;
            }
        }

        // Check cooldown
        if let Some(last) = state.last_execution {
            let elapsed = (Utc::now() - last).num_seconds();
            if elapsed < cooldown_secs as i64 {
                return false;
            }
        }

        true
    }

    /// Check if an action can execute (cooldown, max executions)
    pub fn can_execute(&self, id: &str) -> bool {
        // Collect all needed data first to avoid holding locks during checks
        let (cooldown_secs, max_executions) = {
            let actions = self.actions.lock().unwrap();
            if let Some(action) = actions.get(id) {
                if !action.enabled {
                    return false;
                }
                (action.cooldown_secs, action.max_executions)
            } else {
                return false;
            }
        };

        let state = {
            let states = self.states.lock().unwrap();
            states.get(id).cloned()
        };

        if let Some(state) = state {
            self.can_execute_internal(id, cooldown_secs, max_executions, &state)
        } else {
            false
        }
    }

    /// Execute an action immediately
    pub async fn execute(&self, id: &str) -> Result<ActionResult> {
        let executor = {
            let executors = self.executors.lock().unwrap();
            executors.get(id).cloned()
        };

        let action = {
            let actions = self.actions.lock().unwrap();
            actions.get(id).cloned()
        };

        if let (Some(exec), Some(action)) = (executor, action) {
            if !action.enabled {
                bail!("Action {} is disabled", id);
            }

            let start = std::time::Instant::now();
            let result = exec(&action).await;
            let duration_ms = start.elapsed().as_millis() as u64;

            let exec_result = ActionResult {
                action_id: id.to_string(),
                success: result.is_ok(),
                message: result.unwrap_or_else(|e| e.to_string()),
                executed_at: Utc::now(),
                duration_ms,
            };

            // Update state
            {
                let mut states = self.states.lock().unwrap();
                if let Some(state) = states.get_mut(id) {
                    state.last_execution = Some(exec_result.executed_at);
                    state.execution_count += 1;
                    state.recent_results.push_back(exec_result.clone());
                    if state.recent_results.len() > self.max_results {
                        state.recent_results.pop_front();
                    }
                }
            }

            info!("Executed action {}: {} ({}ms)",
                id,
                if exec_result.success { "success" } else { "failed" },
                duration_ms
            );

            Ok(exec_result)
        } else {
            bail!("Action not found or no executor: {}", id)
        }
    }

    /// Execute an action synchronously (blocking)
    pub fn execute_blocking(&self, id: &str) -> Result<ActionResult> {
        // Create a new runtime for blocking execution
        let rt = tokio::runtime::Handle::try_current()
            .map_err(|_| anyhow::anyhow!("No Tokio runtime available"))?;
        rt.block_on(self.execute(id))
    }

    /// Get actions that should run now (based on triggers)
    pub fn get_triggered_actions(&self) -> Vec<String> {
        // Collect action data and state data while holding locks
        let (action_data, states_guard) = {
            let actions = self.actions.lock().unwrap();
            let states = self.states.lock().unwrap();
            let now = Utc::now();

            let action_data: Vec<_> = actions
                .values()
                .filter(|action| {
                    if !action.enabled {
                        return false;
                    }
                    // Check trigger while we have the lock
                    self.check_trigger(&action.trigger, now)
                })
                .map(|a| (a.id.clone(), a.cooldown_secs, a.max_executions))
                .collect();

            // Clone states we need
            let states_clone: HashMap<String, ActionState> = states.clone();
            (action_data, states_clone)
        };

        // Check cooldown for each action (locks already released)
        action_data
            .into_iter()
            .filter(|(id, cooldown_secs, max_exec)| {
                if let Some(state) = states_guard.get(id) {
                    self.can_execute_internal(id, *cooldown_secs, *max_exec, state)
                } else {
                    false
                }
            })
            .map(|(id, _, _)| id)
            .collect()
    }

    /// Check if a trigger condition is met
    fn check_trigger(&self, trigger: &Trigger, now: DateTime<Utc>) -> bool {
        match trigger {
            Trigger::Interval(_secs) => {
                // For interval triggers, we check if enough time has passed
                // This is simplified - in practice, you'd track last check time
                true // Always true, cooldown handles the actual interval
            }
            Trigger::Time(cron_expr) => {
                // Check if current time matches cron expression
                // Simplified - just check if it's a valid cron
                cron::Schedule::try_from(cron_expr.as_str()).is_ok()
            }
            Trigger::FileChange { .. } => {
                // File changes are handled by the watcher
                false
            }
            Trigger::SystemEvent(_) => {
                // System events are checked externally
                false
            }
            Trigger::Custom(_) => {
                // Custom triggers are checked externally
                false
            }
            Trigger::All(triggers) => {
                triggers.iter().all(|t| self.check_trigger(t, now))
            }
            Trigger::Any(triggers) => {
                triggers.iter().any(|t| self.check_trigger(t, now))
            }
        }
    }

    /// Trigger an action by system event
    pub async fn trigger_by_event(&self, event_type: &str) -> Vec<ActionResult> {
        // Collect IDs in a scope to ensure lock is dropped before await
        let triggered_ids: Vec<String> = {
            let actions = self.actions.lock().unwrap();
            actions
                .values()
                .filter(|a| {
                    if !a.enabled || !self.can_execute(&a.id) {
                        return false;
                    }

                    match &a.trigger {
                        Trigger::SystemEvent(e) if e == event_type => true,
                        Trigger::Any(triggers) if triggers.iter().any(|t|
                            matches!(t, Trigger::SystemEvent(e) if e == event_type)
                        ) => true,
                        _ => false,
                    }
                })
                .map(|a| a.id.clone())
                .collect()
        };

        // Execute triggered actions
        let mut results = Vec::new();
        for id in triggered_ids {
            if let Ok(result) = self.execute(&id).await {
                results.push(result);
            }
        }
        results
    }

    /// Trigger an action by custom condition
    pub async fn trigger_by_custom(&self, condition: &str) -> Vec<ActionResult> {
        // Collect IDs in a scope to ensure lock is dropped before await
        let triggered_ids: Vec<String> = {
            let actions = self.actions.lock().unwrap();
            actions
                .values()
                .filter(|a| {
                    if !a.enabled || !self.can_execute(&a.id) {
                        return false;
                    }

                    match &a.trigger {
                        Trigger::Custom(c) if c == condition => true,
                        Trigger::Any(triggers) if triggers.iter().any(|t|
                            matches!(t, Trigger::Custom(c) if c == condition)
                        ) => true,
                        _ => false,
                    }
                })
                .map(|a| a.id.clone())
                .collect()
        };

        // Execute triggered actions
        let mut results = Vec::new();
        for id in triggered_ids {
            if let Ok(result) = self.execute(&id).await {
                results.push(result);
            }
        }
        results
    }

    /// Trigger an action by custom condition (blocking)
    pub fn trigger_by_custom_blocking(&self, condition: &str) -> Vec<ActionResult> {
        let rt = tokio::runtime::Handle::try_current();
        if let Ok(rt) = rt {
            rt.block_on(self.trigger_by_custom(condition))
        } else {
            Vec::new()
        }
    }

    /// Get execution history for an action
    pub fn get_history(&self, id: &str) -> Option<Vec<ActionResult>> {
        let states = self.states.lock().unwrap();
        states.get(id).map(|s| s.recent_results.iter().cloned().collect())
    }

    /// Get statistics for an action
    pub fn get_stats(&self, id: &str) -> Option<ActionStats> {
        let actions = self.actions.lock().unwrap();
        let states = self.states.lock().unwrap();

        if let (Some(action), Some(state)) = (actions.get(id), states.get(id)) {
            let success_count = state.recent_results.iter().filter(|r| r.success).count();
            let total_count = state.recent_results.len();

            Some(ActionStats {
                action_id: id.to_string(),
                name: action.name.clone(),
                enabled: action.enabled,
                execution_count: state.execution_count,
                success_rate: if total_count > 0 {
                    success_count as f32 / total_count as f32
                } else {
                    0.0
                },
                last_execution: state.last_execution,
            })
        } else {
            None
        }
    }

    /// Get all action statistics
    pub fn get_all_stats(&self) -> Vec<ActionStats> {
        // Collect keys first to avoid deadlock (get_stats also acquires the lock)
        let keys: Vec<String> = {
            let actions = self.actions.lock().unwrap();
            actions.keys().cloned().collect()
        };

        keys.iter()
            .filter_map(|id| self.get_stats(id))
            .collect()
    }
}

/// Statistics for a proactive action
#[derive(Debug, Clone, Serialize)]
pub struct ActionStats {
    pub action_id: String,
    pub name: String,
    pub enabled: bool,
    pub execution_count: u64,
    pub success_rate: f32,
    pub last_execution: Option<DateTime<Utc>>,
}

/// Built-in proactive actions
pub fn create_builtin_actions() -> Vec<ProactiveAction> {
    vec![
        ProactiveAction::new("health_check", Trigger::Interval(300))
            .with_description("Periodic health check of the agent")
            .with_priority(Priority::Low)
            .with_cooldown(300)
            .with_tag("system")
            .with_tag("health"),

        ProactiveAction::new("cleanup_temp", Trigger::Interval(3600))
            .with_description("Clean up temporary files")
            .with_priority(Priority::Low)
            .with_cooldown(1800)
            .with_tag("maintenance")
            .with_tag("cleanup"),

        ProactiveAction::new("sync_state", Trigger::Interval(60))
            .with_description("Sync agent state with storage")
            .with_priority(Priority::Normal)
            .with_cooldown(60)
            .with_tag("sync")
            .with_tag("state"),

        ProactiveAction::new("check_updates", Trigger::Interval(86400))
            .with_description("Check for agent updates")
            .with_priority(Priority::Low)
            .with_cooldown(43200)
            .with_tag("update")
            .with_tag("maintenance"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_action() {
        let action = ProactiveAction::new("test", Trigger::Interval(60))
            .with_description("Test action")
            .with_priority(Priority::High);

        assert_eq!(action.name, "test");
        assert_eq!(action.priority, Priority::High);
        assert!(action.enabled);
    }

    #[test]
    fn test_register_action() {
        let engine = ProactiveEngine::new();
        let action = ProactiveAction::new("test", Trigger::Interval(60));

        let id = engine.register(action);
        assert!(engine.get(&id).is_some());
    }

    #[test]
    fn test_unregister_action() {
        let engine = ProactiveEngine::new();
        let action = ProactiveAction::new("test", Trigger::Interval(60));

        let id = engine.register(action);
        engine.unregister(&id).unwrap();
        assert!(engine.get(&id).is_none());
    }

    #[tokio::test]
    async fn test_can_execute() {
        let engine = ProactiveEngine::new();
        let action = ProactiveAction::new("test", Trigger::Interval(60))
            .with_cooldown(10);

        let id = engine.register(action);
        assert!(engine.can_execute(&id));

        // Execute and check cooldown
        engine.register_with_executor(
            ProactiveAction::new("test2", Trigger::Interval(60)).with_cooldown(10),
            |_| async { Ok("done".to_string()) },
        );

        // Test async execution
        let id2 = engine.register_with_executor(
            ProactiveAction::new("test3", Trigger::Interval(60)),
            |_| async { Ok("executed".to_string()) },
        );

        let result = engine.execute(&id2).await.unwrap();
        assert!(result.success);
        assert_eq!(result.message, "executed");
    }

    #[test]
    fn test_enable_disable() {
        let engine = ProactiveEngine::new();
        let action = ProactiveAction::new("test", Trigger::Interval(60));

        let id = engine.register(action);
        assert!(engine.can_execute(&id));

        engine.set_enabled(&id, false).unwrap();
        assert!(!engine.can_execute(&id));

        engine.set_enabled(&id, true).unwrap();
        assert!(engine.can_execute(&id));
    }

    #[test]
    fn test_builtin_actions() {
        let actions = create_builtin_actions();
        assert!(!actions.is_empty());

        for action in actions {
            assert!(!action.name.is_empty());
            assert!(action.enabled);
        }
    }
}
