//! Safe file operations with sandbox integration
//!
//! This module provides filesystem operations that are restricted by the
//! FileSystemSandbox. All operations require sandbox validation before execution.

use crate::security::{
    ApprovalManager, ApprovalDecision,
    sandbox::{FileSystemSandbox, FileOperation, SandboxResult},
    approval::{ActionType, Action},
};
use anyhow::{Result, Context, bail};
use std::path::{Path, PathBuf};
use std::fs;
use std::io::Write;

/// Maximum file size for reading (10 MB)
const MAX_READ_SIZE: usize = 10 * 1024 * 1024;

/// Maximum file size for writing (50 MB)
const MAX_WRITE_SIZE: usize = 50 * 1024 * 1024;

/// File content with metadata
#[derive(Debug, Clone)]
pub struct FileContent {
    pub path: PathBuf,
    pub content: String,
    pub size: usize,
    pub lines: usize,
}

/// File metadata
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub is_file: bool,
    pub modified: Option<std::time::SystemTime>,
    pub created: Option<std::time::SystemTime>,
}

/// Directory listing
#[derive(Debug, Clone)]
pub struct DirectoryListing {
    pub path: PathBuf,
    pub entries: Vec<FileInfo>,
    pub total_count: usize,
}

/// File operation result
#[derive(Debug, Clone)]
pub enum FileOperationResult {
    Success { message: String },
    Cancelled { reason: String },
    Error { message: String },
}

/// Safe filesystem operations handler
#[derive(Clone)]
pub struct FileSystemTool {
    sandbox: FileSystemSandbox,
    approver: ApprovalManager,
}

impl FileSystemTool {
    /// Create a new filesystem tool with default sandbox
    pub fn new() -> Self {
        Self {
            sandbox: FileSystemSandbox::new(),
            approver: ApprovalManager::with_defaults(),
        }
    }

    /// Create with custom sandbox
    pub fn with_sandbox(sandbox: FileSystemSandbox) -> Self {
        Self {
            sandbox,
            approver: ApprovalManager::with_defaults(),
        }
    }

    /// Create with custom sandbox and approver
    pub fn with_config(sandbox: FileSystemSandbox, approver: ApprovalManager) -> Self {
        Self {
            sandbox,
            approver,
        }
    }

    /// Set a custom approval manager (e.g., for voice mode auto-approval)
    pub fn set_approver(mut self, approver: ApprovalManager) -> Self {
        self.approver = approver;
        self
    }

    /// Get the sandbox reference
    pub fn sandbox(&self) -> &FileSystemSandbox {
        &self.sandbox
    }

    /// Read a file's contents
    ///
    /// # Security
    /// - Validates path against sandbox
    /// - Checks file size limits
    /// - Requires approval for files outside allowed directories
    pub async fn read_file(&self, path: impl AsRef<Path>) -> Result<FileContent> {
        let path = path.as_ref();

        // Validate with sandbox
        let validation = self.sandbox.validate(path, &FileOperation::Read)
            .context("Sandbox validation failed")?;

        // Check if approval is needed
        if validation.requires_approval {
            let action = Action {
                id: uuid::Uuid::new_v4().to_string(),
                action_type: ActionType::FileRead,
                description: format!("Read file: {}", path.display()),
                risk_level: crate::security::approval::RiskLevel::Low,
                target: path.display().to_string(),
                details: [
                    ("resolved_path".to_string(), validation.resolved_path.display().to_string()),
                    ("reason".to_string(), validation.reason.clone()),
                ].into_iter().collect(),
                requested_at: chrono::Utc::now(),
            };

            match self.approver.request_approval(action)? {
                ApprovalDecision::Approved | ApprovalDecision::ApprovedForSession => {
                    // Continue with operation
                }
                ApprovalDecision::Denied => {
                    bail!("File read denied by user");
                }
            }
        }

        // Check if blocked
        if !validation.allowed && !validation.requires_approval {
            bail!("Access denied: {}", validation.reason);
        }

        let resolved_path = &validation.resolved_path;

        // Check if file exists
        if !resolved_path.exists() {
            bail!("File not found: {}", path.display());
        }

        // Check if it's a file
        if !resolved_path.is_file() {
            bail!("Path is not a file: {}", path.display());
        }

        // Check file size
        let metadata = fs::metadata(resolved_path)
            .context("Failed to read file metadata")?;
        let size = metadata.len() as usize;

        if size > MAX_READ_SIZE {
            bail!(
                "File too large ({} bytes, max {} bytes). Use read_file_chunked for large files.",
                size,
                MAX_READ_SIZE
            );
        }

        // Read content
        let content = fs::read_to_string(resolved_path)
            .context("Failed to read file content")?;

        let lines = content.lines().count();

        tracing::info!(
            path = %path.display(),
            size = size,
            lines = lines,
            "File read successfully"
        );

        Ok(FileContent {
            path: resolved_path.clone(),
            content,
            size,
            lines,
        })
    }

