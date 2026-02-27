//! File system sandbox
//!
//! Restricts file operations to allowed directories and blocks access to sensitive files.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Default blocked paths (sensitive system files and directories)
pub const DEFAULT_BLOCKED_PATHS: &[&str] = &[
    "/etc/passwd",
    "/etc/shadow",
    "/etc/sudoers",
    "/root/.ssh",
    "/root/.gnupg",
    ".ssh",
    ".gnupg",
    ".env",
    ".env.local",
    ".env.production",
    "credentials.json",
    "secrets.json",
    "id_rsa",
    "id_ed25519",
    ".pem",
    ".key",
];

/// Directories that are always blocked (entire subtree)
pub const BLOCKED_DIRECTORIES: &[&str] = &[
    "/root",
    "/etc/shadow",
    "/etc/sudoers.d",
    "/var/lib/private",
    "/run/secrets",
];

/// System directories where writes/deletes are always blocked
pub const SYSTEM_WRITE_BLOCKED: &[&str] = &[
    "/usr/bin",
    "/usr/sbin",
    "/usr/lib",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/sbin",
    "/bin",
    "/boot",
    "/sys",
    "/proc",
    "/dev",
];

/// Default allowed paths (user workspace directories)
pub const DEFAULT_ALLOWED_PATHS: &[&str] = &[
    "~/Documents",
    "~/Projects",
    "~/workspace",
    "~/code",
    "~/src",
    "~/Downloads",
    "/tmp",
];

/// Risk levels for file operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    Low,      // Reading allowed files
    Medium,   // Writing to allowed files
    High,     // Reading potentially sensitive files
    Critical, // Writing to system directories, deleting files
}

/// File operation types
#[derive(Debug, Clone)]
pub enum FileOperation {
    Read,
    Write,
    Delete,
    Execute,
    List,
}

impl FileOperation {
    pub fn risk_level(&self) -> RiskLevel {
        match self {
            FileOperation::Read | FileOperation::List => RiskLevel::Low,
            FileOperation::Write => RiskLevel::Medium,
            FileOperation::Execute => RiskLevel::High,
            FileOperation::Delete => RiskLevel::Critical,
        }
    }
}

/// File system sandbox configuration
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Allowed directory paths (user can access these)
    pub allowed_paths: Vec<PathBuf>,
    /// Blocked file patterns (never accessible)
    pub blocked_patterns: Vec<String>,
    /// Whether to allow access outside allowed paths (requires approval)
    pub allow_outside_with_approval: bool,
    /// Whether to block hidden files by default
    pub block_hidden_files: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());

        let allowed_paths = DEFAULT_ALLOWED_PATHS
            .iter()
            .map(|p| {
                if p.starts_with("~/") {
                    PathBuf::from(&home).join(&p[2..])
                } else {
                    PathBuf::from(p)
                }
            })
            .collect();

        let blocked_patterns = DEFAULT_BLOCKED_PATHS
            .iter()
            .map(|s| s.to_string())
            .collect();

        Self {
            allowed_paths,
            blocked_patterns,
            allow_outside_with_approval: true,
            block_hidden_files: false,
        }
    }
}

/// File system sandbox
#[derive(Clone)]
pub struct FileSystemSandbox {
    config: SandboxConfig,
}

impl FileSystemSandbox {
    /// Create a new sandbox with default configuration
    pub fn new() -> Self {
        Self {
            config: SandboxConfig::default(),
        }
    }

    /// Create a sandbox with custom configuration
    pub fn with_config(config: SandboxConfig) -> Self {
        Self { config }
    }

    /// Get the current configuration
    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// Resolve a path, handling ~ expansion and symlinks
    pub fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let expanded = if path.starts_with('~') {
            let home = std::env::var("HOME")
                .unwrap_or_else(|_| "/home/user".to_string());
            PathBuf::from(&home).join(&path[2..])
        } else if PathBuf::from(path).is_absolute() {
            PathBuf::from(path)
        } else {
            // For relative paths, join with current directory
            let cwd = std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            cwd.join(path)
        };

        // If the path exists, canonicalize it
        if expanded.exists() {
            return Ok(expanded.canonicalize()
                .unwrap_or_else(|_| expanded.clone()));
        }

        // For non-existent paths, try to find the file in project subdirectories
        // This handles the case where user runs from parent of project directory
        if let Some(found) = self.find_in_project_subdirs(&expanded) {
            return Ok(found);
        }

        // For non-existent paths, resolve parent and join
        let canonical = if let Some(parent) = expanded.parent() {
            if parent.exists() {
                parent.canonicalize()
                    .map(|p| p.join(expanded.file_name().unwrap_or_default()))
                    .unwrap_or(expanded)
            } else {
                expanded
            }
        } else {
            expanded
        };

