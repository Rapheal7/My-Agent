//! Built-in filesystem skill
//!
//! Provides file and directory operations with sandbox restrictions.

use anyhow::{Result, bail};
use std::collections::HashMap;
use std::path::Path;

use crate::security::sandbox::{FileSystemSandbox, FileOperation};
use super::super::registry::{
    Skill, SkillMeta, SkillCategory, Permission, SkillParameter, ParameterType,
    SkillResult, SkillContext,
};

/// Create the filesystem skill
pub fn create_skill() -> Skill {
    let meta = SkillMeta {
        id: "builtin-filesystem".to_string(),
        name: "Filesystem".to_string(),
        description: "File and directory operations with sandbox restrictions".to_string(),
        version: "1.0.0".to_string(),
        author: Some("my-agent".to_string()),
        category: SkillCategory::Filesystem,
        permissions: vec![Permission::ReadFiles, Permission::WriteFiles],
        parameters: vec![
            SkillParameter {
                name: "operation".to_string(),
                param_type: ParameterType::Enum,
                required: true,
                default: None,
                description: "Operation to perform".to_string(),
                allowed_values: Some(vec![
                    "read".to_string(),
                    "write".to_string(),
                    "list".to_string(),
                    "delete".to_string(),
                    "mkdir".to_string(),
                    "exists".to_string(),
                    "copy".to_string(),
                    "move".to_string(),
                ]),
            },
            SkillParameter {
                name: "path".to_string(),
                param_type: ParameterType::Path,
                required: true,
                default: None,
                description: "Target file or directory path".to_string(),
                allowed_values: None,
            },
            SkillParameter {
                name: "content".to_string(),
                param_type: ParameterType::String,
                required: false,
                default: None,
                description: "Content to write (for write operation)".to_string(),
                allowed_values: None,
            },
            SkillParameter {
                name: "destination".to_string(),
                param_type: ParameterType::Path,
                required: false,
                default: None,
                description: "Destination path (for copy/move)".to_string(),
                allowed_values: None,
            },
        ],
        builtin: true,
        tags: vec!["file".to_string(), "filesystem".to_string(), "io".to_string()],
    };

    Skill::new(meta, execute_filesystem)
}

/// Execute filesystem operations
fn execute_filesystem(
    params: HashMap<String, String>,
    ctx: &SkillContext,
) -> Result<SkillResult> {
    let sandbox = FileSystemSandbox::new();

    let operation = params.get("operation")
        .ok_or_else(|| anyhow::anyhow!("Missing 'operation' parameter"))?;

    let path_str = params.get("path")
        .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?;

    let path = sandbox.resolve_path(path_str)?;

    match operation.as_str() {
        "read" => read_file(&sandbox, &path, ctx),
        "write" => {
            let content = params.get("content")
                .ok_or_else(|| anyhow::anyhow!("Missing 'content' parameter for write"))?;
            write_file(&sandbox, &path, content, ctx)
        }
        "list" => list_directory(&sandbox, &path, ctx),
        "delete" => delete_file(&sandbox, &path, ctx),
        "mkdir" => create_directory(&sandbox, &path, ctx),
        "exists" => check_exists(&sandbox, &path, ctx),
        "copy" => {
            let dest = params.get("destination")
                .ok_or_else(|| anyhow::anyhow!("Missing 'destination' parameter for copy"))?;
            let dest_path = sandbox.resolve_path(dest)?;
            copy_file(&sandbox, &path, &dest_path, ctx)
        }
        "move" => {
            let dest = params.get("destination")
                .ok_or_else(|| anyhow::anyhow!("Missing 'destination' parameter for move"))?;
            let dest_path = sandbox.resolve_path(dest)?;
            move_file(&sandbox, &path, &dest_path, ctx)
        }
        _ => bail!("Unknown operation: {}", operation),
    }
}

/// Read file contents
fn read_file(sandbox: &FileSystemSandbox, path: &Path, ctx: &SkillContext) -> Result<SkillResult> {
    // Check if operation is allowed
    let check = sandbox.validate(path, &FileOperation::Read)?;
    if !check.allowed {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Access denied: {}", check.reason)),
            duration_ms: 0,
        });
    }

    // Require approval for sensitive files
    if check.requires_approval && ctx.require_approval {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Requires approval: {}", check.reason)),
            duration_ms: 0,
        });
    }

    let content = std::fs::read_to_string(path)?;

    Ok(SkillResult {
        success: true,
        output: content,
        error: None,
        duration_ms: 0,
    })
}

/// Write content to file
fn write_file(sandbox: &FileSystemSandbox, path: &Path, content: &str, ctx: &SkillContext) -> Result<SkillResult> {
    let check = sandbox.validate(path, &FileOperation::Write)?;
    if !check.allowed {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Access denied: {}", check.reason)),
            duration_ms: 0,
        });
    }

    // Always require approval for write operations
    if ctx.require_approval {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some("Write operations require approval".to_string()),
            duration_ms: 0,
        });
    }

    // Create parent directories if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(path, content)?;

    Ok(SkillResult {
        success: true,
        output: format!("Successfully wrote {} bytes to {}", content.len(), path.display()),
        error: None,
        duration_ms: 0,
    })
}

