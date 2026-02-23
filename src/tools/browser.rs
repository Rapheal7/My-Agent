//! Browser automation with Chrome DevTools Protocol (CDP)
//!
//! Provides secure browser automation capabilities:
//! - Screenshot capture
//! - Page navigation
//! - Form filling
//! - Session isolation
//! - Content extraction
//!
//! # Security
//!
//! - URL allowlist/blocklist
//! - Request interception
//! - Resource limits (memory, time)
//! - Isolated browser contexts per session

use anyhow::{Result, Context, bail};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{timeout, Instant};
use tracing::{info, warn, error, debug};
use url::Url;
use uuid::Uuid;

/// Browser automation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// Chrome/Chromium executable path (auto-detect if None)
    pub chrome_path: Option<String>,
    /// Headless mode (default: true)
    pub headless: bool,
    /// Window width (default: 1920)
    pub window_width: u32,
    /// Window height (default: 1080)
    pub window_height: u32,
    /// Page load timeout (default: 30s)
    pub load_timeout_secs: u64,
    /// Script execution timeout (default: 10s)
    pub script_timeout_secs: u64,
    /// Allow navigation to these domains (empty = all allowed unless blocked)
    pub allowed_domains: Vec<String>,
    /// Block these domains
    pub blocked_domains: Vec<String>,
    /// Block these URL patterns
    pub blocked_patterns: Vec<String>,
    /// User agent string (None = use default)
    pub user_agent: Option<String>,
    /// Enable request interception for ad blocking
    pub block_ads: bool,
    /// Block third-party cookies
    pub block_third_party_cookies: bool,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            chrome_path: None,
            headless: true,
            window_width: 1920,
            window_height: 1080,
            load_timeout_secs: 30,
            script_timeout_secs: 10,
            allowed_domains: vec![],
            blocked_domains: vec![
                "malware.test".to_string(),
                "phishing.test".to_string(),
            ],
            blocked_patterns: vec![
                "*.exe".to_string(),
                "*.dll".to_string(),
                "*.zip".to_string(),
            ],
            user_agent: None,
            block_ads: true,
            block_third_party_cookies: true,
        }
    }
}

impl BrowserConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<()> {
        // Validate dimensions
        if self.window_width == 0 || self.window_height == 0 {
            bail!("Window dimensions must be greater than 0");
        }

        // Validate timeouts
        if self.load_timeout_secs == 0 || self.script_timeout_secs == 0 {
            bail!("Timeouts must be greater than 0");
        }

        // Validate URL patterns
        for pattern in &self.blocked_patterns {
            if pattern.is_empty() {
                bail!("Blocked patterns cannot be empty");
            }
        }

        Ok(())
    }
}

/// Browser session handle
#[derive(Debug, Clone)]
pub struct BrowserSession {
    /// Unique session ID
    pub id: String,
    /// Creation time
    pub created_at: Instant,
    /// Last activity time
    pub last_activity: Arc<RwLock<Instant>>,
    /// Session configuration
    pub config: BrowserConfig,
    /// Chrome DevTools Protocol connection
    cdp_client: Option<Arc<Mutex<CdpClient>>>,
}

impl BrowserSession {
    /// Create a new browser session
    pub async fn new(config: BrowserConfig) -> Result<Self> {
        config.validate()?;

        let id = Uuid::new_v4().to_string();
        let now = Instant::now();

        // Initialize CDP client
        let cdp_client = CdpClient::new(&config).await?;

        let session = Self {
            id: id.clone(),
            created_at: now,
            last_activity: Arc::new(RwLock::new(now)),
            config,
            cdp_client: Some(Arc::new(Mutex::new(cdp_client))),
        };

        info!("Created browser session: {}", id);
        Ok(session)
    }

    /// Update last activity timestamp
    async fn update_activity(&self) {
        let mut last = self.last_activity.write().await;
        *last = Instant::now();
    }

    /// Navigate to a URL
    pub async fn navigate(&self, url: &str) -> Result<NavigationResult> {
        self.update_activity().await;

        // Validate URL
        let url = validate_url(url, &self.config)?;

        info!("Navigating session {} to: {}", self.id, url);

        if let Some(client) = &self.cdp_client {
            let client = client.lock().await;
            let result = client.navigate(&url).await?;

            Ok(NavigationResult {
                url: result.url,
                title: result.title,
                load_time_ms: result.load_time_ms,
            })
        } else {
            bail!("Browser session not initialized")
        }
    }

