//! Gateway Daemon Mode - always-on service for receiving messages
//!
//! Integrates soul engine, web server, and messaging listeners
//! into a unified daemon that can receive messages from CLI,
//! messaging platforms, and web.

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Gateway configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GatewayConfig {
    /// HTTP server port
    #[serde(default = "default_port")]
    pub port: u16,
    /// Host to bind to
    #[serde(default = "default_host")]
    pub host: String,
    /// Enable web server
    #[serde(default = "default_true")]
    pub enable_web: bool,
    /// Enable messaging integrations
    #[serde(default)]
    pub enable_messaging: bool,
    /// Auto-start soul engine
    #[serde(default = "default_true")]
    pub auto_start_soul: bool,
}

fn default_port() -> u16 {
    18789
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            host: default_host(),
            enable_web: true,
            enable_messaging: false,
            auto_start_soul: true,
        }
    }
}

/// Gateway state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayState {
    Stopped,
    Starting,
    Running,
    Stopping,
}

impl std::fmt::Display for GatewayState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GatewayState::Stopped => write!(f, "Stopped"),
            GatewayState::Starting => write!(f, "Starting"),
            GatewayState::Running => write!(f, "Running"),
            GatewayState::Stopping => write!(f, "Stopping"),
        }
    }
}

/// Gateway statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct GatewayStats {
    pub state: String,
    pub uptime_secs: u64,
    pub web_enabled: bool,
    pub messaging_enabled: bool,
    pub soul_running: bool,
    pub port: u16,
}

/// The gateway daemon
pub struct Gateway {
    config: GatewayConfig,
    state: Arc<Mutex<GatewayState>>,
    started_at: Arc<Mutex<Option<std::time::Instant>>>,
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,
}

impl Gateway {
    /// Create a new gateway with default config
    pub fn new() -> Self {
        Self {
            config: GatewayConfig::default(),
            state: Arc::new(Mutex::new(GatewayState::Stopped)),
            started_at: Arc::new(Mutex::new(None)),
            shutdown_tx: None,
        }
    }

    /// Create with custom config
    pub fn with_config(config: GatewayConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(GatewayState::Stopped)),
            started_at: Arc::new(Mutex::new(None)),
            shutdown_tx: None,
        }
    }

    /// Start the gateway daemon
    pub async fn start(&mut self) -> Result<()> {
        let mut state = self.state.lock().await;
        if *state != GatewayState::Stopped {
            anyhow::bail!("Gateway is not stopped (current state: {:?})", *state);
        }
        *state = GatewayState::Starting;
        drop(state);

        info!("Starting gateway daemon on {}:{}...", self.config.host, self.config.port);

        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx.clone());

        // Record start time
        *self.started_at.lock().await = Some(std::time::Instant::now());

        // Start soul engine if configured
        if self.config.auto_start_soul {
            info!("Starting soul engine...");
            match crate::soul::engine::start_soul().await {
                Ok(()) => info!("Soul engine started"),
                Err(e) => warn!("Failed to start soul engine: {}", e),
            }
        }

        // Start web server if enabled
        if self.config.enable_web {
            let port = self.config.port;
            let host = self.config.host.clone();
            let mut shutdown_rx = shutdown_tx.subscribe();

            tokio::spawn(async move {
                info!("Starting web server on {}:{}...", host, port);

                // Use the existing server module
                let server_future = crate::server::start(&host, port, false, None, None);

                tokio::select! {
                    result = server_future => {
                        if let Err(e) = result {
                            warn!("Web server error: {}", e);
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Web server shutting down");
                    }
                }
            });
        }

        // Start messaging listeners if enabled
        if self.config.enable_messaging {
            let mut shutdown_rx = shutdown_tx.subscribe();
            tokio::spawn(async move {
                info!("Starting messaging listeners...");
                // Messaging integration would connect to configured platforms
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = shutdown_rx.recv() => {},
                }
                info!("Messaging listeners stopped");
            });
        }

        *self.state.lock().await = GatewayState::Running;
        info!(
            "Gateway daemon running on {}:{} (web: {}, messaging: {}, soul: {})",
            self.config.host,
            self.config.port,
            self.config.enable_web,
            self.config.enable_messaging,
            self.config.auto_start_soul,
        );

        Ok(())
    }

    /// Stop the gateway daemon
    pub async fn stop(&mut self) -> Result<()> {
        let mut state = self.state.lock().await;
        if *state != GatewayState::Running {
            return Ok(());
        }
        *state = GatewayState::Stopping;
        drop(state);

        info!("Stopping gateway daemon...");

        // Send shutdown signal
        if let Some(tx) = &self.shutdown_tx {
            let _ = tx.send(());
        }

        // Stop soul engine
        if let Err(e) = crate::soul::engine::stop_soul().await {
            warn!("Error stopping soul engine: {}", e);
        }

        *self.state.lock().await = GatewayState::Stopped;
        info!("Gateway daemon stopped");
        Ok(())
    }

    /// Get gateway statistics
    pub async fn stats(&self) -> GatewayStats {
        let state = self.state.lock().await;
        let uptime = self.started_at.lock().await
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);

        let soul_running = crate::soul::get_soul_stats().await.is_some();

        GatewayStats {
            state: format!("{}", *state),
            uptime_secs: uptime,
            web_enabled: self.config.enable_web,
            messaging_enabled: self.config.enable_messaging,
            soul_running,
            port: self.config.port,
        }
    }

    /// Run the gateway daemon (blocks until Ctrl+C)
    pub async fn run(&mut self) -> Result<()> {
        self.start().await?;

        println!("Gateway daemon is running.");
        println!("  Web server: http://{}:{}", self.config.host, self.config.port);
        println!("  API status: http://{}:{}/api/status", self.config.host, self.config.port);
        println!();
        println!("Press Ctrl+C to stop.");

        // Wait for Ctrl+C
        tokio::signal::ctrl_c().await?;
        println!("\nShutting down...");

        self.stop().await?;
        Ok(())
    }
}

impl Default for Gateway {
    fn default() -> Self {
        Self::new()
    }
}