    /// Read a portion of a file
    pub async fn read_file_chunked(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
        length: usize,
    ) -> Result<FileContent> {
        let path = path.as_ref();

        // Validate with sandbox
        let validation = self.sandbox.validate(path, &FileOperation::Read)
            .context("Sandbox validation failed")?;

        if !validation.allowed && !validation.requires_approval {
            bail!("Access denied: {}", validation.reason);
        }

        let resolved_path = &validation.resolved_path;

        if !resolved_path.exists() || !resolved_path.is_file() {
            bail!("File not found: {}", path.display());
        }

        // Read specific chunk
        use std::io::{BufRead, BufReader};
        let file = fs::File::open(resolved_path)
            .context("Failed to open file")?;
        let reader = BufReader::new(file);

        let mut content = String::new();
        let mut current_offset = 0;

        for line in reader.lines() {
            let line = line?;
            let line_len = line.len() + 1; // +1 for newline

            if current_offset >= offset && content.len() < length {
                content.push_str(&line);
                content.push('\n');
            }

            current_offset += line_len;

            if content.len() >= length {
                break;
            }
        }

        let lines = content.lines().count();
        let size = content.len();

        Ok(FileContent {
            path: resolved_path.clone(),
            content,
            size,
            lines,
        })
    }

    /// Write content to a file
    ///
    /// # Security
    /// - Requires approval for write operations (Medium risk)
    /// - Validates path against sandbox
    /// - Creates parent directories if needed
    /// - Shows diff preview for approval
    pub async fn write_file(
        &self,
        path: impl AsRef<Path>,
        content: impl AsRef<str>,
    ) -> Result<FileOperationResult> {
        let path = path.as_ref();
        let content = content.as_ref();

        // Validate with sandbox
        let validation = self.sandbox.validate(path, &FileOperation::Write)
            .context("Sandbox validation failed")?;

        // Check if hard-blocked before showing any approval dialog
        if !validation.allowed && !validation.requires_approval {
            return Ok(FileOperationResult::Error {
                message: format!("Access denied: {}", validation.reason),
            });
        }

        // Read existing content for diff preview
        let original_content = if validation.resolved_path.exists() {
            fs::read_to_string(&validation.resolved_path).unwrap_or_default()
        } else {
            String::new()
        };

        // New file creation is Low risk (auto-approved), editing existing files is Medium risk
        let is_new_file = !validation.resolved_path.exists();
        let (risk_level, description) = if is_new_file {
            (
                crate::security::approval::RiskLevel::Low,
                format!("Create new file: {} ({} lines)", path.display(), content.lines().count()),
            )
        } else {
            (
                crate::security::approval::RiskLevel::Medium,
                format!("Edit file: {} ({} -> {} lines)",
                    path.display(),
                    original_content.lines().count(),
                    content.lines().count()),
            )
        };

        let action = Action {
            id: uuid::Uuid::new_v4().to_string(),
            action_type: ActionType::FileWrite,
            description,
            risk_level,
            target: path.display().to_string(),
            details: [
                ("resolved_path".to_string(), validation.resolved_path.display().to_string()),
                ("content_size".to_string(), content.len().to_string()),
                ("will_create".to_string(), is_new_file.to_string()),
                ("original_lines".to_string(), original_content.lines().count().to_string()),
                ("new_lines".to_string(), content.lines().count().to_string()),
            ].into_iter().collect(),
            requested_at: chrono::Utc::now(),
        };

        // Use diff preview for approval (auto-approved for new files since Low risk)
        let decision = self.approver.request_approval_with_diff(
            action,
            &original_content,
            content
        )?;

        match decision {
            ApprovalDecision::Approved | ApprovalDecision::ApprovedForSession => {
                // Continue with operation
            }
            ApprovalDecision::Denied => {
                return Ok(FileOperationResult::Cancelled {
                    reason: "File edit cancelled by user".to_string(),
                });
            }
        }

        let resolved_path = &validation.resolved_path;

        // Check content size
        if content.len() > MAX_WRITE_SIZE {
            bail!(
                "Content too large ({} bytes, max {} bytes)",
                content.len(),
                MAX_WRITE_SIZE
            );
        }

        // Create parent directories if needed
        if let Some(parent) = resolved_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)
                    .context("Failed to create parent directories")?;
            }
        }

        // Write file
        let mut file = fs::File::create(resolved_path)
            .context("Failed to create file")?;
        file.write_all(content.as_bytes())
            .context("Failed to write file content")?;

        tracing::info!(
            path = %path.display(),
            size = content.len(),
            "File written successfully"
        );

        Ok(FileOperationResult::Success {
            message: format!("Wrote {} bytes to {}", content.len(), path.display()),
        })
    }

    /// Append content to a file
    pub async fn append_file(
        &self,
        path: impl AsRef<Path>,
        content: impl AsRef<str>,
    ) -> Result<FileOperationResult> {
        let path = path.as_ref();
        let content = content.as_ref();

        // Validate with sandbox
        let validation = self.sandbox.validate(path, &FileOperation::Write)
            .context("Sandbox validation failed")?;

        if !validation.allowed && !validation.requires_approval {
            bail!("Access denied: {}", validation.reason);
        }

        let action = Action {
            id: uuid::Uuid::new_v4().to_string(),
            action_type: ActionType::FileWrite,
            description: format!("Append {} bytes to file: {}", content.len(), path.display()),
            risk_level: crate::security::approval::RiskLevel::Medium,
            target: path.display().to_string(),
            details: Default::default(),
            requested_at: chrono::Utc::now(),
        };

        match self.approver.request_approval(action)? {
            ApprovalDecision::Approved | ApprovalDecision::ApprovedForSession => {}
            ApprovalDecision::Denied => {
                return Ok(FileOperationResult::Cancelled {
                    reason: "File append denied by user".to_string(),
                });
            }
        }

        let resolved_path = &validation.resolved_path;

        // Append to file
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(resolved_path)
            .context("Failed to open file for appending")?;

        file.write_all(content.as_bytes())
            .context("Failed to append to file")?;

        tracing::info!(
            path = %path.display(),
            appended = content.len(),
            "File appended successfully"
        );

        Ok(FileOperationResult::Success {
            message: format!("Appended {} bytes to {}", content.len(), path.display()),
        })
    }

    /// Delete a file
    ///
    /// # Security
    /// - Requires approval (Critical risk)
    /// - Validates path against sandbox
    pub async fn delete_file(&self, path: impl AsRef<Path>) -> Result<FileOperationResult> {
        let path = path.as_ref();

        // Validate with sandbox
        let validation = self.sandbox.validate(path, &FileOperation::Delete)
            .context("Sandbox validation failed")?;

        // Check if hard-blocked before showing any approval dialog
        if !validation.allowed && !validation.requires_approval {
            return Ok(FileOperationResult::Error {
                message: format!("Access denied: {}", validation.reason),
            });
        }

        // Delete operations always require approval (Critical risk)
        let action = Action {
            id: uuid::Uuid::new_v4().to_string(),
            action_type: ActionType::FileDelete,
            description: format!("Delete file: {}", path.display()),
            risk_level: crate::security::approval::RiskLevel::Critical,
            target: path.display().to_string(),
            details: [
                ("resolved_path".to_string(), validation.resolved_path.display().to_string()),
            ].into_iter().collect(),
            requested_at: chrono::Utc::now(),
        };

        match self.approver.request_approval(action)? {
            ApprovalDecision::Approved | ApprovalDecision::ApprovedForSession => {
                // Continue with operation
            }
            ApprovalDecision::Denied => {
                return Ok(FileOperationResult::Cancelled {
                    reason: "File deletion denied by user".to_string(),
                });
            }
        }

        let resolved_path = &validation.resolved_path;

        if !resolved_path.exists() {
            return Ok(FileOperationResult::Error {
                message: format!("File does not exist: {}", path.display()),
            });
        }

        // Safety check: only delete files, not directories
        if resolved_path.is_dir() {
            bail!("Path is a directory, use delete_directory instead: {}", path.display());
        }

        fs::remove_file(resolved_path)
            .context("Failed to delete file")?;

        tracing::info!(
            path = %path.display(),
            "File deleted successfully"
        );

        Ok(FileOperationResult::Success {
            message: format!("Deleted file: {}", path.display()),
        })
    }

    /// List directory contents
    ///
    /// # Security
    /// - Validates path against sandbox
    /// - Low risk operation (usually auto-approved)
    pub async fn list_directory(&self, path: impl AsRef<Path>) -> Result<DirectoryListing> {
        let path = path.as_ref();

        // Validate with sandbox
        let validation = self.sandbox.validate(path, &FileOperation::List)
            .context("Sandbox validation failed")?;

        if !validation.allowed {
            bail!("Access denied: {}", validation.reason);
        }

        let resolved_path = &validation.resolved_path;

        if !resolved_path.exists() {
            bail!("Directory not found: {}", path.display());
        }

        if !resolved_path.is_dir() {
            bail!("Path is not a directory: {}", path.display());
        }

        let mut entries = Vec::new();
        let dir_entries = fs::read_dir(resolved_path)
            .context("Failed to read directory")?;

        for entry in dir_entries {
            let entry = entry?;
            let metadata = entry.metadata()?;

            entries.push(FileInfo {
                path: entry.path(),
                name: entry.file_name().to_string_lossy().to_string(),
                size: metadata.len(),
                is_dir: metadata.is_dir(),
                is_file: metadata.is_file(),
                modified: metadata.modified().ok(),
                created: metadata.created().ok(),
            });
        }

        // Sort: directories first, then files alphabetically
        entries.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });

        let total_count = entries.len();

        tracing::debug!(
            path = %path.display(),
            entries = total_count,
            "Directory listed successfully"
        );

        Ok(DirectoryListing {
            path: resolved_path.clone(),
            entries,
            total_count,
        })
    }

    /// Create a directory
    pub async fn create_directory(&self, path: impl AsRef<Path>) -> Result<FileOperationResult> {
        let path = path.as_ref();

        // Validate with sandbox - treat as write operation
        let validation = self.sandbox.validate(path, &FileOperation::Write)
            .context("Sandbox validation failed")?;

        if !validation.allowed && !validation.requires_approval {
            bail!("Access denied: {}", validation.reason);
        }

        let action = Action {
            id: uuid::Uuid::new_v4().to_string(),
            action_type: ActionType::FileWrite,
            description: format!("Create directory: {}", path.display()),
            risk_level: crate::security::approval::RiskLevel::Medium,
            target: path.display().to_string(),
            details: Default::default(),
            requested_at: chrono::Utc::now(),
        };

        match self.approver.request_approval(action)? {
            ApprovalDecision::Approved | ApprovalDecision::ApprovedForSession => {}
            ApprovalDecision::Denied => {
                return Ok(FileOperationResult::Cancelled {
                    reason: "Directory creation denied by user".to_string(),
                });
            }
        }

        let resolved_path = &validation.resolved_path;

        if resolved_path.exists() {
            return Ok(FileOperationResult::Error {
                message: format!("Path already exists: {}", path.display()),
            });
        }

        fs::create_dir_all(resolved_path)
            .context("Failed to create directory")?;

        tracing::info!(
            path = %path.display(),
            "Directory created successfully"
        );

        Ok(FileOperationResult::Success {
            message: format!("Created directory: {}", path.display()),
        })
    }

    /// Get file information
    pub async fn file_info(&self, path: impl AsRef<Path>) -> Result<FileInfo> {
        let path = path.as_ref();

        // Validate with sandbox
        let validation = self.sandbox.validate(path, &FileOperation::Read)
            .context("Sandbox validation failed")?;

        if !validation.allowed {
            bail!("Access denied: {}", validation.reason);
        }

        let resolved_path = &validation.resolved_path;

        if !resolved_path.exists() {
            bail!("Path not found: {}", path.display());
        }

        let metadata = fs::metadata(resolved_path)
            .context("Failed to read metadata")?;

        Ok(FileInfo {
            path: resolved_path.clone(),
            name: resolved_path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            size: metadata.len(),
            is_dir: metadata.is_dir(),
            is_file: metadata.is_file(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
        })
    }

    /// Search for files matching a pattern
    pub async fn search_files(
        &self,
        directory: impl AsRef<Path>,
        pattern: &str,
    ) -> Result<Vec<FileInfo>> {
        let directory = directory.as_ref();

        // Validate directory
        let validation = self.sandbox.validate(directory, &FileOperation::List)
            .context("Sandbox validation failed")?;

        if !validation.allowed {
            bail!("Access denied: {}", validation.reason);
        }

        let resolved_dir = &validation.resolved_path;

        if !resolved_dir.is_dir() {
            bail!("Path is not a directory: {}", directory.display());
        }

        let pattern_lower = pattern.to_lowercase();
        let mut results = Vec::new();

        self.search_recursive(resolved_dir, &pattern_lower, &mut results)?;

        Ok(results)
    }

    fn search_recursive(&self, dir: &Path, pattern: &str, results: &mut Vec<FileInfo>) -> Result<()> {
        let entries = fs::read_dir(dir)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_lowercase();

            // Check if name matches pattern
            if name.contains(pattern) {
                let metadata = entry.metadata()?;
                results.push(FileInfo {
                    path: path.clone(),
                    name: entry.file_name().to_string_lossy().to_string(),
                    size: metadata.len(),
                    is_dir: metadata.is_dir(),
                    is_file: metadata.is_file(),
                    modified: metadata.modified().ok(),
                    created: metadata.created().ok(),
                });
            }

            // Recurse into subdirectories
            if path.is_dir() {
                // Validate subdirectory
                if let Ok(validation) = self.sandbox.validate(&path, &FileOperation::List) {
                    if validation.allowed {
                        let _ = self.search_recursive(&path, pattern, results);
                    }
                }
            }
        }

        Ok(())
    }
}