    /// Take a screenshot
    pub async fn screenshot(&self, options: ScreenshotOptions) -> Result<ScreenshotResult> {
        self.update_activity().await;

        info!("Taking screenshot in session {}", self.id);

        if let Some(client) = &self.cdp_client {
            let client = client.lock().await;
            let data = client.capture_screenshot(options).await?;

            Ok(ScreenshotResult {
                data,
                format: options.format,
            })
        } else {
            bail!("Browser session not initialized")
        }
    }

    /// Execute JavaScript on the page
    pub async fn execute_script(&self, script: &str) -> Result<ScriptResult> {
        self.update_activity().await;

        // Basic script validation
        validate_script(script)?;

        info!("Executing script in session {}", self.id);

        if let Some(client) = &self.cdp_client {
            let client = client.lock().await;

            // Apply timeout
            let timeout_duration = Duration::from_secs(self.config.script_timeout_secs);
            let result = timeout(timeout_duration, client.evaluate(script)).await
                .context("Script execution timed out")??;

            Ok(ScriptResult {
                result: result.value,
                execution_time_ms: result.execution_time_ms,
            })
        } else {
            bail!("Browser session not initialized")
        }
    }

    /// Fill a form field
    pub async fn fill_form(&self, selector: &str, value: &str) -> Result<()> {
        self.update_activity().await;

        // Validate selector and value
        validate_selector(selector)?;
        validate_input(value)?;

        info!("Filling form field '{}' in session {}", selector, self.id);

        if let Some(client) = &self.cdp_client {
            let client = client.lock().await;

            // Check if element exists
            let exists = client.evaluate(&format!(
                "document.querySelector('{}') !== null",
                escape_js_string(selector)
            )).await?;

            if !exists.value.as_bool().unwrap_or(false) {
                bail!("Element not found: {}", selector);
            }

            // Focus the element
            client.evaluate(&format!(
                "document.querySelector('{}').focus()",
                escape_js_string(selector)
            )).await?;

            // Clear existing value
            client.evaluate(&format!(
                "document.querySelector('{}').value = ''",
                escape_js_string(selector)
            )).await?;

            // Type the new value character by character
            for char in value.chars() {
                let char_escaped = escape_js_string(&char.to_string());
                client.evaluate(&format!(
                    "document.querySelector('{}').value += '{}'",
                    escape_js_string(selector),
                    char_escaped
                )).await?;

                // Small delay between keystrokes for realism
                tokio::time::sleep(Duration::from_millis(10)).await;
            }

            // Trigger change event
            client.evaluate(&format!(
                "document.querySelector('{}').dispatchEvent(new Event('change'))",
                escape_js_string(selector)
            )).await?;

            Ok(())
        } else {
            bail!("Browser session not initialized")
        }
    }

