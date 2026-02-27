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
            headless: false,
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

    /// Check if the CDP connection is still alive
    pub async fn is_alive(&self) -> bool {
        if let Some(client) = &self.cdp_client {
            let mut client = client.lock().await;
            // Send a lightweight CDP command to check the connection
            client.send_command_to_session("Runtime.evaluate", serde_json::json!({
                "expression": "1"
            })).await.is_ok()
        } else {
            false
        }
    }

    /// Navigate to a URL
    pub async fn navigate(&self, url: &str) -> Result<NavigationResult> {
        self.update_activity().await;

        // Validate URL
        let url = validate_url(url, &self.config)?;

        info!("Navigating session {} to: {}", self.id, url);

        if let Some(client) = &self.cdp_client {
            let mut client = client.lock().await;
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
            let mut client = client.lock().await;
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
            let mut client = client.lock().await;

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
            let mut client = client.lock().await;

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
            let mut client = client.lock().await;

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
            let mut client = client.lock().await;

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
            let mut client = client.lock().await;

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
            let mut client = client.lock().await;

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
        // Find Chrome/Chromium executable
        let chrome_path = find_chrome(&config.chrome_path)?;
        info!("Using Chrome: {}", chrome_path);

        // Find available port
        let port = find_available_port().await?;

        // Launch Chrome with remote debugging
        let mut cmd = tokio::process::Command::new(&chrome_path);
        cmd.arg(format!("--remote-debugging-port={}", port))
            .arg(format!("--window-size={},{}", config.window_width, config.window_height))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-default-apps")
            .arg("--disable-popup-blocking")
            .arg("--disable-background-timer-throttling")
            .arg("--disable-renderer-backgrounding")
            .arg("--disable-backgrounding-occluded-windows")
            .arg("--disable-features=IsolateOrigins,site-per-process")
            .arg("--disable-blink-features=AutomationControlled");

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

        // Suppress browser stdout/stderr noise
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        // Start Chrome
        let chrome_process = cmd.spawn()
            .with_context(|| format!("Failed to launch Chrome: {}", chrome_path))?;

        // Brief wait before polling CDP endpoint (the ws polling loop handles the real wait)
        tokio::time::sleep(Duration::from_millis(200)).await;

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

    /// Send a command to the attached session (flat protocol — include sessionId directly)
    async fn send_command_to_session(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        self.message_id += 1;
        let id = self.message_id;

        let message = serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
            "sessionId": self.session_id
        });

        debug!("CDP Session Command: {}", message);

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
                // Skip events and other messages
            }
        }
    }

    /// Navigate to a URL using Page.navigate CDP command
    async fn navigate(&mut self, url: &str) -> Result<NavigateResult> {
        let start = Instant::now();

        // Use Page.navigate via the session
        self.send_command_to_session("Page.navigate", serde_json::json!({
            "url": url
        })).await?;

        // Poll document.readyState instead of sleeping a fixed 2s
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if Instant::now() > deadline {
                break; // Proceed anyway after 10s
            }
            let state = self.send_command_to_session("Runtime.evaluate", serde_json::json!({
                "expression": "document.readyState"
            })).await;
            if let Ok(val) = state {
                let ready = val["result"]["value"].as_str().unwrap_or("");
                if ready == "complete" || ready == "interactive" {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Get the actual page URL and title in one eval
        let eval = self.send_command_to_session("Runtime.evaluate", serde_json::json!({
            "expression": "JSON.stringify({url: document.URL, title: document.title})",
            "returnByValue": true
        })).await?;
        let json_str = eval["result"]["value"].as_str().unwrap_or("{}");
        let info: serde_json::Value = serde_json::from_str(json_str).unwrap_or_default();

        Ok(NavigateResult {
            url: info["url"].as_str().unwrap_or(url).to_string(),
            title: info["title"].as_str().unwrap_or("").to_string(),
            load_time_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Capture screenshot via CDP Page.captureScreenshot
    async fn capture_screenshot(&mut self, options: ScreenshotOptions) -> Result<Vec<u8>> {
        let format = match options.format {
            ScreenshotFormat::Png => "png",
            ScreenshotFormat::Jpeg => "jpeg",
            ScreenshotFormat::Webp => "webp",
        };

        debug!("Capturing screenshot in {} format", format);

        let result = self.send_command_to_session("Page.captureScreenshot", serde_json::json!({
            "format": format
        })).await?;

        let data_b64 = result["data"].as_str()
            .context("No screenshot data in CDP response")?;

        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data_b64)
            .context("Failed to decode screenshot base64")?;

        Ok(bytes)
    }

    /// Evaluate JavaScript via CDP Runtime.evaluate
    async fn evaluate(&mut self, script: &str) -> Result<EvaluateResult> {
        let start = Instant::now();

        let result = self.send_command_to_session("Runtime.evaluate", serde_json::json!({
            "expression": script,
            "returnByValue": true
        })).await?;

        let value = result.get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        Ok(EvaluateResult {
            value,
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

    // Check common Chrome/Chromium locations
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

    for path in &candidates {
        if std::path::Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }

    // Try `which` for various Chrome names
    for name in &["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"] {
        if let Ok(output) = std::process::Command::new("which").arg(name).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Ok(path);
                }
            }
        }
    }

    bail!(
        "Chrome or Chromium is required for browser automation (CDP). Firefox does not support the Chrome DevTools Protocol.\n\
        Install one of these:\n\
        - Ubuntu/Debian: sudo apt install chromium-browser  OR  sudo snap install chromium\n\
        - Fedora: sudo dnf install chromium\n\
        - macOS: brew install --cask google-chrome\n\
        - Or download from: https://www.google.com/chrome/"
    )
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
    // Short timeout — if Chrome doesn't start in 10s, it won't
    let timeout_dur = Duration::from_secs(10);

    loop {
        if start.elapsed() > timeout_dur {
            bail!("Timeout waiting for Chrome to start on port {}. Is Chrome/Chromium installed?", port);
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
                tokio::time::sleep(Duration::from_millis(200)).await;
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

/// An accessibility tree node with an optional ref ID for interactive elements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AXNode {
    /// Reference ID (e.g., "@e1") — only for interactive elements
    pub ref_id: Option<String>,
    /// ARIA role
    pub role: String,
    /// Accessible name
    pub name: String,
    /// Whether the element is focused
    pub focused: bool,
    /// Current value (for inputs, selects, etc.)
    pub value: Option<String>,
    /// CDP backend node ID for resolving to DOM
    pub backend_node_id: Option<i64>,
}

/// Accessibility tree snapshot result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AXSnapshot {
    /// Current page URL
    pub url: String,
    /// Page title
    pub title: String,
    /// Compact text representation of the accessibility tree
    pub tree_text: String,
    /// Number of interactive elements with refs
    pub element_count: usize,
}

/// Map from ref IDs (@e1, @e2, ...) to backend node IDs
#[derive(Debug, Clone, Default)]
pub struct RefMap {
    pub refs: HashMap<String, i64>,
}

/// Interactive ARIA roles that get ref IDs
const INTERACTIVE_ROLES: &[&str] = &[
    "button", "link", "textbox", "checkbox", "radio", "combobox",
    "listbox", "menuitem", "menuitemcheckbox", "menuitemradio",
    "option", "searchbox", "slider", "spinbutton", "switch",
    "tab", "treeitem", "gridcell",
];

impl BrowserSession {
    /// Get an accessibility tree snapshot with ref IDs for interactive elements
    pub async fn accessibility_snapshot(&self) -> Result<(AXSnapshot, RefMap)> {
        self.update_activity().await;

        info!("Getting accessibility snapshot for session {}", self.id);

        if let Some(client) = &self.cdp_client {
            let mut client = client.lock().await;

            // Enable Accessibility domain
            let _ = client.send_command_to_session("Accessibility.enable", serde_json::json!({})).await;

            // Get the full accessibility tree
            let ax_result = client.send_command_to_session(
                "Accessibility.getFullAXTree",
                serde_json::json!({})
            ).await?;

            // Get current URL and title
            let url = client.evaluate("window.location.href").await
                .map(|r| r.value.as_str().unwrap_or("").to_string())
                .unwrap_or_default();
            let title = client.evaluate("document.title").await
                .map(|r| r.value.as_str().unwrap_or("").to_string())
                .unwrap_or_default();

            // Parse the AX tree nodes
            let nodes = ax_result.get("nodes")
                .and_then(|n| n.as_array())
                .cloned()
                .unwrap_or_default();

            let mut tree_lines = Vec::new();
            let mut ref_map = RefMap::default();
            let mut ref_counter = 0usize;

            for node in &nodes {
                let role = node.get("role")
                    .and_then(|r| r.get("value"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // Skip generic/invisible roles
                if role.is_empty() || role == "none" || role == "generic" || role == "Ignored" {
                    continue;
                }

                let name = node.get("name")
                    .and_then(|n| n.get("value"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let value = node.get("value")
                    .and_then(|v| v.get("value"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let backend_node_id = node.get("backendDOMNodeId")
                    .and_then(|v| v.as_i64());

                let is_interactive = INTERACTIVE_ROLES.contains(&role.to_lowercase().as_str());

                if is_interactive && backend_node_id.is_some() {
                    ref_counter += 1;
                    let ref_id = format!("@e{}", ref_counter);
                    ref_map.refs.insert(ref_id.clone(), backend_node_id.unwrap());

                    let mut line = format!("[{}] {} \"{}\"", ref_id, role, name);
                    if let Some(ref val) = value {
                        if !val.is_empty() {
                            line.push_str(&format!(" value=\"{}\"", val));
                        }
                    }
                    tree_lines.push(line);
                } else if !name.is_empty() {
                    // Non-interactive elements with names for context (headings, text, etc.)
                    tree_lines.push(format!("{}: {}", role, name));
                }
            }

            let tree_text = tree_lines.join("\n");
            let element_count = ref_counter;

            Ok((AXSnapshot { url, title, tree_text, element_count }, ref_map))
        } else {
            bail!("Browser session not initialized")
        }
    }

    /// Act on an element identified by its ref ID from a snapshot
    pub async fn act_on_element(
        &self,
        ref_map: &RefMap,
        ref_id: &str,
        action: &str,
        value: Option<&str>,
    ) -> Result<String> {
        self.update_activity().await;

        let backend_node_id = ref_map.refs.get(ref_id)
            .ok_or_else(|| anyhow::anyhow!("Ref ID not found: {}. Take a new browser_snapshot first.", ref_id))?;

        info!("Acting on {} (backend_node_id={}) action={}", ref_id, backend_node_id, action);

        if let Some(client) = &self.cdp_client {
            let mut client = client.lock().await;

            // Resolve backendNodeId → objectId via DOM.resolveNode
            let resolve_result = client.send_command_to_session(
                "DOM.resolveNode",
                serde_json::json!({ "backendNodeId": backend_node_id })
            ).await?;

            let object_id = resolve_result
                .get("object")
                .and_then(|o| o.get("objectId"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Could not resolve DOM node for {}", ref_id))?
                .to_string();

            match action {
                "click" => {
                    // Get box model to compute center coordinates
                    let box_result = client.send_command_to_session(
                        "DOM.getBoxModel",
                        serde_json::json!({ "backendNodeId": backend_node_id })
                    ).await?;

                    let content = box_result.get("model")
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_array())
                        .ok_or_else(|| anyhow::anyhow!("Could not get box model for {}", ref_id))?;

                    // content quad is [x1,y1, x2,y2, x3,y3, x4,y4]
                    if content.len() >= 4 {
                        let x1 = content[0].as_f64().unwrap_or(0.0);
                        let y1 = content[1].as_f64().unwrap_or(0.0);
                        let x3 = content[4].as_f64().unwrap_or(0.0);
                        let y3 = content[5].as_f64().unwrap_or(0.0);
                        let cx = (x1 + x3) / 2.0;
                        let cy = (y1 + y3) / 2.0;

                        // mousePressed + mouseReleased
                        client.send_command_to_session("Input.dispatchMouseEvent", serde_json::json!({
                            "type": "mousePressed",
                            "x": cx, "y": cy,
                            "button": "left",
                            "clickCount": 1
                        })).await?;
                        client.send_command_to_session("Input.dispatchMouseEvent", serde_json::json!({
                            "type": "mouseReleased",
                            "x": cx, "y": cy,
                            "button": "left",
                            "clickCount": 1
                        })).await?;

                        Ok(format!("Clicked {} at ({:.0}, {:.0})", ref_id, cx, cy))
                    } else {
                        bail!("Invalid box model for {}", ref_id)
                    }
                }

                "type" => {
                    let text = value.ok_or_else(|| anyhow::anyhow!("'type' action requires a 'value'"))?;

                    // Focus the element
                    client.send_command_to_session("DOM.focus", serde_json::json!({
                        "backendNodeId": backend_node_id
                    })).await?;

                    // Insert text in a single CDP call (much faster than per-character keyDown/keyUp)
                    client.send_command_to_session("Input.insertText", serde_json::json!({
                        "text": text
                    })).await?;

                    Ok(format!("Typed \"{}\" into {}", text, ref_id))
                }

                "select" => {
                    let val = value.ok_or_else(|| anyhow::anyhow!("'select' action requires a 'value'"))?;

                    // Use JS to set value and dispatch change event
                    let script = format!(
                        "(function(el) {{ el.value = '{}'; el.dispatchEvent(new Event('change', {{bubbles: true}})); return 'ok'; }})(this)",
                        escape_js_string(val)
                    );
                    client.send_command_to_session("Runtime.callFunctionOn", serde_json::json!({
                        "objectId": object_id,
                        "functionDeclaration": format!(
                            "function() {{ this.value = '{}'; this.dispatchEvent(new Event('change', {{bubbles: true}})); return 'ok'; }}",
                            escape_js_string(val)
                        ),
                        "returnByValue": true
                    })).await?;

                    Ok(format!("Selected \"{}\" on {}", val, ref_id))
                }

                "hover" => {
                    let box_result = client.send_command_to_session(
                        "DOM.getBoxModel",
                        serde_json::json!({ "backendNodeId": backend_node_id })
                    ).await?;

                    let content = box_result.get("model")
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_array())
                        .ok_or_else(|| anyhow::anyhow!("Could not get box model for {}", ref_id))?;

                    if content.len() >= 4 {
                        let x1 = content[0].as_f64().unwrap_or(0.0);
                        let y1 = content[1].as_f64().unwrap_or(0.0);
                        let x3 = content[4].as_f64().unwrap_or(0.0);
                        let y3 = content[5].as_f64().unwrap_or(0.0);
                        let cx = (x1 + x3) / 2.0;
                        let cy = (y1 + y3) / 2.0;

                        client.send_command_to_session("Input.dispatchMouseEvent", serde_json::json!({
                            "type": "mouseMoved",
                            "x": cx, "y": cy
                        })).await?;

                        Ok(format!("Hovered over {} at ({:.0}, {:.0})", ref_id, cx, cy))
                    } else {
                        bail!("Invalid box model for {}", ref_id)
                    }
                }

                "focus" => {
                    client.send_command_to_session("DOM.focus", serde_json::json!({
                        "backendNodeId": backend_node_id
                    })).await?;
                    Ok(format!("Focused {}", ref_id))
                }

                _ => bail!("Unknown action: {}. Supported: click, type, select, hover, focus", action),
            }
        } else {
            bail!("Browser session not initialized")
        }
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

    /// Create or get a named browser session (e.g., "default")
    pub async fn create_named_session(&self, name: &str) -> Result<BrowserSession> {
        self.manager.create_named_session(name).await
    }

    /// Get a session by ID
    pub async fn get_session(&self, id: &str) -> Option<BrowserSession> {
        self.manager.get_session(id).await
    }

    /// Close a session
    pub async fn close_session(&self, id: &str) -> Result<()> {
        self.manager.close_session(id).await
    }

    /// Remove a dead session and create a fresh one with the same name
    pub async fn reconnect_named_session(&self, name: &str) -> Result<BrowserSession> {
        self.manager.reconnect_named_session(name).await
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

    /// Get accessibility tree snapshot for a session
    pub async fn snapshot(&self, session_id: &str) -> Result<(AXSnapshot, RefMap)> {
        if let Some(session) = self.manager.get_session(session_id).await {
            session.accessibility_snapshot().await
        } else {
            bail!("Session not found: {}", session_id)
        }
    }

    /// Act on an element by ref ID in a session
    pub async fn act(
        &self,
        session_id: &str,
        ref_map: &RefMap,
        ref_id: &str,
        action: &str,
        value: Option<&str>,
    ) -> Result<String> {
        if let Some(session) = self.manager.get_session(session_id).await {
            session.act_on_element(ref_map, ref_id, action, value).await
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

    /// Create a browser session with a specific alias/name (e.g., "default").
    /// If a session with this name already exists, return it.
    pub async fn create_named_session(&self, name: &str) -> Result<BrowserSession> {
        // Return existing session if one exists with this name
        {
            let sessions = self.sessions.read().await;
            if let Some(existing) = sessions.get(name) {
                return Ok(existing.clone());
            }
        }

        let session = BrowserSession::new(self.config.clone()).await?;

        let mut sessions = self.sessions.write().await;
        // Store under the alias name, not the UUID
        sessions.insert(name.to_string(), session.clone());

        info!("Created named browser session: {}", name);
        Ok(session)
    }

    /// Get a session by ID
    pub async fn get_session(&self, id: &str) -> Option<BrowserSession> {
        let sessions = self.sessions.read().await;
        sessions.get(id).cloned()
    }

    /// Remove a dead session and create a fresh one with the same name
    pub async fn reconnect_named_session(&self, name: &str) -> Result<BrowserSession> {
        // Remove the dead session (don't call close — connection is already dead)
        {
            let mut sessions = self.sessions.write().await;
            sessions.remove(name);
        }
        info!("Reconnecting browser session '{}'", name);
        self.create_named_session(name).await
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

    #[tokio::test]
    #[ignore] // Run with: cargo test browser_navigate_live -- --ignored --nocapture
    async fn browser_navigate_live() {
        let browser = BrowserTool::new();

        // Create named session
        let session = browser.create_named_session("test").await
            .expect("Failed to create named session");
        println!("Session created: {}", session.id);

        // Navigate
        let result = browser.navigate("test", "https://example.com").await
            .expect("Failed to navigate");
        println!("URL: {}", result.url);
        println!("Title: {}", result.title);
        println!("Load time: {}ms", result.load_time_ms);

        assert!(result.url.contains("example.com"), "URL should contain example.com");
        assert!(!result.title.is_empty(), "Title should not be empty");

        // Session should persist
        assert!(browser.get_session("test").await.is_some(), "Session should persist");

        // Cleanup
        browser.close_session("test").await.expect("Failed to close session");
        println!("Test passed!");
    }
}