        Ok(canonical)
    }

    /// Try to find a path in project subdirectories (those containing Cargo.toml)
    fn find_in_project_subdirs(&self, target: &Path) -> Option<PathBuf> {
        let cwd = std::env::current_dir().ok()?;

        // Look for project directories (containing Cargo.toml or package.json)
        if let Ok(entries) = std::fs::read_dir(&cwd) {
            for entry in entries.flatten() {
                let subdir = entry.path();
                if subdir.is_dir() {
                    // Check if it's a project directory
                    if subdir.join("Cargo.toml").exists() || subdir.join("package.json").exists() {
                        // Try the target path relative to this subdirectory
                        if let Some(file_name) = target.file_name() {
                            if let Some(parent) = target.parent() {
                                // Check if parent is "src" or similar common dirs
                                if let Some(parent_name) = parent.file_name() {
                                    let candidate = subdir.join(parent_name).join(file_name);
                                    if candidate.exists() {
                                        return candidate.canonicalize().ok();
                                    }
                                }
                            }
                        }

                        // Also try direct join
                        let relative = target.strip_prefix(&cwd).ok()?;
                        let candidate = subdir.join(relative);
                        if candidate.exists() {
                            return candidate.canonicalize().ok();
                        }
                    }
                }
            }
        }

        None
    }

    /// Check if a path is blocked (sensitive)
    pub fn is_blocked(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy().to_lowercase();

        // Check blocked directories (entire subtree is blocked)
        for dir in BLOCKED_DIRECTORIES {
            let dir_lower = dir.to_lowercase();
            if path_str == dir_lower || path_str.starts_with(&format!("{}/", dir_lower)) {
                return true;
            }
        }

        // Check blocked patterns
        for pattern in &self.config.blocked_patterns {
            let pattern_lower = pattern.to_lowercase();

            // Check if pattern matches the end of the path (for relative patterns)
            if path_str.ends_with(&pattern_lower) ||
               path_str.contains(&pattern_lower) {
                return true;
            }

            // Check filename for exact match
            if let Some(filename) = path.file_name() {
                if filename.to_string_lossy().to_lowercase() == pattern_lower {
                    return true;
                }
            }
        }

        // Check for hidden files
        if self.config.block_hidden_files {
            if let Some(filename) = path.file_name() {
                if filename.to_string_lossy().starts_with('.') {
                    return true;
                }
            }
        }

        // Check for path traversal attempts
        let path_str = path.to_string_lossy();
        if path_str.contains("..") {
            return true;
        }

        false
    }

    /// Check if a path is in the allowed directories
    pub fn is_allowed(&self, path: &Path) -> bool {
        for allowed in &self.config.allowed_paths {
            if path.starts_with(allowed) {
                return true;
            }
        }
        false
    }

    /// Validate a path for an operation
    pub fn validate(&self, path: &Path, operation: &FileOperation) -> Result<SandboxResult> {
        let resolved = self.resolve_path(&path.to_string_lossy())?;

        // First check if blocked (highest priority)
        if self.is_blocked(&resolved) {
            return Ok(SandboxResult {
                allowed: false,
                requires_approval: false,
                risk_level: RiskLevel::Critical,
                reason: format!("Path '{}' is blocked (sensitive path)", path.display()),
                resolved_path: resolved,
            });
        }

        // Block writes/deletes to system directories
        if matches!(operation, FileOperation::Write | FileOperation::Delete) {
            let resolved_str = resolved.to_string_lossy().to_lowercase();
            for sys_dir in SYSTEM_WRITE_BLOCKED {
                let sys_lower = sys_dir.to_lowercase();
                if resolved_str == sys_lower || resolved_str.starts_with(&format!("{}/", sys_lower)) {
                    return Ok(SandboxResult {
                        allowed: false,
                        requires_approval: false,
                        risk_level: RiskLevel::Critical,
                        reason: format!("Cannot write to system directory: {}", sys_dir),
                        resolved_path: resolved,
                    });
                }
            }
        }

        // Check if in allowed paths
        if self.is_allowed(&resolved) {
            return Ok(SandboxResult {
                allowed: true,
                requires_approval: operation.risk_level() >= RiskLevel::Medium,
                risk_level: operation.risk_level(),
                reason: "Path is in allowed directory".to_string(),
                resolved_path: resolved,
            });
        }

        // Outside allowed paths
        if self.config.allow_outside_with_approval {
            Ok(SandboxResult {
                allowed: false,
                requires_approval: true,
                risk_level: RiskLevel::High,
                reason: format!("Path '{}' is outside allowed directories", path.display()),
                resolved_path: resolved,
            })
        } else {
            Ok(SandboxResult {
                allowed: false,
                requires_approval: false,
                risk_level: RiskLevel::High,
                reason: "Path is outside allowed directories and requires approval".to_string(),
                resolved_path: resolved,
            })
        }
    }

    /// Check if an operation can proceed
    pub fn can_proceed(&self, path: &Path, operation: &FileOperation) -> Result<bool> {
        let result = self.validate(path, operation)?;
        Ok(result.allowed)
    }

    /// Check access for an operation (wrapper for validate with approval-focused result)
    pub fn check_access(&self, path: &Path, operation: &FileOperation) -> Result<AccessCheck> {
        let result = self.validate(path, operation)?;
        Ok(AccessCheck {
            allowed: result.allowed,
            reason: Some(result.reason),
            requires_approval: result.requires_approval,
            risk_level: result.risk_level,
        })
    }

    /// Add an allowed path
    pub fn allow_path(&mut self, path: PathBuf) {
        if !self.config.allowed_paths.contains(&path) {
            self.config.allowed_paths.push(path);
        }
    }

    /// Remove an allowed path
    pub fn disallow_path(&mut self, path: &Path) {
        self.config.allowed_paths.retain(|p| p != path);
    }

    /// Add a blocked pattern
    pub fn block_pattern(&mut self, pattern: String) {
        if !self.config.blocked_patterns.contains(&pattern) {
            self.config.blocked_patterns.push(pattern);
        }
    }
}

