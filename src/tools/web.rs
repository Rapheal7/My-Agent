//! Web fetch and search tools
//!
//! This module provides safe web operations with:
//! - URL validation and blocking of internal/dangerous URLs
//! - Content type filtering
//! - Size limits
//! - Timeout protection
//! - Redirect handling
//! - Rate limiting
//! - Approval integration for external requests

use crate::security::{
    ApprovalManager, ApprovalDecision,
    approval::{ActionType, Action, RiskLevel},
};
use anyhow::{Result, Context, bail};
use std::collections::HashSet;
use std::time::Duration;

/// Default timeout for web requests (30 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum content size (10 MB)
const MAX_CONTENT_SIZE: usize = 10 * 1024 * 1024;

/// Maximum redirects to follow
const MAX_REDIRECTS: usize = 5;

/// Maximum URL length
const MAX_URL_LENGTH: usize = 2048;

/// Blocked URL schemes
const BLOCKED_SCHEMES: &[&str] = &[
    "file",
    "ftp",
    "ftps",
    "javascript",
    "data",
    "vbscript",
    "about",
    "chrome",
    "resource",
    "cid",
];

/// Blocked URL patterns (internal/sensitive)
const BLOCKED_PATTERNS: &[&str] = &[
    "localhost",
    "127.",
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
    "169.254.",  // Link-local
    "0.0.0.0",
    "::1",
    "[::1]",
    "fe80:",      // IPv6 link-local
    ".local",
    ".internal",
    ".corp",
    ".home",
    ".lan",
    "metadata.google.internal",
    "169.254.169.254",  // AWS/Azure/GCP metadata
    "metadata.google.internal",
    "instance-data",  // AWS
];

/// Allowed content types
const ALLOWED_CONTENT_TYPES: &[&str] = &[
    "text/html",
    "text/plain",
    "text/css",
    "text/javascript",
    "application/javascript",
    "application/json",
    "application/xml",
    "text/xml",
    "application/rss+xml",
    "application/atom+xml",
    "text/markdown",
];

/// Blocked content types
const BLOCKED_CONTENT_TYPES: &[&str] = &[
    "application/octet-stream",
    "application/x-executable",
    "application/x-dosexec",
    "application/x-msdownload",
];

/// Web request result
#[derive(Debug, Clone)]
pub struct WebResult {
    /// The final URL (after redirects)
    pub url: String,
    /// HTTP status code
    pub status_code: u16,
    /// Content type
    pub content_type: Option<String>,
    /// Content length
    pub content_length: Option<usize>,
    /// Response headers
    pub headers: std::collections::HashMap<String, String>,
    /// Response body (text)
    pub body: String,
    /// Whether the content was truncated
    pub truncated: bool,
    /// Time taken
    pub duration_ms: u64,
}

/// Search result
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Title of the result
    pub title: String,
    /// URL
    pub url: String,
    /// Snippet/description
    pub snippet: String,
}

/// Web tool configuration
#[derive(Debug, Clone)]
pub struct WebConfig {
    /// Request timeout
    pub timeout: Duration,
    /// Maximum content size
    pub max_content_size: usize,
    /// Maximum redirects
    pub max_redirects: usize,
    /// User agent string
    pub user_agent: String,
    /// Allowed domains (empty = all non-blocked)
    pub allowed_domains: Vec<String>,
    /// Blocked domains
    pub blocked_domains: Vec<String>,
    /// Whether to allow insecure HTTPS (not recommended)
    pub allow_insecure: bool,
    /// Rate limit: requests per minute (0 = unlimited)
    pub rate_limit_per_minute: u32,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_content_size: MAX_CONTENT_SIZE,
            max_redirects: MAX_REDIRECTS,
            user_agent: format!(
                "MyAgent/0.1.0 (+https://github.com/user/my-agent; {})",
                std::env::consts::OS
            ),
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
            allow_insecure: false,
            rate_limit_per_minute: 0,
        }
    }
}

/// Rate limiter for web requests
#[derive(Debug)]
struct RateLimiter {
    requests: Vec<std::time::Instant>,
    limit_per_minute: u32,
}

impl RateLimiter {
    fn new(limit_per_minute: u32) -> Self {
        Self {
            requests: Vec::new(),
            limit_per_minute,
        }
    }