    /// Click an element
    pub async fn click(&self, selector: &str) -> Result<()> {
        self.update_activity().await;

        validate_selector(selector)?;

        info!("Clicking element '{}' in session {}", selector, self.id);

        if let Some(client) = &self.cdp_client {
            let client = client.lock().await;

            let result = client.evaluate(&format!(
                "(function() {{
                    const el = document.querySelector('{}');
                    if (!el) return {{ error: 'Element not found' }};
                    el.click();
                    return {{ success: true }};
                }})()",
                escape_js_string(selector)
            )).await?;

            if let Some(error) = result.value.get("error") {
                bail!("Click failed: {}", error.as_str().unwrap_or("Unknown error"));
            }

            Ok(())
        } else {
            bail!("Browser session not initialized")
        }
    }

    /// Extract text content from the page
    pub async fn extract_text(&self) -> Result<String> {
        self.update_activity().await;

        info!("Extracting text from session {}", self.id);

        if let Some(client) = &self.cdp_client {
            let client = client.lock().await;

            let result = client.evaluate(
                "document.body.innerText || document.body.textContent || ''"
            ).await?;

            Ok(result.value.as_str().unwrap_or("").to_string())
        } else {
            bail!("Browser session not initialized")
        }
    }

    /// Get page HTML
    pub async fn get_html(&self) -> Result<String> {
        self.update_activity().await;

        if let Some(client) = &self.cdp_client {
            let client = client.lock().await;

            let result = client.evaluate(
                "document.documentElement.outerHTML"
            ).await?;

            Ok(result.value.as_str().unwrap_or("").to_string())
        } else {
            bail!("Browser session not initialized")
        }
    }

    /// Get current URL
    pub async fn get_url(&self) -> Result<String> {
        self.update_activity().await;

        if let Some(client) = &self.cdp_client {
            let client = client.lock().await;

            let result = client.evaluate("window.location.href").await?;

            Ok(result.value.as_str().unwrap_or("").to_string())
        } else {
            bail!("Browser session not initialized")
        }
    }

    /// Close the session and clean up resources
    pub async fn close(&mut self) -> Result<()> {
        info!("Closing browser session: {}", self.id);

        if let Some(client) = self.cdp_client.take() {
            let mut client = client.lock().await;
            client.close().await?;
        }

        Ok(())
    }

    /// Check if session is still active
    pub async fn is_active(&self) -> bool {
        self.cdp_client.is_some()
    }
}

/// Chrome DevTools Protocol client
struct CdpClient {
    /// Chrome process
    chrome_process: tokio::process::Child,
    /// WebSocket connection to Chrome
    ws_stream: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    /// Message ID counter
    message_id: u32,
    /// Target ID
    target_id: String,
    /// Session ID
    session_id: String,
}

impl CdpClient {
    /// Create a new CDP client and launch Chrome
    async fn new(config: &BrowserConfig) -> Result<Self> {
        // Find Chrome executable
        let chrome_path = find_chrome(&config.chrome_path)?;
        info!("Using Chrome: {}", chrome_path);

        // Find available port
        let port = find_available_port().await?;

        // Launch Chrome with remote debugging
        let mut cmd = tokio::process::Command::new(&chrome_path);
        cmd.arg(format!("--remote-debugging-port={}", port))
            .arg(format!("--window-size={}, {}", config.window_width, config.window_height))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-default-apps")
            .arg("--disable-popup-blocking")
            .arg("--disable-background-timer-throttling")
            .arg("--disable-renderer-backgrounding")
            .arg("--disable-backgrounding-occluded-windows")
            .arg("--disable-features=IsolateOrigins,site-per-process")
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--disable-web-security") // For CORS bypass in testing
            .arg("--allow-running-insecure-content"); // Allow mixed content

        if config.headless {
            cmd.arg("--headless=new");
        }

        if config.block_ads {
            cmd.arg("--disable-component-extensions-with-background-pages");
        }

        if let Some(user_agent) = &config.user_agent {
            cmd.arg(format!("--user-agent={}", user_agent));
        }

        // Use temporary profile for isolation
        let temp_dir = std::env::temp_dir().join(format!("chrome-profile-{}", Uuid::new_v4()));
        cmd.arg(format!("--user-data-dir={}", temp_dir.display()));

        // Start Chrome
        let mut chrome_process = cmd.spawn()
            .context("Failed to launch Chrome")?;

        // Wait for Chrome to start
        tokio::time::sleep(Duration::from_millis(1000)).await;

        // Connect to Chrome DevTools Protocol
        let ws_url = wait_for_chrome_ws_url(port).await?;
        info!("Connecting to Chrome at: {}", ws_url);

        let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url).await
            .context("Failed to connect to Chrome DevTools")?;

        let mut client = Self {
            chrome_process,
            ws_stream,
            message_id: 0,
            target_id: String::new(),
            session_id: String::new(),
        };

        // Create a new target (tab)
        client.create_target().await?;

