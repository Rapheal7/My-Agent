//! Web browsing skill with semantic snapshots
//!
//! Provides:
//! - HTML to Markdown conversion
//! - Readability-based content extraction
//! - Semantic snapshots (concise summaries instead of full screenshots)
//! - Smart content parsing

use anyhow::{Result, bail};
use std::collections::HashMap;

use super::super::registry::{
    Skill, SkillMeta, SkillCategory, Permission, SkillParameter, ParameterType,
    SkillResult, SkillContext,
};

/// Create the web browsing skill
pub fn create_skill() -> Skill {
    let meta = SkillMeta {
        id: "builtin-web-browsing".to_string(),
        name: "Web Browsing".to_string(),
        description: "Browse web pages with content extraction and semantic snapshots".to_string(),
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
                    "browse".to_string(),
                    "extract".to_string(),
                    "summarize".to_string(),
                    "snapshot".to_string(),
                ]),
            },
            SkillParameter {
                name: "url".to_string(),
                param_type: ParameterType::Url,
                required: true,
                default: None,
                description: "Target URL to browse".to_string(),
                allowed_values: None,
            },
            SkillParameter {
                name: "max_length".to_string(),
                param_type: ParameterType::Integer,
                required: false,
                default: Some("50000".to_string()),
                description: "Maximum content length in characters".to_string(),
                allowed_values: None,
            },
            SkillParameter {
                name: "include_links".to_string(),
                param_type: ParameterType::Boolean,
                required: false,
                default: Some("true".to_string()),
                description: "Include extracted links in output".to_string(),
                allowed_values: None,
            },
        ],
        builtin: true,
        tags: vec!["web".to_string(), "browse".to_string(), "extract".to_string(), "snapshot".to_string()],
    };

    Skill::new(meta, execute_web_browsing)
}

/// Execute web browsing operations
fn execute_web_browsing(
    params: HashMap<String, String>,
    ctx: &SkillContext,
) -> Result<SkillResult> {
    let operation = params.get("operation")
        .ok_or_else(|| anyhow::anyhow!("Missing 'operation' parameter"))?;

    let url = params.get("url")
        .ok_or_else(|| anyhow::anyhow!("Missing 'url' parameter"))?;

    // Validate URL
    if !is_url_allowed(url) {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("URL '{}' is not allowed (blocked for security)", url)),
            duration_ms: 0,
        });
    }

    // Check approval
    if ctx.require_approval {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Web browsing '{}' requires approval", operation)),
            duration_ms: 0,
        });
    }

    match operation.as_str() {
        "browse" => browse_url(url, &params),
        "extract" => extract_content(url, &params),
        "summarize" => summarize_page(url),
        "snapshot" => create_semantic_snapshot(url, &params),
        _ => bail!("Unknown operation: {}", operation),
    }
}

/// Check if URL is allowed (security filter)
fn is_url_allowed(url: &str) -> bool {
    let url_lower = url.to_lowercase();

    // Block localhost and internal IPs
    let blocked_patterns = [
        "localhost",
        "127.0.0.1",
        "0.0.0.0",
        "192.168.",
        "10.",
        "172.16.",
        "172.17.",
        "172.18.",
        "172.19.",
        "172.20.",
        "172.21.",
        "172.22.",
        "172.23.",
        "172.24.",
        "172.25.",
        "172.26.",
        "172.27.",
        "172.28.",
        "172.29.",
        "172.30.",
        "172.31.",
        "::1",
        "file://",
        "javascript:",
        "data:",
    ];

    for pattern in &blocked_patterns {
        if url_lower.contains(pattern) {
            return false;
        }
    }

    true
}

/// Browse a URL and return the main content
fn browse_url(url: &str, params: &HashMap<String, String>) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    let max_length: usize = params.get("max_length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50000);

    let include_links: bool = params.get("include_links")
        .and_then(|s| s.parse().ok())
        .unwrap_or(true);

    let result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.block_on(async {
            fetch_and_parse(url, max_length, include_links).await
        })
    } else {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            fetch_and_parse(url, max_length, include_links).await
        })
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(output) => Ok(SkillResult {
            success: true,
            output,
            error: None,
            duration_ms,
        }),
        Err(e) => Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Browse failed: {}", e)),
            duration_ms,
        }),
    }
}

