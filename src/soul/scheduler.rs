//! Cron-based scheduler
//!
//! Schedules and executes tasks based on cron expressions or intervals.

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{info, warn, error};
use uuid::Uuid;

/// A scheduled task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// Unique task ID
    pub id: String,
    /// Task name
    pub name: String,
    /// Cron expression or interval specification
    pub schedule: TaskSchedule,
    /// Task description
    pub description: Option<String>,
    /// Whether the task is enabled
    pub enabled: bool,
    /// Last execution time
    pub last_run: Option<DateTime<Utc>>,
    /// Next scheduled execution time
    pub next_run: Option<DateTime<Utc>>,
    /// Number of times executed
    pub run_count: u64,
    /// Maximum number of executions (None = unlimited)
    pub max_runs: Option<u64>,
    /// Tags for categorization
    pub tags: Vec<String>,
}

/// Task schedule specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskSchedule {
    /// Cron expression (e.g., "0 0 * * * *" for hourly)
    Cron(String),
    /// Fixed interval in seconds
    Interval(u64),
    /// Run once at specific time
    Once(DateTime<Utc>),
    /// Run on startup
    OnStartup,
}

impl TaskSchedule {
    /// Parse and validate a cron expression
    pub fn parse_cron(expr: &str) -> Result<Schedule> {
        Schedule::try_from(expr)
            .map_err(|e| anyhow::anyhow!("Invalid cron expression '{}': {}", expr, e))
    }

    /// Calculate the next run time from now
    pub fn next_run(&self) -> Result<Option<DateTime<Utc>>> {
        match self {
            TaskSchedule::Cron(expr) => {
                let schedule = Self::parse_cron(expr)?;
                let next = schedule.upcoming(Utc).next();
                Ok(next)
            }
            TaskSchedule::Interval(secs) => {
                let next = Utc::now() + chrono::Duration::seconds(*secs as i64);
                Ok(Some(next))
            }
            TaskSchedule::Once(time) => {
                if time > &Utc::now() {
                    Ok(Some(*time))
                } else {
                    Ok(None) // Already passed
                }
            }
            TaskSchedule::OnStartup => {
                Ok(None) // Not scheduled, runs on startup
            }
        }
    }
}

/// Task execution result
#[derive(Debug)]
pub struct TaskResult {
    pub task_id: String,
    pub success: bool,
    pub message: String,
    pub duration_ms: u64,
}

/// Type alias for task executor function
pub type TaskExecutor = Arc<dyn Fn() -> Pin<Box<dyn std::future::Future<Output = Result<String>> + Send>> + Send + Sync>;

use std::pin::Pin;

/// Scheduler for managing scheduled tasks
pub struct TaskScheduler {
    /// Scheduled tasks
    tasks: Arc<Mutex<HashMap<String, ScheduledTask>>>,
    /// Task executors
    executors: Arc<Mutex<HashMap<String, TaskExecutor>>>,
    /// Running flag
    running: Arc<Mutex<bool>>,
}