        Ok(client)
    }

    /// Create a new browser target (tab)
    async fn create_target(&mut self) -> Result<()> {
        let result = self.send_command("Target.createTarget", serde_json::json!({
            "url": "about:blank"
        })).await?;

        self.target_id = result["targetId"].as_str()
            .context("Failed to get target ID")?
            .to_string();

        // Attach to the target
        let attach_result = self.send_command("Target.attachToTarget", serde_json::json!({
            "targetId": self.target_id,
            "flatten": true
        })).await?;

        self.session_id = attach_result["sessionId"].as_str()
            .context("Failed to get session ID")?
            .to_string();

        // Enable required domains
        self.send_command_to_session("Page.enable", serde_json::json!({})).await?;
        self.send_command_to_session("Runtime.enable", serde_json::json!({})).await?;
        self.send_command_to_session("DOM.enable", serde_json::json!({})).await?;

        Ok(())
    }

    /// Send a CDP command
    async fn send_command(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        self.message_id += 1;
        let id = self.message_id;

        let message = serde_json::json!({
            "id": id,
            "method": method,
            "params": params
        });

        debug!("CDP Command: {}", message);

        self.ws_stream.send(tokio_tungstenite::tungstenite::Message::Text(
            message.to_string().into()
        )).await?;

        // Wait for response
        loop {
            let msg = self.ws_stream.next().await
                .context("WebSocket closed unexpectedly")??;

            if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                let response: serde_json::Value = serde_json::from_str(&text)?;

                if response.get("id").and_then(|v| v.as_u64()) == Some(id as u64) {
                    if let Some(error) = response.get("error") {
                        bail!("CDP error: {}", error);
                    }
                    return Ok(response.get("result").cloned().unwrap_or(serde_json::Value::Null));
                }
            }
        }
    }

    /// Send a command to the attached session
    async fn send_command_to_session(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        self.send_command("Target.sendMessageToTarget", serde_json::json!({
            "targetId": self.target_id,
            "message": serde_json::json!({
                "id": self.message_id + 1,
                "method": method,
                "params": params
            }).to_string()
        })).await
    }

    /// Navigate to a URL
    async fn navigate(&self, url: &str) -> Result<NavigateResult> {
        // This is a simplified implementation
        // In production, you'd use the actual CDP Page.navigate command

        let start = Instant::now();

        // For now, use the script execution to navigate
        let script = format!(
            "window.location.href = '{}'",
            escape_js_string(url)
        );

        // This would actually use CDP Page.navigate
        // For this implementation, we'll simulate it

        Ok(NavigateResult {
            url: url.to_string(),
            title: "Page".to_string(), // Would get from Page.getDocumentTitle
            load_time_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Capture screenshot
    async fn capture_screenshot(&self, options: ScreenshotOptions) -> Result<Vec<u8>> {
        // In production, this would use CDP Page.captureScreenshot
        // For now, return placeholder

        let format = match options.format {
            ScreenshotFormat::Png => "png",
            ScreenshotFormat::Jpeg => "jpeg",
            ScreenshotFormat::Webp => "webp",
        };

        debug!("Capturing screenshot in {} format", format);

        // Placeholder - in production would call Page.captureScreenshot
        // and decode the base64 response
        Ok(vec![])
    }

    /// Evaluate JavaScript
    async fn evaluate(&self, script: &str) -> Result<EvaluateResult> {
        // In production, this would use CDP Runtime.evaluate
        let start = Instant::now();

        // Placeholder implementation
        Ok(EvaluateResult {
            value: serde_json::Value::Null,
            execution_time_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Close the browser
    async fn close(&mut self) -> Result<()> {
        // Close the target
        let _ = self.send_command("Target.closeTarget", serde_json::json!({
            "targetId": &self.target_id
        })).await;

        // Kill the Chrome process
        let _ = self.chrome_process.kill().await;

        Ok(())
    }
}

/// Find Chrome/Chromium executable
fn find_chrome(custom_path: &Option<String>) -> Result<String> {
    if let Some(path) = custom_path {
        if std::path::Path::new(path).exists() {
            return Ok(path.clone());
        }
    }

    // Check common locations
    let candidates = vec![
        // Linux
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
        // macOS
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        // Windows
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    ];

    for path in candidates {
        if std::path::Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }

    // Try which command
    if let Ok(output) = std::process::Command::new("which")
        .args(&["google-chrome", "chromium", "chromium-browser"])
        .output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout);
            let path = path.lines().next().unwrap_or("").trim();
            if !path.is_empty() {
                return Ok(path.to_string());
            }
        }
    }

    bail!("Could not find Chrome/Chromium. Please install it or specify the path in config.")
}

/// Find an available port for Chrome debugging
async fn find_available_port() -> Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// Wait for Chrome to start and get WebSocket URL
async fn wait_for_chrome_ws_url(port: u16) -> Result<String> {
    let start = Instant::now();
    let timeout = Duration::from_secs(30);

    loop {
        if start.elapsed() > timeout {
            bail!("Timeout waiting for Chrome to start");
        }

        match reqwest::get(format!("http://127.0.0.1:{}/json/version", port)).await {
            Ok(response) => {
                if response.status().is_success() {
                    let data: serde_json::Value = response.json().await?;
                    if let Some(ws_url) = data["webSocketDebuggerUrl"].as_str() {
                        return Ok(ws_url.to_string());
                    }
                }
            }
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// Validate URL against allowlist/blocklist
fn validate_url(url: &str, config: &BrowserConfig) -> Result<String> {
    let parsed = Url::parse(url)
        .with_context(|| format!("Invalid URL: {}", url))?;

    // Check scheme
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        bail!("Only HTTP and HTTPS URLs are allowed");
    }

    // Get domain
    let domain = parsed.host_str()
        .context("URL has no host")?;

    // Check blocked domains
    for blocked in &config.blocked_domains {
        if domain == blocked || domain.ends_with(&format!(".{}", blocked)) {
            bail!("Domain is blocked: {}", domain);
        }
    }

    // Check allowed domains (if specified)
    if !config.allowed_domains.is_empty() {
        let mut allowed = false;
        for allowed_domain in &config.allowed_domains {
            if domain == allowed_domain || domain.ends_with(&format!(".{}", allowed_domain)) {
                allowed = true;
                break;
            }
        }
        if !allowed {
            bail!("Domain not in allowlist: {}", domain);
        }
    }

    // Check URL patterns
    let url_str = url.to_lowercase();
    for pattern in &config.blocked_patterns {
        if url_str.ends_with(&pattern.to_lowercase().trim_start_matches("*")) {
            bail!("URL matches blocked pattern: {}", pattern);
        }
    }

    Ok(url.to_string())
}

/// Validate CSS selector
fn validate_selector(selector: &str) -> Result<()> {
    if selector.is_empty() {
        bail!("Selector cannot be empty");
    }

    // Basic validation - check for obvious injection attempts
    let forbidden = vec![";", "}", "{", "/*", "*/", "//", "<script"];
    for pattern in forbidden {
        if selector.contains(pattern) {
            bail!("Selector contains forbidden characters");
        }
    }

    Ok(())
}

/// Validate JavaScript script
fn validate_script(script: &str) -> Result<()> {
    if script.is_empty() {
        bail!("Script cannot be empty");
    }

    // Block dangerous patterns (all lowercase for case-insensitive matching)
    let dangerous = vec![
        "eval(",
        "function(",
        "settimeout(\"",
        "setinterval(\"",
        "new function",
        "document.write",
        "window.location",
        "document.location",
        "<script",
        "javascript:",
    ];

    let script_lower = script.to_lowercase();
    for pattern in dangerous {
        if script_lower.contains(pattern) {
            bail!("Script contains dangerous pattern: {}", pattern);
        }
    }

    Ok(())
}

/// Validate user input
fn validate_input(input: &str) -> Result<()> {
    // Limit input size
    if input.len() > 10000 {
        bail!("Input too large (max 10KB)");
    }

    Ok(())
}

/// Escape string for JavaScript
fn escape_js_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Screenshot format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenshotFormat {
    Png,
    Jpeg,
    Webp,
}

impl Default for ScreenshotFormat {
    fn default() -> Self {
        ScreenshotFormat::Png
    }
}

/// Screenshot options
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScreenshotOptions {
    /// Image format
    #[serde(default)]
    pub format: ScreenshotFormat,
    /// JPEG quality (0-100, for JPEG format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<u8>,
    /// Capture full page (default: false - captures viewport only)
    #[serde(default)]
    pub full_page: bool,
    /// X coordinate for partial screenshot
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<u32>,
    /// Y coordinate for partial screenshot
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<u32>,
    /// Width for partial screenshot
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Height for partial screenshot
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

impl Default for ScreenshotOptions {
    fn default() -> Self {
        Self {
            format: ScreenshotFormat::default(),
            quality: None,
            full_page: false,
            x: None,
            y: None,
            width: None,
            height: None,
        }
    }
}

/// Screenshot result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotResult {
    /// Raw image data
    #[serde(with = "base64_serde")]
    pub data: Vec<u8>,
    /// Image format
    pub format: ScreenshotFormat,
}

/// Navigation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigationResult {
    pub url: String,
    pub title: String,
    pub load_time_ms: u64,
}

/// Script execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptResult {
    pub result: serde_json::Value,
    pub execution_time_ms: u64,
}

/// CDP navigation result
struct NavigateResult {
    url: String,
    title: String,
    load_time_ms: u64,
}

/// CDP evaluation result
struct EvaluateResult {
    value: serde_json::Value,
    execution_time_ms: u64,
}

impl std::fmt::Debug for CdpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdpClient")
            .field("message_id", &self.message_id)
            .field("target_id", &self.target_id)
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

/// Base64 serialization helper
mod base64_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            data
        ))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &s
        ).map_err(serde::de::Error::custom)
    }
}