/// List directory contents
fn list_directory(sandbox: &FileSystemSandbox, path: &Path, _ctx: &SkillContext) -> Result<SkillResult> {
    let check = sandbox.validate(path, &FileOperation::List)?;
    if !check.allowed {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Access denied: {}", check.reason)),
            duration_ms: 0,
        });
    }

    if !path.is_dir() {
        bail!("Path is not a directory: {}", path.display());
    }

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type()?.is_dir();
        entries.push(if is_dir {
            format!("{}/ (dir)", name)
        } else {
            name
        });
    }

    entries.sort();

    Ok(SkillResult {
        success: true,
        output: entries.join("\n"),
        error: None,
        duration_ms: 0,
    })
}

/// Delete a file
fn delete_file(sandbox: &FileSystemSandbox, path: &Path, ctx: &SkillContext) -> Result<SkillResult> {
    let check = sandbox.validate(path, &FileOperation::Delete)?;
    if !check.allowed {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Access denied: {}", check.reason)),
            duration_ms: 0,
        });
    }

    // Always require approval for delete
    if ctx.require_approval {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some("Delete operations require approval".to_string()),
            duration_ms: 0,
        });
    }

    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }

    Ok(SkillResult {
        success: true,
        output: format!("Successfully deleted {}", path.display()),
        error: None,
        duration_ms: 0,
    })
}

/// Create a directory
fn create_directory(sandbox: &FileSystemSandbox, path: &Path, ctx: &SkillContext) -> Result<SkillResult> {
    let check = sandbox.validate(path, &FileOperation::Write)?;
    if !check.allowed {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Access denied: {}", check.reason)),
            duration_ms: 0,
        });
    }

    if ctx.require_approval {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some("Directory creation requires approval".to_string()),
            duration_ms: 0,
        });
    }

    std::fs::create_dir_all(path)?;

    Ok(SkillResult {
        success: true,
        output: format!("Successfully created directory {}", path.display()),
        error: None,
        duration_ms: 0,
    })
}

/// Check if path exists
fn check_exists(sandbox: &FileSystemSandbox, path: &Path, _ctx: &SkillContext) -> Result<SkillResult> {
    let check = sandbox.check_access(path, &FileOperation::Read)?;
    if !check.allowed {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Access denied: {}", check.reason.unwrap_or_default())),
            duration_ms: 0,
        });
    }

    let exists = path.exists();
    let is_dir = path.is_dir();
    let is_file = path.is_file();

    let output = if exists {
        if is_dir {
            format!("Directory exists: {}", path.display())
        } else if is_file {
            format!("File exists: {}", path.display())
        } else {
            format!("Path exists: {}", path.display())
        }
    } else {
        format!("Path does not exist: {}", path.display())
    };

    Ok(SkillResult {
        success: true,
        output,
        error: None,
        duration_ms: 0,
    })
}

/// Copy a file
fn copy_file(sandbox: &FileSystemSandbox, src: &Path, dest: &Path, ctx: &SkillContext) -> Result<SkillResult> {
    let src_check = sandbox.check_access(src, &FileOperation::Read)?;
    let dest_check = sandbox.check_access(dest, &FileOperation::Write)?;

    if !src_check.allowed {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Source access denied: {}", src_check.reason.unwrap_or_default())),
            duration_ms: 0,
        });
    }

    if !dest_check.allowed {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Destination access denied: {}", dest_check.reason.unwrap_or_default())),
            duration_ms: 0,
        });
    }

    if ctx.require_approval {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some("Copy operations require approval".to_string()),
            duration_ms: 0,
        });
    }

    // Create parent directories if needed
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::copy(src, dest)?;

    Ok(SkillResult {
        success: true,
        output: format!("Successfully copied {} to {}", src.display(), dest.display()),
        error: None,
        duration_ms: 0,
    })
}

/// Move a file
fn move_file(sandbox: &FileSystemSandbox, src: &Path, dest: &Path, ctx: &SkillContext) -> Result<SkillResult> {
    let src_check = sandbox.check_access(src, &FileOperation::Delete)?;
    let dest_check = sandbox.check_access(dest, &FileOperation::Write)?;

    if !src_check.allowed {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Source access denied: {}", src_check.reason.unwrap_or_default())),
            duration_ms: 0,
        });
    }

    if !dest_check.allowed {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Destination access denied: {}", dest_check.reason.unwrap_or_default())),
            duration_ms: 0,
        });
    }

    if ctx.require_approval {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some("Move operations require approval".to_string()),
            duration_ms: 0,
        });
    }

    // Create parent directories if needed
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::rename(src, dest)?;

    Ok(SkillResult {
        success: true,
        output: format!("Successfully moved {} to {}", src.display(), dest.display()),
        error: None,
        duration_ms: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_skill() {
        let skill = create_skill();
        assert_eq!(skill.meta.id, "builtin-filesystem");
        assert_eq!(skill.meta.category, SkillCategory::Filesystem);
    }

    #[test]
    fn test_exists_operation() {
        let skill = create_skill();
        let ctx = SkillContext::default();

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "exists".to_string());
        params.insert("path".to_string(), "/tmp".to_string());

        let result = skill.execute(params, &ctx).unwrap();
        assert!(result.success);
        assert!(result.output.contains("exists"));
    }

    #[test]
    fn test_list_operation() {
        let skill = create_skill();
        let ctx = SkillContext::default();

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "list".to_string());
        params.insert("path".to_string(), "/tmp".to_string());

        let result = skill.execute(params, &ctx).unwrap();
        assert!(result.success);
    }
}
