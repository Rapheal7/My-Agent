//! Built-in web skill
//!
//! Provides web fetch and search operations with security restrictions.

use anyhow::{Result, bail};
use std::collections::HashMap;

use crate::tools::web::{WebTool, WebConfig, SearchResult};
use super::super::registry::{
    Skill, SkillMeta, SkillCategory, Permission, SkillParameter, ParameterType,
    SkillResult, SkillContext,
};

/// Create the web skill
pub fn create_skill() -> Skill {
    let meta = SkillMeta {
        id: "builtin-web".to_string(),
        name: "Web".to_string(),
        description: "Web fetch and search operations with security restrictions".to_string(),
        version: "1.0.0".to_string(),
        author: Some("my-agent".to_string()),
        category: SkillCategory::Web,
        permissions: vec![Permission::NetworkAccess],
        parameters: vec![
            SkillParameter {
                name: "operation".to_string(),
                param_type: ParameterType::Enum,
                required: true,
                default: None,
                description: "Operation to perform".to_string(),
                allowed_values: Some(vec![
                    "fetch".to_string(),
                    "fetch_text".to_string(),
                    "check".to_string(),
                    "search".to_string(),
                ]),
            },
            SkillParameter {
                name: "url".to_string(),
                param_type: ParameterType::Url,
                required: false,
                default: None,
                description: "Target URL (for fetch/check operations)".to_string(),
                allowed_values: None,
            },
            SkillParameter {
                name: "query".to_string(),
                param_type: ParameterType::String,
                required: false,
                default: None,
                description: "Search query (for search operation)".to_string(),
                allowed_values: None,
            },
            SkillParameter {
                name: "timeout".to_string(),
                param_type: ParameterType::Integer,
                required: false,
                default: Some("30".to_string()),
                description: "Request timeout in seconds".to_string(),
                allowed_values: None,
            },
            SkillParameter {
                name: "max_size".to_string(),
                param_type: ParameterType::Integer,
                required: false,
                default: Some("10485760".to_string()), // 10 MB
                description: "Maximum content size in bytes".to_string(),
                allowed_values: None,
            },
        ],
        builtin: true,
        tags: vec!["web".to_string(), "http".to_string(), "fetch".to_string(), "search".to_string()],
    };

    Skill::new(meta, execute_web)
}

/// Execute web operations
fn execute_web(
    params: HashMap<String, String>,
    ctx: &SkillContext,
) -> Result<SkillResult> {
    let operation = params.get("operation")
        .ok_or_else(|| anyhow::anyhow!("Missing 'operation' parameter"))?;

    // Build config from parameters
    let mut config = WebConfig::default();

    if let Some(timeout_str) = params.get("timeout") {
        if let Ok(timeout_secs) = timeout_str.parse::<u64>() {
            config.timeout = std::time::Duration::from_secs(timeout_secs);
        }
    }

    if let Some(max_size_str) = params.get("max_size") {
        if let Ok(max_size) = max_size_str.parse::<usize>() {
            config.max_content_size = max_size;
        }
    }

    // Create web tool with config
    let tool = WebTool::with_config(config)
        .map_err(|e| anyhow::anyhow!("Failed to create web tool: {}", e))?;

    // Check if approval is required
    if ctx.require_approval && operation != "check" {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Web '{}' operations require approval", operation)),
            duration_ms: 0,
        });
    }

    match operation.as_str() {
        "fetch" => {
            let url = params.get("url")
                .ok_or_else(|| anyhow::anyhow!("Missing 'url' parameter for fetch"))?;
            fetch_url(&tool, url, ctx)
        }
        "fetch_text" => {
            let url = params.get("url")
                .ok_or_else(|| anyhow::anyhow!("Missing 'url' parameter for fetch_text"))?;
            fetch_text_only(&tool, url, ctx)
        }
        "check" => {
            let url = params.get("url")
                .ok_or_else(|| anyhow::anyhow!("Missing 'url' parameter for check"))?;
            check_url(&tool, url, ctx)
        }
        "search" => {
            let query = params.get("query")
                .ok_or_else(|| anyhow::anyhow!("Missing 'query' parameter for search"))?;
            search_web(&tool, query, ctx)
        }
        _ => bail!("Unknown operation: {}", operation),
    }
}

/// Fetch URL and return full result
fn fetch_url(tool: &WebTool, url: &str, _ctx: &SkillContext) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            // We're in an async context, use block_on
            let result = handle.block_on(async {
                tool.fetch(url).await
            });

            let duration_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(web_result) => {
                    let output = format_web_result(&web_result);
                    Ok(SkillResult {
                        success: true,
                        output,
                        error: None,
                        duration_ms,
                    })
                }
                Err(e) => {
                    Ok(SkillResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Fetch failed: {}", e)),
                        duration_ms,
                    })
                }
            }
        }
        Err(_) => {
            // No runtime available, create one
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| anyhow::anyhow!("Failed to create runtime: {}", e))?;

            let result = rt.block_on(async {
                tool.fetch(url).await
            });

            let duration_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(web_result) => {
                    let output = format_web_result(&web_result);
                    Ok(SkillResult {
                        success: true,
                        output,
                        error: None,
                        duration_ms,
                    })
                }
                Err(e) => {
                    Ok(SkillResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Fetch failed: {}", e)),
                        duration_ms,
                    })
                }
            }
        }
    }
}

/// Fetch URL and return text only
fn fetch_text_only(tool: &WebTool, url: &str, _ctx: &SkillContext) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    let result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.block_on(async { tool.fetch_text(url).await })
    } else {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!("Failed to create runtime: {}", e))?;
        rt.block_on(async { tool.fetch_text(url).await })
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(text) => {
            Ok(SkillResult {
                success: true,
                output: text,
                error: None,
                duration_ms,
            })
        }
        Err(e) => {
            Ok(SkillResult {
                success: false,
                output: String::new(),
                error: Some(format!("Fetch failed: {}", e)),
                duration_ms,
            })
        }
    }
}