    fn check_rate_limit(&mut self) -> bool {
        if self.limit_per_minute == 0 {
            return true;
        }

        let now = std::time::Instant::now();
        let one_minute_ago = now - Duration::from_secs(60);

        // Remove old requests
        self.requests.retain(|&t| t > one_minute_ago);

        // Check if under limit
        if self.requests.len() < self.limit_per_minute as usize {
            self.requests.push(now);
            true
        } else {
            false
        }
    }
}

/// Safe web fetch tool
#[derive(Clone)]
pub struct WebTool {
    config: WebConfig,
    approver: ApprovalManager,
    rate_limiter: std::sync::Arc<std::sync::Mutex<RateLimiter>>,
    client: reqwest::Client,
}

impl WebTool {
    /// Create a new web tool with default configuration
    pub fn new() -> Result<Self> {
        let client = Self::build_client(&WebConfig::default())?;

        Ok(Self {
            config: WebConfig::default(),
            approver: ApprovalManager::with_defaults(),
            rate_limiter: std::sync::Arc::new(std::sync::Mutex::new(RateLimiter::new(0))),
            client,
        })
    }

    /// Create with custom configuration
    pub fn with_config(config: WebConfig) -> Result<Self> {
        let client = Self::build_client(&config)?;
        let rate_limit = config.rate_limit_per_minute;

        Ok(Self {
            config,
            approver: ApprovalManager::with_defaults(),
            rate_limiter: std::sync::Arc::new(std::sync::Mutex::new(RateLimiter::new(rate_limit))),
            client,
        })
    }

    /// Create with custom configuration and approver
    pub fn with_approver(config: WebConfig, approver: ApprovalManager) -> Result<Self> {
        let client = Self::build_client(&config)?;
        let rate_limit = config.rate_limit_per_minute;

        Ok(Self {
            config,
            approver,
            rate_limiter: std::sync::Arc::new(std::sync::Mutex::new(RateLimiter::new(rate_limit))),
            client,
        })
    }

    /// Build the HTTP client
    fn build_client(config: &WebConfig) -> Result<reqwest::Client> {
        let builder = reqwest::Client::builder()
            .timeout(config.timeout)
            .redirect(reqwest::redirect::Policy::limited(config.max_redirects))
            .user_agent(&config.user_agent)
            .danger_accept_invalid_certs(config.allow_insecure);

        builder.build()
            .context("Failed to build HTTP client")
    }

    /// Validate a URL for safety
    fn validate_url(&self, url: &str) -> Result<()> {
        // Check URL length
        if url.len() > MAX_URL_LENGTH {
            bail!("URL too long ({} chars, max {})", url.len(), MAX_URL_LENGTH);
        }

        // Parse URL
        let parsed = url::Url::parse(url)
            .context("Invalid URL format")?;

        // Check scheme
        let scheme = parsed.scheme().to_lowercase();
        if BLOCKED_SCHEMES.contains(&scheme.as_str()) {
            bail!("URL scheme '{}' is not allowed", scheme);
        }

        if scheme != "http" && scheme != "https" {
            bail!("URL scheme '{}' is not supported", scheme);
        }

        // Get host
        let host = parsed.host_str()
            .ok_or_else(|| anyhow::anyhow!("URL has no host"))?
            .to_lowercase();

        // Check blocked patterns
        for pattern in BLOCKED_PATTERNS {
            if host.contains(&pattern.to_lowercase()) {
                bail!("URL host contains blocked pattern: {}", pattern);
            }
        }
        for pattern in &self.config.blocked_domains {
            if host.contains(&pattern.to_lowercase()) {
                bail!("URL host contains blocked pattern: {}", pattern);
            }
        }

        // Check allowed domains if configured
        if !self.config.allowed_domains.is_empty() {
            let is_allowed = self.config.allowed_domains.iter()
                .any(|allowed| host.ends_with(&allowed.to_lowercase()));
            if !is_allowed {
                bail!("Domain '{}' is not in the allowed list", host);
            }
        }

        Ok(())
    }

    /// Determine risk level for a URL
    fn url_risk_level(&self, url: &str) -> RiskLevel {
        let parsed = match url::Url::parse(url) {
            Ok(u) => u,
            Err(_) => return RiskLevel::High, // Invalid URLs are high risk
        };

        let host = parsed.host_str()
            .map(|h| h.to_lowercase())
            .unwrap_or_default();

        // External domains are medium risk
        // Internal/file URLs should have been blocked by validate_url
        RiskLevel::Medium
    }

