//! Lifecycle Hook System - extensible event hooks at key execution points
//!
//! Allows users and the agent itself to inject behavior at key points
//! without modifying core code. Hooks are registered with priorities
//! and fire in order.

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Points in the lifecycle where hooks can fire
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookPoint {
    BeforePromptBuild,
    AfterPromptBuild,
    BeforeToolExecution,
    AfterToolExecution,
    BeforeResponse,
    AfterResponse,
    OnError,
    OnSessionStart,
    OnSessionEnd,
    OnLearningCapture,
    OnToolLoopStart,
    OnToolLoopEnd,
}

impl std::fmt::Display for HookPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookPoint::BeforePromptBuild => write!(f, "before_prompt_build"),
            HookPoint::AfterPromptBuild => write!(f, "after_prompt_build"),
            HookPoint::BeforeToolExecution => write!(f, "before_tool_execution"),
            HookPoint::AfterToolExecution => write!(f, "after_tool_execution"),
            HookPoint::BeforeResponse => write!(f, "before_response"),
            HookPoint::AfterResponse => write!(f, "after_response"),
            HookPoint::OnError => write!(f, "on_error"),
            HookPoint::OnSessionStart => write!(f, "on_session_start"),
            HookPoint::OnSessionEnd => write!(f, "on_session_end"),
            HookPoint::OnLearningCapture => write!(f, "on_learning_capture"),
            HookPoint::OnToolLoopStart => write!(f, "on_tool_loop_start"),
            HookPoint::OnToolLoopEnd => write!(f, "on_tool_loop_end"),
        }
    }
}

/// Context passed to hook handlers
#[derive(Debug, Clone)]
pub struct HookContext {
    pub hook_point: HookPoint,
    pub data: HashMap<String, serde_json::Value>,
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
}

impl HookContext {
    pub fn new(hook_point: HookPoint, session_id: &str) -> Self {
        Self {
            hook_point,
            data: HashMap::new(),
            session_id: session_id.to_string(),
            timestamp: Utc::now(),
        }
    }

    pub fn with_data(mut self, key: &str, value: serde_json::Value) -> Self {
        self.data.insert(key.to_string(), value);
        self
    }
}

/// Action a hook can return
#[derive(Debug, Clone)]
pub enum HookAction {
    /// Continue with normal execution
    Continue,
    /// Modify data in the context
    ModifyData(HashMap<String, serde_json::Value>),
    /// Skip the operation
    Skip,
    /// Log a message
    Log(String),
}

/// Type alias for hook handler functions
pub type HookFn = Arc<dyn Fn(&HookContext) -> Result<Option<HookAction>> + Send + Sync>;

/// A registered hook with metadata
struct RegisteredHook {
    name: String,
    priority: i32,
    handler: HookFn,
}

/// Central hook registry
pub struct HookRegistry {
    hooks: HashMap<HookPoint, Vec<RegisteredHook>>,
}