/// Result of sandbox validation
#[derive(Debug)]
pub struct SandboxResult {
    /// Whether the operation is allowed
    pub allowed: bool,
    /// Whether approval is required
    pub requires_approval: bool,
    /// Risk level of the operation
    pub risk_level: RiskLevel,
    /// Human-readable reason
    pub reason: String,
    /// The resolved (canonical) path
    pub resolved_path: PathBuf,
}

/// Access check result (for skills API)
#[derive(Debug)]
pub struct AccessCheck {
    /// Whether the operation is allowed
    pub allowed: bool,
    /// Whether approval is required
    pub requires_approval: bool,
    /// Risk level of the operation
    pub risk_level: RiskLevel,
    /// Human-readable reason
    pub reason: Option<String>,
}

impl Default for FileSystemSandbox {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_default_config() {
        let sandbox = FileSystemSandbox::new();
        assert!(!sandbox.config.allowed_paths.is_empty());
        assert!(!sandbox.config.blocked_patterns.is_empty());
    }

    #[test]
    fn test_blocked_paths() {
        let sandbox = FileSystemSandbox::new();

        // Test blocked files
        assert!(sandbox.is_blocked(Path::new("/etc/passwd")));
        assert!(sandbox.is_blocked(Path::new("/home/user/.ssh/id_rsa")));
        assert!(sandbox.is_blocked(Path::new("credentials.json")));
        assert!(sandbox.is_blocked(Path::new(".env")));
    }

    #[test]
    fn test_path_resolution() {
        let sandbox = FileSystemSandbox::new();

        // Test home expansion
        let resolved = sandbox.resolve_path("~/Documents").unwrap();
        assert!(resolved.to_string_lossy().contains("Documents"));
    }

    #[test]
    fn test_risk_levels() {
        assert_eq!(FileOperation::Read.risk_level(), RiskLevel::Low);
        assert_eq!(FileOperation::Write.risk_level(), RiskLevel::Medium);
        assert_eq!(FileOperation::Execute.risk_level(), RiskLevel::High);
        assert_eq!(FileOperation::Delete.risk_level(), RiskLevel::Critical);
    }

    #[test]
    fn test_blocked_directories() {
        let sandbox = FileSystemSandbox::new();

        // /root and everything under it should be blocked
        assert!(sandbox.is_blocked(Path::new("/root")));
        assert!(sandbox.is_blocked(Path::new("/root/secret.txt")));
        assert!(sandbox.is_blocked(Path::new("/root/.bashrc")));
        assert!(sandbox.is_blocked(Path::new("/root/subdir/file.txt")));

        // Other system paths should not be blocked for reads
        assert!(!sandbox.is_blocked(Path::new("/usr/bin/test")));
        assert!(!sandbox.is_blocked(Path::new("/home/user/file.txt")));
    }

    #[test]
    fn test_system_write_blocked() {
        let sandbox = FileSystemSandbox::new();

        // Writes to system directories should be blocked
        let result = sandbox.validate(Path::new("/usr/bin/test"), &FileOperation::Write).unwrap();
        assert!(!result.allowed);
        assert!(!result.requires_approval); // Hard-blocked, not approval-gated

        let result = sandbox.validate(Path::new("/sbin/init"), &FileOperation::Write).unwrap();
        assert!(!result.allowed);

        // Reads from system directories are fine (just outside allowed paths)
        let result = sandbox.validate(Path::new("/usr/bin/ls"), &FileOperation::Read).unwrap();
        // Not hard-blocked, but outside allowed paths so requires approval
        assert!(result.requires_approval);
    }
}
