//! File and event monitoring
//!
//! Watches files and directories for changes and triggers callbacks.

use anyhow::{Result, Context, bail};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{info, warn, error};
use uuid::Uuid;

/// File system event type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileEvent {
    Created,
    Modified,
    Deleted,
    Renamed { from: String, to: String },
    Any,
}

impl std::fmt::Display for FileEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileEvent::Created => write!(f, "Created"),
            FileEvent::Modified => write!(f, "Modified"),
            FileEvent::Deleted => write!(f, "Deleted"),
            FileEvent::Renamed { from, to } => write!(f, "Renamed({} -> {})", from, to),
            FileEvent::Any => write!(f, "Any"),
        }
    }
}

/// A watched path configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchConfig {
    /// Unique watch ID
    pub id: String,
    /// Path to watch
    pub path: PathBuf,
    /// Events to watch for
    pub events: Vec<FileEvent>,
    /// File patterns to match (glob style)
    pub patterns: Vec<String>,
    /// Whether to watch recursively
    pub recursive: bool,
    /// Debounce time in milliseconds
    pub debounce_ms: u64,
    /// Whether this watch is enabled
    pub enabled: bool,
    /// Tags for categorization
    pub tags: Vec<String>,
}

impl WatchConfig {
    /// Create a new watch configuration for a path
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            path: path.into(),
            events: vec![FileEvent::Any],
            patterns: Vec::new(),
            recursive: true,
            debounce_ms: 500,
            enabled: true,
            tags: Vec::new(),
        }
    }

    /// Set events to watch
    pub fn with_events(mut self, events: Vec<FileEvent>) -> Self {
        self.events = events;
        self
    }

    /// Set file patterns
    pub fn with_patterns(mut self, patterns: Vec<&str>) -> Self {
        self.patterns = patterns.into_iter().map(|s| s.to_string()).collect();
        self
    }

    /// Set recursive mode
    pub fn with_recursive(mut self, recursive: bool) -> Self {
        self.recursive = recursive;
        self
    }

    /// Set debounce time
    pub fn with_debounce(mut self, ms: u64) -> Self {
        self.debounce_ms = ms;
        self
    }

    /// Check if a path matches the patterns
    pub fn matches_pattern(&self, path: &Path) -> bool {
        if self.patterns.is_empty() {
            return true;
        }

        let path_str = path.to_string_lossy();
        let filename = path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();

        for pattern in &self.patterns {
            // Simple glob matching: * matches anything
            if pattern.contains('*') {
                let parts: Vec<&str> = pattern.split('*').collect();
                if parts.len() == 2 {
                    let prefix = parts[0];
                    let suffix = parts[1];
                    if filename.starts_with(prefix) && filename.ends_with(suffix) {
                        return true;
                    }
                }
            } else if filename == *pattern || path_str.ends_with(pattern) {
                return true;
            }
        }

        false
    }

    /// Check if event type matches
    pub fn matches_event(&self, event: &FileEvent) -> bool {
        self.events.contains(&FileEvent::Any) || self.events.contains(event)
    }
}

/// A file system event with context
#[derive(Debug, Clone)]
pub struct FileSystemEvent {
    /// Watch ID that triggered this event
    pub watch_id: String,
    /// Type of event
    pub event_type: FileEvent,
    /// Path that was affected
    pub path: PathBuf,
    /// Timestamp of the event
    pub timestamp: Instant,
}

/// Callback type for file events
pub type FileEventCallback = Box<dyn Fn(&FileSystemEvent) + Send + Sync>;