impl Default for FileSystemTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience functions for one-off operations

/// Read a file with default sandbox
pub async fn read_file(path: impl AsRef<Path>) -> Result<String> {
    let tool = FileSystemTool::new();
    let content = tool.read_file(path).await?;
    Ok(content.content)
}

/// Read a file with approval (for use in agent context)
pub async fn read_file_with_approval(
    path: impl AsRef<Path>,
    approver: &ApprovalManager,
) -> Result<String> {
    let sandbox = FileSystemSandbox::new();
    let tool = FileSystemTool::with_config(sandbox, approver.clone());
    let content = tool.read_file(path).await?;
    Ok(content.content)
}

/// Write a file with default sandbox
pub async fn write_file(path: impl AsRef<Path>, content: impl AsRef<str>) -> Result<()> {
    let tool = FileSystemTool::new();
    match tool.write_file(path, content).await? {
        FileOperationResult::Success { .. } => Ok(()),
        FileOperationResult::Cancelled { reason } => bail!("Cancelled: {}", reason),
        FileOperationResult::Error { message } => bail!("Error: {}", message),
    }
}

/// List directory contents
pub async fn list_directory(path: impl AsRef<Path>) -> Result<Vec<FileInfo>> {
    let tool = FileSystemTool::new();
    let listing = tool.list_directory(path).await?;
    Ok(listing.entries)
}