/// Extract main content from a page using readability
fn extract_content(url: &str, params: &HashMap<String, String>) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    let max_length: usize = params.get("max_length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(30000);

    let result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.block_on(async {
            fetch_and_extract(url, max_length).await
        })
    } else {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            fetch_and_extract(url, max_length).await
        })
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(output) => Ok(SkillResult {
            success: true,
            output,
            error: None,
            duration_ms,
        }),
        Err(e) => Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Extract failed: {}", e)),
            duration_ms,
        }),
    }
}

/// Summarize a web page
fn summarize_page(url: &str) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    let result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.block_on(async {
            fetch_and_summarize(url).await
        })
    } else {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            fetch_and_summarize(url).await
        })
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(output) => Ok(SkillResult {
            success: true,
            output,
            error: None,
            duration_ms,
        }),
        Err(e) => Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Summarize failed: {}", e)),
            duration_ms,
        }),
    }
}

/// Create a semantic snapshot of a web page
fn create_semantic_snapshot(url: &str, params: &HashMap<String, String>) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    let max_length: usize = params.get("max_length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10000);

    let result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.block_on(async {
            create_snapshot(url, max_length).await
        })
    } else {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            create_snapshot(url, max_length).await
        })
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(output) => Ok(SkillResult {
            success: true,
            output,
            error: None,
            duration_ms,
        }),
        Err(e) => Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Snapshot failed: {}", e)),
            duration_ms,
        }),
    }
}

// ============================================================================
// Async helper functions
// ============================================================================

/// Fetch and parse HTML content
async fn fetch_and_parse(url: &str, max_length: usize, include_links: bool) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("my-agent/1.0")
        .build()?;

    let response = client.get(url).send().await?;
    let html = response.text().await?;

    // Parse HTML and extract main content
    let document = scraper::Html::parse_document(&html);

    // Extract title
    let title = document
        .select(&scraper::Selector::parse("title").unwrap())
        .next()
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default();

    // Extract main content
    let content = extract_main_content(&document);

    // Extract links if requested
    let links = if include_links {
        extract_links(&document, url)
    } else {
        String::new()
    };

    // Build output
    let mut output = format!("# {}\n\n", title);
    output.push_str(&content);

    if content.len() > max_length {
        output = format!("{}... [truncated]", &output[..max_length]);
    }

    if include_links && !links.is_empty() {
        output.push_str("\n\n## Links\n\n");
        output.push_str(&links);
    }

    Ok(output)
}

/// Fetch and extract main content using simple heuristics
async fn fetch_and_extract(url: &str, max_length: usize) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("my-agent/1.0")
        .build()?;

    let response = client.get(url).send().await?;
    let html = response.text().await?;

    let document = scraper::Html::parse_document(&html);

    // Try to find main content area
    let main_selectors = ["main", "article", ".content", "#content", ".post", ".article"];

    let mut content = String::new();

    for selector_str in &main_selectors {
        if let Ok(selector) = scraper::Selector::parse(selector_str) {
            if let Some(element) = document.select(&selector).next() {
                content = element.text().collect();
                content = clean_text(&content);
                if content.len() > 100 {
                    break;
                }
            }
        }
    }

    // Fallback to body if no main content found
    if content.is_empty() {
        if let Ok(selector) = scraper::Selector::parse("body") {
            if let Some(element) = document.select(&selector).next() {
                content = element.text().collect();
                content = clean_text(&content);
            }
        }
    }

    if content.len() > max_length {
        content = format!("{}... [truncated]", &content[..max_length]);
    }

    Ok(content)
}

/// Fetch and create a brief summary
async fn fetch_and_summarize(url: &str) -> Result<String> {
    let content = fetch_and_extract(url, 5000).await?;

    // Create a simple summary (first few sentences)
    let sentences: Vec<&str> = content.split(". ").take(5).collect();
    let summary = sentences.join(". ");

    Ok(format!("Summary: {}", summary))
}