    /// Fetch a URL
    ///
    /// # Security
    /// - URL is validated against blocked patterns
    /// - Requires user approval for external URLs (Medium risk)
    /// - Content size is limited
    /// - Timeout protection
    pub async fn fetch(&self, url: &str) -> Result<WebResult> {
        // Validate URL
        self.validate_url(url)?;

        // Check rate limit
        if let Ok(mut limiter) = self.rate_limiter.lock() {
            if !limiter.check_rate_limit() {
                bail!("Rate limit exceeded. Please wait before making more requests.");
            }
        }

        // Determine risk and request approval
        let risk_level = self.url_risk_level(url);

        let action = Action {
            id: uuid::Uuid::new_v4().to_string(),
            action_type: ActionType::NetworkRequest,
            description: format!("Fetch URL: {}", url),
            risk_level,
            target: url.to_string(),
            details: [
                ("timeout".to_string(), format!("{:?}", self.config.timeout)),
                ("max_size".to_string(), format!("{}", self.config.max_content_size)),
            ].into_iter().collect(),
            requested_at: chrono::Utc::now(),
        };

        match self.approver.request_approval(action)? {
            ApprovalDecision::Approved | ApprovalDecision::ApprovedForSession => {
                // Continue with request
            }
            ApprovalDecision::Denied => {
                bail!("Web request denied by user");
            }
        }

        // Execute the request
        self.fetch_internal(url).await
    }

    /// Fetch without approval (for internal use after approval)
    async fn fetch_internal(&self, url: &str) -> Result<WebResult> {
        let start = std::time::Instant::now();

        // Make the request
        let response = self.client.get(url)
            .send()
            .await
            .context("Failed to fetch URL")?;

        let status = response.status();
        let status_code = status.as_u16();

        // Check content type
        let content_type = response.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or(s).to_string());

        if let Some(ref ct) = content_type {
            // Check for blocked content types
            for blocked in BLOCKED_CONTENT_TYPES {
                if ct.to_lowercase().contains(blocked) {
                    bail!("Content type '{}' is not allowed", ct);
                }
            }
        }

        // Get content length if available
        let content_length = response.content_length()
            .map(|l| l as usize);

        // Check if content is too large before downloading
        if let Some(len) = content_length {
            if len > self.config.max_content_size {
                bail!(
                    "Content too large ({} bytes, max {} bytes)",
                    len,
                    self.config.max_content_size
                );
            }
        }

        // Get the final URL after redirects
        let final_url = response.url().to_string();

        // Collect headers
        let mut headers = std::collections::HashMap::new();
        for (key, value) in response.headers() {
            if let (Ok(k), Ok(v)) = (key.to_string().parse(), value.to_str()) {
                headers.insert(k, v.to_string());
            }
        }

        // Read response body with size limit
        let body_bytes = response.bytes().await
            .context("Failed to read response body")?;

        let truncated = body_bytes.len() > self.config.max_content_size;
        let body_bytes = if truncated {
            &body_bytes[..self.config.max_content_size]
        } else {
            &body_bytes[..]
        };

        // Convert to text (best effort)
        let body = String::from_utf8_lossy(body_bytes).to_string();

        let duration = start.elapsed();

        tracing::info!(
            url = %url,
            final_url = %final_url,
            status = %status_code,
            size = %body.len(),
            duration_ms = %duration.as_millis(),
            "Web fetch completed"
        );

        Ok(WebResult {
            url: final_url,
            status_code,
            content_type,
            content_length,
            headers,
            body,
            truncated,
            duration_ms: duration.as_millis() as u64,
        })
    }

    /// Fetch and return just the text content
    pub async fn fetch_text(&self, url: &str) -> Result<String> {
        let result = self.fetch(url).await?;
        Ok(result.body)
    }

    /// Check if a URL is accessible (returns status code)
    pub async fn check_url(&self, url: &str) -> Result<u16> {
        // Quick validation without approval for checks
        self.validate_url(url)?;

        let response = self.client.head(url)
            .send()
            .await
            .context("Failed to check URL")?;

        Ok(response.status().as_u16())
    }

    /// Search the web (placeholder - requires search API integration)
    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        // This is a placeholder implementation
        // In production, you'd integrate with:
        // - SerpAPI (Google)
        // - Bing Search API
        // - Brave Search API
        // - DuckDuckGo Instant Answer API
        // - etc.

        bail!("Web search not implemented. To enable search, configure a search API provider like SerpAPI, Bing Search, or Brave Search.");
    }

    /// Get the configuration
    pub fn config(&self) -> &WebConfig {
        &self.config
    }

    /// Add a blocked domain
    pub fn block_domain(&mut self, domain: impl Into<String>) {
        self.config.blocked_domains.push(domain.into());
    }

    /// Add an allowed domain
    pub fn allow_domain(&mut self, domain: impl Into<String>) {
        self.config.allowed_domains.push(domain.into());
    }

    /// Set timeout
    pub fn set_timeout(&mut self, duration: Duration) {
        self.config.timeout = duration;
    }

    /// Set user agent
    pub fn set_user_agent(&mut self, user_agent: impl Into<String>) {
        self.config.user_agent = user_agent.into();
    }
}