/// Check if a path is accessible
pub fn is_path_accessible(path: impl AsRef<Path>) -> bool {
    let sandbox = FileSystemSandbox::new();
    let path = path.as_ref();
    sandbox.can_proceed(path, &FileOperation::Read).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_read_write_file() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.txt");

        // Create sandbox that allows temp directory
        let mut sandbox = FileSystemSandbox::new();
        sandbox.allow_path(temp_dir.path().to_path_buf());

        // Create approver that auto-approves all actions for testing
        let approver_config = crate::security::ApprovalConfig {
            approval_threshold: crate::security::RiskLevel::Critical,
            auto_approve_low_risk: true,
            session_duration_minutes: 60,
            enable_audit_log: false,
        };
        let approver = crate::security::ApprovalManager::new(approver_config);

        let tool = FileSystemTool::with_config(sandbox, approver);

        // Write file
        let result = tool.write_file(&test_file, "Hello, World!").await;
        assert!(result.is_ok());

        // Read file
        let content = tool.read_file(&test_file).await.unwrap();
        assert_eq!(content.content, "Hello, World!");
        assert_eq!(content.lines, 1);
    }

    #[tokio::test]
    async fn test_list_directory() {
        let temp_dir = TempDir::new().unwrap();

        // Create some files
        fs::write(temp_dir.path().join("file1.txt"), "content1").unwrap();
        fs::write(temp_dir.path().join("file2.txt"), "content2").unwrap();
        fs::create_dir(temp_dir.path().join("subdir")).unwrap();

        let mut sandbox = FileSystemSandbox::new();
        sandbox.allow_path(temp_dir.path().to_path_buf());

        // Create approver that auto-approves all actions for testing
        let approver_config = crate::security::ApprovalConfig {
            approval_threshold: crate::security::RiskLevel::Critical,
            auto_approve_low_risk: true,
            session_duration_minutes: 60,
            enable_audit_log: false,
        };
        let approver = crate::security::ApprovalManager::new(approver_config);

        let tool = FileSystemTool::with_config(sandbox, approver);
        let listing = tool.list_directory(temp_dir.path()).await.unwrap();

        assert_eq!(listing.total_count, 3);
        assert!(listing.entries.iter().any(|e| e.name == "file1.txt"));
        assert!(listing.entries.iter().any(|e| e.name == "file2.txt"));
        assert!(listing.entries.iter().any(|e| e.name == "subdir" && e.is_dir));
    }

    #[tokio::test]
    async fn test_blocked_path() {
        // Create approver that auto-approves all actions for testing
        let approver_config = crate::security::ApprovalConfig {
            approval_threshold: crate::security::RiskLevel::Critical,
            auto_approve_low_risk: true,
            session_duration_minutes: 60,
            enable_audit_log: false,
        };
        let approver = crate::security::ApprovalManager::new(approver_config);
        let sandbox = FileSystemSandbox::new();
        let tool = FileSystemTool::with_config(sandbox, approver);

        // Try to read a blocked file
        let result = tool.read_file("/etc/passwd").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_search_files() {
        let temp_dir = TempDir::new().unwrap();

        // Create files with different names
        fs::write(temp_dir.path().join("test_file.txt"), "content").unwrap();
        fs::write(temp_dir.path().join("another.txt"), "content").unwrap();
        fs::write(temp_dir.path().join("test_other.rs"), "content").unwrap();

        let mut sandbox = FileSystemSandbox::new();
        sandbox.allow_path(temp_dir.path().to_path_buf());

        // Create approver that auto-approves all actions for testing
        let approver_config = crate::security::ApprovalConfig {
            approval_threshold: crate::security::RiskLevel::Critical,
            auto_approve_low_risk: true,
            session_duration_minutes: 60,
            enable_audit_log: false,
        };
        let approver = crate::security::ApprovalManager::new(approver_config);

        let tool = FileSystemTool::with_config(sandbox, approver);
        let results = tool.search_files(temp_dir.path(), "test").await.unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|f| f.name == "test_file.txt"));
        assert!(results.iter().any(|f| f.name == "test_other.rs"));
    }
}