/// Create a semantic snapshot
async fn create_snapshot(url: &str, max_length: usize) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("my-agent/1.0")
        .build()?;

    let response = client.get(url).send().await?;
    let html = response.text().await?;

    let document = scraper::Html::parse_document(&html);

    // Extract title
    let title = document
        .select(&scraper::Selector::parse("title").unwrap())
        .next()
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default();

    // Extract meta description
    let description = document
        .select(&scraper::Selector::parse("meta[name='description']").unwrap())
        .next()
        .and_then(|el| el.value().attr("content"))
        .unwrap_or("");

    // Extract main content
    let content = extract_main_content(&document);
    let clean_content = clean_text(&content);

    // Extract key elements (headers, lists)
    let mut key_elements = Vec::new();

    if let Ok(selector) = scraper::Selector::parse("h1, h2, h3") {
        for el in document.select(&selector).take(10) {
            key_elements.push(format!("[{}] {}", el.value().name(), el.text().collect::<String>()));
        }
    }

    // Build snapshot
    let mut snapshot = format!(
        "# Semantic Snapshot: {}\n\nURL: {}\n",
        title, url
    );

    if !description.is_empty() {
        snapshot.push_str(&format!("Description: {}\n\n", description));
    }

    if !key_elements.is_empty() {
        snapshot.push_str("## Key Sections\n\n");
        for el in &key_elements {
            snapshot.push_str(&format!("- {}\n", el));
        }
        snapshot.push_str("\n");
    }

    // Add condensed content
    snapshot.push_str("## Content Summary\n\n");

    // Take first portion and key sentences
    let words: Vec<&str> = clean_content.split_whitespace().collect();
    let summary_len = max_length.min(words.len());
    let summary: String = words[..summary_len].join(" ");

    snapshot.push_str(&summary);
    if words.len() > summary_len {
        snapshot.push_str("... [truncated]");
    }

    Ok(snapshot)
}

/// Extract main content from HTML document
fn extract_main_content(document: &scraper::Html) -> String {
    // Remove script, style, nav, footer, header elements
    let remove_selectors = ["script", "style", "nav", "footer", "header", "aside", ".sidebar", ".nav", ".footer"];

    // Try to get main content
    let main_selectors = ["main", "article", ".content", "#content", ".post", ".entry-content"];

    for selector_str in &main_selectors {
        if let Ok(selector) = scraper::Selector::parse(selector_str) {
            if let Some(element) = document.select(&selector).next() {
                let text: String = element.text().collect();
                if text.len() > 100 {
                    return text;
                }
            }
        }
    }

    // Fallback to body
    if let Ok(selector) = scraper::Selector::parse("body") {
        if let Some(element) = document.select(&selector).next() {
            return element.text().collect();
        }
    }

    String::new()
}

/// Extract links from document
fn extract_links(document: &scraper::Html, base_url: &str) -> String {
    let mut links = Vec::new();

    if let Ok(selector) = scraper::Selector::parse("a[href]") {
        for el in document.select(&selector) {
            if let Some(href) = el.value().attr("href") {
                let text: String = el.text().collect();
                let text = text.trim();

                // Skip empty or javascript links
                if text.is_empty() || href.starts_with("javascript:") {
                    continue;
                }

                // Resolve relative URLs
                let full_url = if href.starts_with("http") {
                    href.to_string()
                } else if href.starts_with("/") {
                    format!("{}{}", base_url.trim_end_matches('/'), href)
                } else {
                    format!("{}/{}", base_url.trim_end_matches('/'), href)
                };

                links.push(format!("- [{}] {}", text, full_url));
            }
        }
    }

    // Deduplicate and limit
    links.sort();
    links.dedup();
    links.truncate(20);

    links.join("\n")
}

/// Clean extracted text
fn clean_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_skill() {
        let skill = create_skill();
        assert_eq!(skill.meta.id, "builtin-web-browsing");
        assert_eq!(skill.meta.category, SkillCategory::Web);
    }

    #[test]
    fn test_url_validation() {
        // Blocked URLs
        assert!(!is_url_allowed("http://localhost/admin"));
        assert!(!is_url_allowed("http://127.0.0.1/admin"));
        assert!(!is_url_allowed("http://192.168.1.1/router"));
        assert!(!is_url_allowed("file:///etc/passwd"));
        assert!(!is_url_allowed("javascript:alert(1)"));

        // Allowed URLs
        assert!(is_url_allowed("https://example.com"));
        assert!(is_url_allowed("https://github.com/user/repo"));
    }

    #[test]
    fn test_missing_url() {
        let skill = create_skill();
        let ctx = SkillContext {
            require_approval: false,
            ..Default::default()
        };

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "browse".to_string());
        // Missing URL

        let result = skill.execute(params, &ctx);
        assert!(result.is_err() || !result.unwrap().success);
    }

    #[test]
    fn test_requires_approval() {
        let skill = create_skill();
        let ctx = SkillContext {
            require_approval: true,
            ..Default::default()
        };

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "browse".to_string());
        params.insert("url".to_string(), "https://example.com".to_string());

        let result = skill.execute(params, &ctx).unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("approval"));
    }

    #[test]
    fn test_clean_text() {
        let dirty = "  Hello   World  \n\n  Test  ";
        let clean = clean_text(dirty);
        assert_eq!(clean, "Hello World Test");
    }
}