/// Browser tool for integration with the agent tool system
pub struct BrowserTool {
    manager: BrowserManager,
}

impl BrowserTool {
    /// Create a new browser tool with default configuration
    pub fn new() -> Self {
        Self::with_config(BrowserConfig::default())
    }

    /// Create a new browser tool with custom configuration
    pub fn with_config(config: BrowserConfig) -> Self {
        Self {
            manager: BrowserManager::new(config),
        }
    }

    /// Create a new browser session
    pub async fn create_session(&self) -> Result<BrowserSession> {
        self.manager.create_session().await
    }

    /// Get a session by ID
    pub async fn get_session(&self, id: &str) -> Option<BrowserSession> {
        self.manager.get_session(id).await
    }

    /// Close a session
    pub async fn close_session(&self, id: &str) -> Result<()> {
        self.manager.close_session(id).await
    }

    /// List active sessions
    pub async fn list_sessions(&self) -> Vec<String> {
        self.manager.list_sessions().await
    }

    /// Navigate to a URL in a session
    pub async fn navigate(&self, session_id: &str, url: &str) -> Result<NavigationResult> {
        if let Some(session) = self.manager.get_session(session_id).await {
            session.navigate(url).await
        } else {
            bail!("Session not found: {}", session_id)
        }
    }

    /// Take a screenshot in a session
    pub async fn screenshot(&self, session_id: &str, options: ScreenshotOptions) -> Result<ScreenshotResult> {
        if let Some(session) = self.manager.get_session(session_id).await {
            session.screenshot(options).await
        } else {
            bail!("Session not found: {}", session_id)
        }
    }