impl Default for TaskScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskScheduler {
    /// Create a new scheduler
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            executors: Arc::new(Mutex::new(HashMap::new())),
            running: Arc::new(Mutex::new(false)),
        }
    }

    /// Add a scheduled task
    pub async fn add_task(&self, mut task: ScheduledTask) -> Result<String> {
        // Calculate next run time
        task.next_run = task.schedule.next_run()?;

        let id = task.id.clone();
        let mut tasks = self.tasks.lock().await;
        tasks.insert(id.clone(), task);

        info!("Added scheduled task: {}", id);
        Ok(id)
    }

    /// Add a task with an executor function
    pub async fn add_task_with_executor<F, Fut>(
        &self,
        mut task: ScheduledTask,
        executor: F,
    ) -> Result<String>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<String>> + Send + 'static,
    {
        task.next_run = task.schedule.next_run()?;

        let id = task.id.clone();

        // Store task
        {
            let mut tasks = self.tasks.lock().await;
            tasks.insert(id.clone(), task);
        }

        // Store executor
        {
            let mut executors = self.executors.lock().await;
            executors.insert(id.clone(), Arc::new(move || Box::pin(executor())));
        }

        info!("Added scheduled task with executor: {}", id);
        Ok(id)
    }

    /// Remove a task
    pub async fn remove_task(&self, id: &str) -> Result<()> {
        let mut tasks = self.tasks.lock().await;
        let mut executors = self.executors.lock().await;

        if tasks.remove(id).is_some() {
            executors.remove(id);
            info!("Removed scheduled task: {}", id);
            Ok(())
        } else {
            bail!("Task not found: {}", id)
        }
    }

    /// Enable/disable a task
    pub async fn set_task_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(id) {
            task.enabled = enabled;
            info!("Task {} {}", id, if enabled { "enabled" } else { "disabled" });
            Ok(())
        } else {
            bail!("Task not found: {}", id)
        }
    }

    /// Get a task by ID
    pub async fn get_task(&self, id: &str) -> Option<ScheduledTask> {
        self.tasks.lock().await.get(id).cloned()
    }

    /// List all tasks
    pub async fn list_tasks(&self) -> Vec<ScheduledTask> {
        self.tasks.lock().await.values().cloned().collect()
    }

    /// List tasks by tag
    pub async fn list_tasks_by_tag(&self, tag: &str) -> Vec<ScheduledTask> {
        self.tasks
            .lock()
            .await
            .values()
            .filter(|t| t.tags.contains(&tag.to_string()))
            .cloned()
            .collect()
    }

    /// Execute a task immediately
    pub async fn execute_now(&self, id: &str) -> Result<TaskResult> {
        let executor = {
            let executors = self.executors.lock().await;
            executors.get(id).cloned()
        };

        if let Some(exec) = executor {
            let start = std::time::Instant::now();

            let result = exec().await;
            let duration_ms = start.elapsed().as_millis() as u64;

            // Update task stats
            {
                let mut tasks = self.tasks.lock().await;
                if let Some(task) = tasks.get_mut(id) {
                    task.last_run = Some(Utc::now());
                    task.run_count += 1;
                    task.next_run = task.schedule.next_run().ok().flatten();

                    // Check if max runs reached
                    if let Some(max) = task.max_runs {
                        if task.run_count >= max {
                            task.enabled = false;
                        }
                    }
                }
            }

            match result {
                Ok(message) => Ok(TaskResult {
                    task_id: id.to_string(),
                    success: true,
                    message,
                    duration_ms,
                }),
                Err(e) => Ok(TaskResult {
                    task_id: id.to_string(),
                    success: false,
                    message: e.to_string(),
                    duration_ms,
                }),
            }
        } else {
            bail!("No executor for task: {}", id)
        }
    }

    /// Get tasks that are due to run
    pub async fn get_due_tasks(&self) -> Vec<String> {
        let now = Utc::now();
        let tasks = self.tasks.lock().await;

        tasks
            .values()
            .filter(|t| {
                t.enabled
                    && t.next_run.map_or(false, |next| next <= now)
            })
            .map(|t| t.id.clone())
            .collect()
    }

    /// Start the scheduler loop
    pub async fn start(&self) {
        let mut running = self.running.lock().await;
        if *running {
            warn!("Scheduler already running");
            return;
        }
        *running = true;
        drop(running);

        info!("Scheduler started");

        loop {
            // Check if we should stop
            {
                let running = self.running.lock().await;
                if !*running {
                    break;
                }
            }

            // Get due tasks
            let due_tasks = self.get_due_tasks().await;

            // Execute due tasks
            for task_id in due_tasks {
                match self.execute_now(&task_id).await {
                    Ok(result) => {
                        if result.success {
                            info!("Task {} completed: {} ({}ms)",
                                result.task_id, result.message, result.duration_ms);
                        } else {
                            warn!("Task {} failed: {}", result.task_id, result.message);
                        }
                    }
                    Err(e) => {
                        error!("Task {} execution error: {}", task_id, e);
                    }
                }
            }

            // Sleep until next check
            sleep(Duration::from_secs(1)).await;
        }

        info!("Scheduler stopped");
    }

    /// Stop the scheduler
    pub async fn stop(&self) {
        let mut running = self.running.lock().await;
        *running = false;
        info!("Stopping scheduler...");
    }

    /// Check if scheduler is running
    pub async fn is_running(&self) -> bool {
        *self.running.lock().await
    }

    /// Get scheduler statistics
    pub async fn stats(&self) -> SchedulerStats {
        let tasks = self.tasks.lock().await;
        let total = tasks.len();
        let enabled = tasks.values().filter(|t| t.enabled).count();
        let total_runs: u64 = tasks.values().map(|t| t.run_count).sum();

        SchedulerStats {
            total_tasks: total,
            enabled_tasks: enabled,
            total_runs,
            is_running: *self.running.lock().await,
        }
    }
}