/// File watcher
pub struct FileWatcher {
    /// Watch configurations
    watches: Arc<Mutex<HashMap<String, WatchConfig>>>,
    /// Event callbacks
    callbacks: Arc<Mutex<HashMap<String, Vec<FileEventCallback>>>>,
    /// Debounce tracking
    debounce: Arc<Mutex<HashMap<String, Instant>>>,
    /// The underlying notify watcher
    watcher: Arc<Mutex<Option<RecommendedWatcher>>>,
    /// Running flag
    running: Arc<Mutex<bool>>,
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl FileWatcher {
    /// Create a new file watcher
    pub fn new() -> Self {
        Self {
            watches: Arc::new(Mutex::new(HashMap::new())),
            callbacks: Arc::new(Mutex::new(HashMap::new())),
            debounce: Arc::new(Mutex::new(HashMap::new())),
            watcher: Arc::new(Mutex::new(None)),
            running: Arc::new(Mutex::new(false)),
        }
    }

    /// Add a watch with a callback
    pub fn add_watch(&self, config: WatchConfig, callback: FileEventCallback) -> Result<String> {
        let id = config.id.clone();
        let path = config.path.clone();

        // Store config
        {
            let mut watches = self.watches.lock().unwrap();
            watches.insert(id.clone(), config);
        }

        // Store callback
        {
            let mut callbacks = self.callbacks.lock().unwrap();
            callbacks.entry(id.clone()).or_insert_with(Vec::new).push(callback);
        }

        // If watcher is already running, add the path
        {
            let mut watcher = self.watcher.lock().unwrap();
            if let Some(ref mut w) = *watcher {
                let mode = if self.watches.lock().unwrap().get(&id).map_or(false, |c| c.recursive) {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };
                w.watch(&path, mode)
                    .with_context(|| format!("Failed to watch path: {:?}", path))?;
            }
        }

        info!("Added watch: {} -> {:?}", id, path);
        Ok(id)
    }

    /// Remove a watch
    pub fn remove_watch(&self, id: &str) -> Result<()> {
        let path = {
            let mut watches = self.watches.lock().unwrap();
            watches.remove(id).map(|c| c.path)
        };

        if let Some(path) = path {
            // Remove from callbacks
            {
                let mut callbacks = self.callbacks.lock().unwrap();
                callbacks.remove(id);
            }

            // Unwatch the path
            {
                let mut watcher = self.watcher.lock().unwrap();
                if let Some(ref mut w) = *watcher {
                    w.unwatch(&path).ok();
                }
            }

            info!("Removed watch: {}", id);
            Ok(())
        } else {
            bail!("Watch not found: {}", id)
        }
    }

    /// Get watch configuration
    pub fn get_watch(&self, id: &str) -> Option<WatchConfig> {
        self.watches.lock().unwrap().get(id).cloned()
    }

    /// List all watches
    pub fn list_watches(&self) -> Vec<WatchConfig> {
        self.watches.lock().unwrap().values().cloned().collect()
    }

    /// Enable/disable a watch
    pub fn set_watch_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let mut watches = self.watches.lock().unwrap();
        if let Some(config) = watches.get_mut(id) {
            config.enabled = enabled;
            info!("Watch {} {}", id, if enabled { "enabled" } else { "disabled" });
            Ok(())
        } else {
            bail!("Watch not found: {}", id)
        }
    }