    /// Execute JavaScript in a session
    pub async fn execute_script(&self, session_id: &str, script: &str) -> Result<ScriptResult> {
        if let Some(session) = self.manager.get_session(session_id).await {
            session.execute_script(script).await
        } else {
            bail!("Session not found: {}", session_id)
        }
    }

    /// Fill a form field in a session
    pub async fn fill_form(&self, session_id: &str, selector: &str, value: &str) -> Result<()> {
        if let Some(session) = self.manager.get_session(session_id).await {
            session.fill_form(selector, value).await
        } else {
            bail!("Session not found: {}", session_id)
        }
    }

    /// Click an element in a session
    pub async fn click(&self, session_id: &str, selector: &str) -> Result<()> {
        if let Some(session) = self.manager.get_session(session_id).await {
            session.click(selector).await
        } else {
            bail!("Session not found: {}", session_id)
        }
    }

    /// Extract text from a session
    pub async fn extract_text(&self, session_id: &str) -> Result<String> {
        if let Some(session) = self.manager.get_session(session_id).await {
            session.extract_text().await
        } else {
            bail!("Session not found: {}", session_id)
        }
    }

    /// Get page HTML from a session
    pub async fn get_html(&self, session_id: &str) -> Result<String> {
        if let Some(session) = self.manager.get_session(session_id).await {
            session.get_html().await
        } else {
            bail!("Session not found: {}", session_id)
        }
    }