/// Convenience functions for one-off operations

/// Fetch a URL with default configuration
pub async fn fetch(url: &str) -> Result<WebResult> {
    let tool = WebTool::new()?;
    tool.fetch(url).await
}

/// Fetch just the text content from a URL
pub async fn fetch_text(url: &str) -> Result<String> {
    let tool = WebTool::new()?;
    tool.fetch_text(url).await
}

/// Check if a URL is accessible
pub async fn check_url(url: &str) -> Result<u16> {
    let tool = WebTool::new()?;
    tool.check_url(url).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_validate_url() {
        let tool = WebTool::new().unwrap();

        // Valid URLs
        assert!(tool.validate_url("https://example.com").is_ok());
        assert!(tool.validate_url("http://example.com/path").is_ok());
        assert!(tool.validate_url("https://example.com:8080/path?query=1").is_ok());

        // Blocked schemes
        assert!(tool.validate_url("file:///etc/passwd").is_err());
        assert!(tool.validate_url("ftp://example.com").is_err());
        assert!(tool.validate_url("javascript:alert(1)").is_err());
        assert!(tool.validate_url("data:text/html,test").is_err());

        // Blocked internal addresses
        assert!(tool.validate_url("http://localhost").is_err());
        assert!(tool.validate_url("http://127.0.0.1").is_err());
        assert!(tool.validate_url("http://192.168.1.1").is_err());
        assert!(tool.validate_url("http://10.0.0.1").is_err());
        assert!(tool.validate_url("http://169.254.169.254").is_err());  // AWS metadata
        assert!(tool.validate_url("http://metadata.google.internal").is_err());

        // Invalid URLs
        assert!(tool.validate_url("not-a-url").is_err());
        assert!(tool.validate_url("").is_err());
    }

    #[tokio::test]
    async fn test_url_risk_level() {
        let tool = WebTool::new().unwrap();

        // External URLs are medium risk
        assert_eq!(
            tool.url_risk_level("https://example.com"),
            RiskLevel::Medium
        );

        // Invalid URLs are high risk
        assert_eq!(
            tool.url_risk_level("not-a-url"),
            RiskLevel::High
        );
    }

    #[tokio::test]
    async fn test_blocked_domains() {
        let mut config = WebConfig::default();
        config.blocked_domains = vec!["evil.com".to_string()];
        let tool = WebTool::with_config(config).unwrap();

        assert!(tool.validate_url("https://example.com").is_ok());
        assert!(tool.validate_url("https://evil.com").is_err());
        assert!(tool.validate_url("https://sub.evil.com").is_err());
    }

    #[tokio::test]
    async fn test_allowed_domains() {
        let mut config = WebConfig::default();
        config.allowed_domains = vec!["example.com".to_string(), "github.com".to_string()];
        let tool = WebTool::with_config(config).unwrap();

        assert!(tool.validate_url("https://example.com").is_ok());
        assert!(tool.validate_url("https://github.com/path").is_ok());
        assert!(tool.validate_url("https://sub.example.com").is_ok());
        assert!(tool.validate_url("https://evil.com").is_err());
    }

    #[tokio::test]
    async fn test_url_length_limit() {
        let tool = WebTool::new().unwrap();
        let long_url = format!("https://example.com/{}", "a".repeat(MAX_URL_LENGTH));

        assert!(tool.validate_url(&long_url).is_err());
    }

    #[tokio::test]
    async fn test_rate_limiter() {
        let mut limiter = RateLimiter::new(2);

        assert!(limiter.check_rate_limit());
        assert!(limiter.check_rate_limit());
        assert!(!limiter.check_rate_limit()); // Third request blocked

        // After clearing old requests (simulated by creating new limiter)
        let mut limiter2 = RateLimiter::new(2);
        assert!(limiter2.check_rate_limit());
    }
}