    /// Start watching
    pub fn start(&self) -> Result<()> {
        let mut running = self.running.lock().unwrap();
        if *running {
            warn!("File watcher already running");
            return Ok(());
        }

        // Create event channel
        let (tx, mut rx) = mpsc::channel::<FileSystemEvent>(100);

        // Create the watcher
        let watches = self.watches.clone();
        let debounce = self.debounce.clone();

        let event_handler = move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) => {
                    // Find matching watches
                    let watches = watches.lock().unwrap();
                    for (id, config) in watches.iter() {
                        if !config.enabled {
                            continue;
                        }

                        // Check if any path matches
                        for path in &event.paths {
                            if path.starts_with(&config.path) && config.matches_pattern(path) {
                                // Convert event type
                                let event_type = match event.kind {
                                    EventKind::Create(_) => FileEvent::Created,
                                    EventKind::Modify(_) => FileEvent::Modified,
                                    EventKind::Remove(_) => FileEvent::Deleted,
                                    EventKind::Any => FileEvent::Any,
                                    _ => continue,
                                };

                                if !config.matches_event(&event_type) {
                                    continue;
                                }

                                // Check debounce
                                let key = format!("{}:{}", id, path.display());
                                let mut debounce = debounce.lock().unwrap();
                                let now = Instant::now();

                                if let Some(last) = debounce.get(&key) {
                                    if now.duration_since(*last) < Duration::from_millis(config.debounce_ms) {
                                        continue; // Skip due to debounce
                                    }
                                }
                                debounce.insert(key, now);

                                // Send event
                                let fs_event = FileSystemEvent {
                                    watch_id: id.clone(),
                                    event_type,
                                    path: path.clone(),
                                    timestamp: now,
                                };

                                if tx.blocking_send(fs_event).is_err() {
                                    warn!("Failed to send file event");
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Watch error: {}", e);
                }
            }
        };

        let mut watcher = notify::recommended_watcher(event_handler)
            .context("Failed to create file watcher")?;

        // Watch all configured paths
        let watches_lock = self.watches.lock().unwrap();
        for (id, config) in watches_lock.iter() {
            if config.enabled {
                let mode = if config.recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };
                if let Err(e) = watcher.watch(&config.path, mode) {
                    warn!("Failed to watch {} ({:?}): {}", id, config.path, e);
                }
            }
        }
        drop(watches_lock);

        // Store the watcher
        *self.watcher.lock().unwrap() = Some(watcher);
        *running = true;
        drop(running);

        info!("File watcher started");

        // Start event processing loop in background
        let callbacks = self.callbacks.clone();
        let running = self.running.clone();

        tokio::spawn(async move {
            loop {
                // Check if still running
                {
                    let running = running.lock().unwrap();
                    if !*running {
                        break;
                    }
                }

                // Process events
                while let Ok(event) = rx.try_recv() {
                    let callbacks = callbacks.lock().unwrap();
                    if let Some(cbs) = callbacks.get(&event.watch_id) {
                        for callback in cbs {
                            callback(&event);
                        }
                    }
                }

                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        Ok(())
    }

    /// Stop watching
    pub fn stop(&self) {
        let mut running = self.running.lock().unwrap();
        if !*running {
            return;
        }

        // Stop the watcher
        {
            let mut watcher = self.watcher.lock().unwrap();
            if let Some(mut w) = watcher.take() {
                // Unwatch all paths
                let watches = self.watches.lock().unwrap();
                for config in watches.values() {
                    w.unwatch(&config.path).ok();
                }
            }
        }

        *running = false;
        info!("File watcher stopped");
    }

    /// Check if watcher is running
    pub fn is_running(&self) -> bool {
        *self.running.lock().unwrap()
    }

    /// Get watcher statistics
    pub fn stats(&self) -> WatcherStats {
        let watches = self.watches.lock().unwrap();
        let total = watches.len();
        let enabled = watches.values().filter(|w| w.enabled).count();

        WatcherStats {
            total_watches: total,
            enabled_watches: enabled,
            is_running: *self.running.lock().unwrap(),
        }
    }
}

/// Watcher statistics
#[derive(Debug, Clone, Serialize)]
pub struct WatcherStats {
    pub total_watches: usize,
    pub enabled_watches: usize,
    pub is_running: bool,
}

/// Helper to create a simple watch
pub fn watch_path(
    path: impl Into<PathBuf>,
    events: Vec<FileEvent>,
    callback: FileEventCallback,
) -> Result<(FileWatcher, String)> {
    let watcher = FileWatcher::new();
    let config = WatchConfig::new(path)
        .with_events(events);

    let id = watcher.add_watch(config, callback)?;
    Ok((watcher, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_watch_config() {
        let config = WatchConfig::new("/tmp/test")
            .with_events(vec![FileEvent::Modified])
            .with_patterns(vec!["*.txt"]);

        assert!(config.recursive);
        assert_eq!(config.events.len(), 1);
        assert!(config.matches_pattern(Path::new("file.txt")));
        assert!(!config.matches_pattern(Path::new("file.rs")));
    }

    #[test]
    fn test_pattern_matching() {
        let config = WatchConfig::new("/tmp")
            .with_patterns(vec!["*.rs", "*.txt"]);

        assert!(config.matches_pattern(Path::new("main.rs")));
        assert!(config.matches_pattern(Path::new("readme.txt")));
        assert!(!config.matches_pattern(Path::new("main.go")));
    }

    #[test]
    fn test_event_matching() {
        let config = WatchConfig::new("/tmp")
            .with_events(vec![FileEvent::Modified, FileEvent::Created]);

        assert!(config.matches_event(&FileEvent::Modified));
        assert!(config.matches_event(&FileEvent::Created));
        assert!(!config.matches_event(&FileEvent::Deleted));
    }

    #[test]
    fn test_any_event_matching() {
        let config = WatchConfig::new("/tmp")
            .with_events(vec![FileEvent::Any]);

        assert!(config.matches_event(&FileEvent::Modified));
        assert!(config.matches_event(&FileEvent::Created));
        assert!(config.matches_event(&FileEvent::Deleted));
    }

    #[tokio::test]
    async fn test_watcher_add_remove() {
        let watcher = FileWatcher::new();
        let temp_dir = TempDir::new().unwrap();

        let config = WatchConfig::new(temp_dir.path());
        let callback: FileEventCallback = Box::new(|_| {});

        let id = watcher.add_watch(config, callback).unwrap();
        assert!(watcher.get_watch(&id).is_some());

        watcher.remove_watch(&id).unwrap();
        assert!(watcher.get_watch(&id).is_none());
    }
}