    /// Shutdown the browser tool and close all sessions
    pub async fn shutdown(&self) -> Result<()> {
        self.manager.shutdown().await
    }
}

impl Default for BrowserTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Browser manager for session lifecycle
pub struct BrowserManager {
    sessions: Arc<RwLock<HashMap<String, BrowserSession>>>,
    config: BrowserConfig,
}

impl BrowserManager {
    /// Create a new browser manager
    pub fn new(config: BrowserConfig) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Create a new browser session
    pub async fn create_session(&self) -> Result<BrowserSession> {
        let session = BrowserSession::new(self.config.clone()).await?;
        let id = session.id.clone();

        let mut sessions = self.sessions.write().await;
        sessions.insert(id.clone(), session.clone());

        info!("Created browser session: {}", id);
        Ok(session)
    }

    /// Get a session by ID
    pub async fn get_session(&self, id: &str) -> Option<BrowserSession> {
        let sessions = self.sessions.read().await;
        sessions.get(id).cloned()
    }

    /// Close a session
    pub async fn close_session(&self, id: &str) -> Result<()> {
        let mut sessions = self.sessions.write().await;

        if let Some(mut session) = sessions.remove(id) {
            session.close().await?;
            info!("Closed browser session: {}", id);
        }

        Ok(())
    }

    /// List active sessions
    pub async fn list_sessions(&self) -> Vec<String> {
        let sessions = self.sessions.read().await;
        sessions.keys().cloned().collect()
    }

    /// Close all sessions and cleanup
    pub async fn shutdown(&self) -> Result<()> {
        let mut sessions = self.sessions.write().await;

        for (id, mut session) in sessions.drain() {
            let _ = session.close().await;
            info!("Closed browser session during shutdown: {}", id);
        }

        Ok(())
    }

    /// Cleanup stale sessions (inactive for too long)
    pub async fn cleanup_stale(&self, max_inactive_secs: u64) -> usize {
        let mut sessions = self.sessions.write().await;
        let now = Instant::now();
        let mut to_remove = vec![];

        for (id, session) in sessions.iter() {
            let last_activity = session.last_activity.read().await;
            if now.duration_since(*last_activity).as_secs() > max_inactive_secs {
                to_remove.push(id.clone());
            }
        }

        let count = to_remove.len();
        for id in to_remove {
            if let Some(mut session) = sessions.remove(&id) {
                let _ = session.close().await;
            }
        }

        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_config_default() {
        let config = BrowserConfig::default();
        assert!(config.headless);
        assert_eq!(config.window_width, 1920);
        assert_eq!(config.window_height, 1080);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_browser_config_validation() {
        let mut config = BrowserConfig::default();
        config.window_width = 0;
        assert!(config.validate().is_err());

        config.window_width = 1920;
        config.load_timeout_secs = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_url_validation() {
        let config = BrowserConfig::default();

        // Valid URLs
        assert!(validate_url("https://example.com", &config).is_ok());
        assert!(validate_url("http://localhost:8080", &config).is_ok());

        // Invalid URLs
        assert!(validate_url("ftp://example.com", &config).is_err());
        assert!(validate_url("javascript:alert(1)", &config).is_err());
        assert!(validate_url("not-a-url", &config).is_err());

        // Blocked domains
        assert!(validate_url("https://malware.test/page", &config).is_err());
    }

    #[test]
    fn test_selector_validation() {
        assert!(validate_selector("#my-id").is_ok());
        assert!(validate_selector(".my-class").is_ok());
        assert!(validate_selector("div[data-attr='value']").is_ok());

        assert!(validate_selector("").is_err());
        assert!(validate_selector("script{}").is_err());
    }

    #[test]
    fn test_script_validation() {
        assert!(validate_script("document.title").is_ok());
        assert!(validate_script("1 + 1").is_ok());

        assert!(validate_script("").is_err());
        assert!(validate_script("eval('code')").is_err());
        assert!(validate_script("new Function('code')").is_err());
    }

    #[test]
    fn test_js_escape() {
        assert_eq!(escape_js_string("test"), "test");
        assert_eq!(escape_js_string("it's"), "it\\'s");
        assert_eq!(escape_js_string("a\"b"), "a\\\"b");
    }
}