/// Check if URL is accessible
fn check_url(tool: &WebTool, url: &str, _ctx: &SkillContext) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    let result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.block_on(async { tool.check_url(url).await })
    } else {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!("Failed to create runtime: {}", e))?;
        rt.block_on(async { tool.check_url(url).await })
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(status_code) => {
            let status_text = match status_code {
                200 => "OK",
                301 => "Moved Permanently",
                302 => "Found (Redirect)",
                400 => "Bad Request",
                401 => "Unauthorized",
                403 => "Forbidden",
                404 => "Not Found",
                500 => "Internal Server Error",
                502 => "Bad Gateway",
                503 => "Service Unavailable",
                _ => "Unknown",
            };

            Ok(SkillResult {
                success: status_code < 400,
                output: format!("Status: {} ({})", status_code, status_text),
                error: if status_code >= 400 {
                    Some(format!("HTTP error: {}", status_code))
                } else {
                    None
                },
                duration_ms,
            })
        }
        Err(e) => {
            Ok(SkillResult {
                success: false,
                output: String::new(),
                error: Some(format!("Check failed: {}", e)),
                duration_ms,
            })
        }
    }
}

/// Search the web
fn search_web(tool: &WebTool, query: &str, _ctx: &SkillContext) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    let result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.block_on(async { tool.search(query).await })
    } else {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!("Failed to create runtime: {}", e))?;
        rt.block_on(async { tool.search(query).await })
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(results) => {
            let output = format_search_results(&results);
            Ok(SkillResult {
                success: true,
                output,
                error: None,
                duration_ms,
            })
        }
        Err(e) => {
            Ok(SkillResult {
                success: false,
                output: String::new(),
                error: Some(format!("Search failed: {}", e)),
                duration_ms,
            })
        }
    }
}

/// Format web fetch result as readable text
fn format_web_result(result: &crate::tools::web::WebResult) -> String {
    let mut lines = Vec::new();

    lines.push(format!("URL: {}", result.url));
    lines.push(format!("Status: {}", result.status_code));

    if let Some(ref ct) = result.content_type {
        lines.push(format!("Content-Type: {}", ct));
    }

    if let Some(len) = result.content_length {
        lines.push(format!("Content-Length: {} bytes", len));
    }

    if result.truncated {
        lines.push("Note: Content was truncated due to size limit".to_string());
    }

    lines.push(format!("Duration: {} ms", result.duration_ms));
    lines.push(String::new());
    lines.push("--- Body ---".to_string());
    lines.push(result.body.clone());

    lines.join("\n")
}

/// Format search results as readable text
fn format_search_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No results found".to_string();
    }

    let mut lines = Vec::new();
    lines.push(format!("Found {} results:\n", results.len()));

    for (i, result) in results.iter().enumerate() {
        lines.push(format!("{}. {}", i + 1, result.title));
        lines.push(format!("   URL: {}", result.url));
        lines.push(format!("   {}", result.snippet));
        lines.push(String::new());
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_skill() {
        let skill = create_skill();
        assert_eq!(skill.meta.id, "builtin-web");
        assert_eq!(skill.meta.category, SkillCategory::Web);
    }

    #[test]
    fn test_check_operation_localhost_blocked() {
        let skill = create_skill();
        let ctx = SkillContext {
            require_approval: false,
            ..Default::default()
        };

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "check".to_string());
        params.insert("url".to_string(), "http://localhost:8080".to_string());

        let result = skill.execute(params, &ctx).unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("blocked") ||
                result.error.as_ref().unwrap().contains("localhost"));
    }

    #[test]
    fn test_check_operation_internal_ip_blocked() {
        let skill = create_skill();
        let ctx = SkillContext {
            require_approval: false,
            ..Default::default()
        };

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "check".to_string());
        params.insert("url".to_string(), "http://192.168.1.1".to_string());

        let result = skill.execute(params, &ctx).unwrap();
        assert!(!result.success);
    }

    #[test]
    fn test_fetch_requires_url() {
        let skill = create_skill();
        let ctx = SkillContext::default();

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "fetch".to_string());
        // Missing URL

        let result = skill.execute(params, &ctx);
        assert!(result.is_err() || !result.unwrap().success);
    }

    #[test]
    fn test_search_requires_query() {
        let skill = create_skill();
        let ctx = SkillContext::default();

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "search".to_string());
        // Missing query

        let result = skill.execute(params, &ctx);
        assert!(result.is_err() || !result.unwrap().success);
    }

    #[test]
    fn test_unknown_operation() {
        let skill = create_skill();
        // Use require_approval: false so we reach the operation match
        let ctx = SkillContext {
            require_approval: false,
            ..Default::default()
        };

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "invalid_op".to_string());

        // The execute function returns an error for unknown operations
        let result = skill.execute(params, &ctx);
        // Either an error is returned, or a SkillResult with success: false
        match result {
            Err(e) => assert!(e.to_string().contains("Unknown")),
            Ok(skill_result) => {
                assert!(!skill_result.success);
                assert!(skill_result.error.unwrap().contains("Unknown"));
            }
        }
    }

    #[test]
    fn test_requires_approval() {
        let skill = create_skill();
        let ctx = SkillContext {
            require_approval: true,
            ..Default::default()
        };

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "fetch".to_string());
        params.insert("url".to_string(), "https://example.com".to_string());

        let result = skill.execute(params, &ctx).unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("approval"));
    }
}