impl HookRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }

    /// Register a hook at a specific point
    pub fn register(
        &mut self,
        point: HookPoint,
        name: &str,
        priority: i32,
        handler: HookFn,
    ) {
        let hook = RegisteredHook {
            name: name.to_string(),
            priority,
            handler,
        };

        let hooks = self.hooks.entry(point).or_default();
        hooks.push(hook);
        // Sort by priority (lower = earlier)
        hooks.sort_by_key(|h| h.priority);

        debug!("Registered hook '{}' at {} with priority {}", name, point, priority);
    }

    /// Unregister a hook by name
    pub fn unregister(&mut self, point: HookPoint, name: &str) -> bool {
        if let Some(hooks) = self.hooks.get_mut(&point) {
            let before = hooks.len();
            hooks.retain(|h| h.name != name);
            let removed = hooks.len() < before;
            if removed {
                debug!("Unregistered hook '{}' from {}", name, point);
            }
            removed
        } else {
            false
        }
    }

    /// Fire all hooks for a specific point
    pub fn fire(&self, point: HookPoint, context: &HookContext) -> Vec<HookAction> {
        let mut actions = Vec::new();

        if let Some(hooks) = self.hooks.get(&point) {
            for hook in hooks {
                match (hook.handler)(context) {
                    Ok(Some(action)) => {
                        debug!("Hook '{}' at {} returned action: {:?}", hook.name, point, action);
                        if matches!(action, HookAction::Skip) {
                            actions.push(action);
                            return actions; // Skip means stop processing
                        }
                        actions.push(action);
                    }
                    Ok(None) => {
                        // Hook didn't produce an action, continue
                    }
                    Err(e) => {
                        warn!("Hook '{}' at {} failed: {}", hook.name, point, e);
                    }
                }
            }
        }

        actions
    }

    /// Check if any hooks are registered for a point
    pub fn has_hooks(&self, point: HookPoint) -> bool {
        self.hooks.get(&point).map(|h| !h.is_empty()).unwrap_or(false)
    }

    /// Get count of registered hooks
    pub fn hook_count(&self) -> usize {
        self.hooks.values().map(|v| v.len()).sum()
    }

    /// List all registered hooks
    pub fn list_hooks(&self) -> Vec<(HookPoint, String, i32)> {
        let mut result = Vec::new();
        for (point, hooks) in &self.hooks {
            for hook in hooks {
                result.push((*point, hook.name.clone(), hook.priority));
            }
        }
        result
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if any HookActions indicate we should skip
pub fn should_skip(actions: &[HookAction]) -> bool {
    actions.iter().any(|a| matches!(a, HookAction::Skip))
}

/// Process log actions
pub fn process_log_actions(actions: &[HookAction]) {
    for action in actions {
        if let HookAction::Log(msg) = action {
            info!("[Hook] {}", msg);
        }
    }
}

/// Merge ModifyData actions into a single map
pub fn merge_data_actions(actions: &[HookAction]) -> HashMap<String, serde_json::Value> {
    let mut merged = HashMap::new();
    for action in actions {
        if let HookAction::ModifyData(data) = action {
            merged.extend(data.clone());
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_registry() {
        let mut registry = HookRegistry::new();

        let handler: HookFn = Arc::new(|_ctx| Ok(Some(HookAction::Continue)));
        registry.register(HookPoint::AfterResponse, "test_hook", 0, handler);

        assert!(registry.has_hooks(HookPoint::AfterResponse));
        assert!(!registry.has_hooks(HookPoint::BeforeResponse));
        assert_eq!(registry.hook_count(), 1);
    }

    #[test]
    fn test_hook_priority_ordering() {
        let mut registry = HookRegistry::new();

        let results = Arc::new(std::sync::Mutex::new(Vec::new()));

        let r1 = results.clone();
        let handler1: HookFn = Arc::new(move |_| {
            r1.lock().unwrap().push(1);
            Ok(Some(HookAction::Log("first".to_string())))
        });

        let r2 = results.clone();
        let handler2: HookFn = Arc::new(move |_| {
            r2.lock().unwrap().push(2);
            Ok(Some(HookAction::Log("second".to_string())))
        });

        registry.register(HookPoint::OnError, "hook_b", 10, handler2);
        registry.register(HookPoint::OnError, "hook_a", 5, handler1);

        let ctx = HookContext::new(HookPoint::OnError, "test-session");
        let _ = registry.fire(HookPoint::OnError, &ctx);

        let order = results.lock().unwrap();
        assert_eq!(*order, vec![1, 2]); // Priority 5 first, then 10
    }

    #[test]
    fn test_unregister() {
        let mut registry = HookRegistry::new();
        let handler: HookFn = Arc::new(|_| Ok(None));
        registry.register(HookPoint::OnSessionStart, "test", 0, handler);
        assert_eq!(registry.hook_count(), 1);

        assert!(registry.unregister(HookPoint::OnSessionStart, "test"));
        assert_eq!(registry.hook_count(), 0);
    }

    #[test]
    fn test_should_skip() {
        assert!(!should_skip(&[HookAction::Continue]));
        assert!(should_skip(&[HookAction::Skip]));
        assert!(should_skip(&[HookAction::Continue, HookAction::Skip]));
    }
}