/// Scheduler statistics
#[derive(Debug, Clone, Serialize)]
pub struct SchedulerStats {
    pub total_tasks: usize,
    pub enabled_tasks: usize,
    pub total_runs: u64,
    pub is_running: bool,
}

/// Helper function to create a simple recurring task
pub fn create_recurring_task(
    name: &str,
    interval_secs: u64,
    description: Option<&str>,
) -> ScheduledTask {
    ScheduledTask {
        id: Uuid::new_v4().to_string(),
        name: name.to_string(),
        schedule: TaskSchedule::Interval(interval_secs),
        description: description.map(|s| s.to_string()),
        enabled: true,
        last_run: None,
        next_run: None,
        run_count: 0,
        max_runs: None,
        tags: Vec::new(),
    }
}

/// Helper function to create a cron-based task
pub fn create_cron_task(
    name: &str,
    cron_expr: &str,
    description: Option<&str>,
) -> Result<ScheduledTask> {
    // Validate cron expression
    TaskSchedule::parse_cron(cron_expr)?;

    Ok(ScheduledTask {
        id: Uuid::new_v4().to_string(),
        name: name.to_string(),
        schedule: TaskSchedule::Cron(cron_expr.to_string()),
        description: description.map(|s| s.to_string()),
        enabled: true,
        last_run: None,
        next_run: None,
        run_count: 0,
        max_runs: None,
        tags: Vec::new(),
    })
}

/// Helper function to create a one-time task
pub fn create_one_time_task(
    name: &str,
    run_at: DateTime<Utc>,
    description: Option<&str>,
) -> ScheduledTask {
    ScheduledTask {
        id: Uuid::new_v4().to_string(),
        name: name.to_string(),
        schedule: TaskSchedule::Once(run_at),
        description: description.map(|s| s.to_string()),
        enabled: true,
        last_run: None,
        next_run: None,
        run_count: 0,
        max_runs: Some(1),
        tags: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interval_schedule() {
        let schedule = TaskSchedule::Interval(60);
        let next = schedule.next_run().unwrap();
        assert!(next.is_some());
    }

    #[test]
    fn test_cron_schedule() {
        let schedule = TaskSchedule::Cron("0 0 * * * *".to_string());
        let next = schedule.next_run().unwrap();
        assert!(next.is_some());
    }

    #[test]
    fn test_create_recurring_task() {
        let task = create_recurring_task("test", 60, Some("Test task"));
        assert_eq!(task.name, "test");
        assert!(task.enabled);
    }

    #[tokio::test]
    async fn test_scheduler_add_remove() {
        let scheduler = TaskScheduler::new();
        let task = create_recurring_task("test", 60, None);

        let id = scheduler.add_task(task).await.unwrap();
        assert!(scheduler.get_task(&id).await.is_some());

        scheduler.remove_task(&id).await.unwrap();
        assert!(scheduler.get_task(&id).await.is_none());
    }

    #[tokio::test]
    async fn test_scheduler_execute() {
        let scheduler = TaskScheduler::new();
        let task = create_recurring_task("test", 60, None);

        scheduler.add_task_with_executor(task, || async {
            Ok("Executed!".to_string())
        }).await.unwrap();

        let tasks = scheduler.list_tasks().await;
        let task_id = tasks[0].id.clone();

        let result = scheduler.execute_now(&task_id).await.unwrap();
        assert!(result.success);
        assert_eq!(result.message, "Executed!");
    }
}
