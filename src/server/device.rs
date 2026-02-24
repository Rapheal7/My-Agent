//! Remote Device Agent System
//!
//! Allows remote devices (e.g., a MacBook) to connect to the server and
//! receive tool calls for execution. The LLM can route tool calls to any
//! connected device via the `switch_device` / `list_devices` tools.
//!
//! Architecture:
//!   Browser (voice) → Cloud Server → LLM
//!                                      ↓ tool calls
//!                           ┌──────────┴──────────┐
//!                           ↓                     ↓
//!                     Server (local)       MacBook Agent
//!                           ↓                     ↓
//!                      tool results          tool results
//!                           └──────────┬──────────┘
//!                                      ↓
//!                                 back to LLM

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{info, warn, debug, error};

/// Message sent from server to a remote device agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceToolRequest {
    /// Unique request ID for correlating responses
    pub request_id: String,
    /// Tool name to execute
    pub tool_name: String,
    /// Tool arguments as JSON
    pub arguments: serde_json::Value,
}

/// Message sent from remote device agent back to server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceToolResponse {
    /// Matches the request_id
    pub request_id: String,
    /// Whether the tool call succeeded
    pub success: bool,
    /// Human-readable result message
    pub message: String,
    /// Optional structured data
    pub data: Option<serde_json::Value>,
}

/// Information about a connected device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Device name (e.g., "MacBook", "Desktop")
    pub name: String,
    /// Available tool names on this device
    pub capabilities: Vec<String>,
    /// Device platform (e.g., "macos", "linux", "windows")
    pub platform: String,
    /// When the device connected
    pub connected_at: chrono::DateTime<chrono::Utc>,
}

/// Internal handle to communicate with a connected device
struct DeviceHandle {
    info: DeviceInfo,
    /// Send tool requests to the device's WebSocket handler
    request_tx: mpsc::Sender<(DeviceToolRequest, oneshot::Sender<DeviceToolResponse>)>,
}

/// Registry of connected remote devices
pub struct DeviceRegistry {
    devices: RwLock<HashMap<String, DeviceHandle>>,
    /// Which device is currently active for tool routing (None = local server)
    active_device: RwLock<Option<String>>,
}

impl DeviceRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            devices: RwLock::new(HashMap::new()),
            active_device: RwLock::new(None),
        })
    }

    /// Register a new device connection
    pub async fn register(
        &self,
        info: DeviceInfo,
        request_tx: mpsc::Sender<(DeviceToolRequest, oneshot::Sender<DeviceToolResponse>)>,
    ) -> String {
        let device_id = info.name.clone();
        info!("Device registered: {} (platform: {}, capabilities: {:?})",
            info.name, info.platform, info.capabilities);

        let mut devices = self.devices.write().await;
        devices.insert(device_id.clone(), DeviceHandle { info, request_tx });
        device_id
    }

    /// Remove a device when it disconnects
    pub async fn unregister(&self, device_id: &str) {
        let mut devices = self.devices.write().await;
        if devices.remove(device_id).is_some() {
            info!("Device unregistered: {}", device_id);
        }

        // Clear active device if it was the one that disconnected
        let mut active = self.active_device.write().await;
        if active.as_deref() == Some(device_id) {
            *active = None;
            info!("Active device cleared (disconnected)");
        }
    }

    /// List all connected devices
    pub async fn list_devices(&self) -> Vec<DeviceInfo> {
        let devices = self.devices.read().await;
        devices.values().map(|h| h.info.clone()).collect()
    }

    /// Set the active device for tool routing
    pub async fn set_active_device(&self, device_name: Option<&str>) -> Result<()> {
        if let Some(name) = device_name {
            let devices = self.devices.read().await;
            if !devices.contains_key(name) {
                let available: Vec<String> = devices.keys().cloned().collect();
                bail!("Device '{}' not connected. Available: {:?}", name, available);
            }
            drop(devices);

            let mut active = self.active_device.write().await;
            *active = Some(name.to_string());
            info!("Active device set to: {}", name);
        } else {
            let mut active = self.active_device.write().await;
            *active = None;
            info!("Active device set to: local server");
        }
        Ok(())
    }

    /// Get the currently active device name (None = local server)
    pub async fn get_active_device(&self) -> Option<String> {
        self.active_device.read().await.clone()
    }

    /// Execute a tool call on a remote device
    /// Returns None if device not found (caller should fall back to local)
    pub async fn execute_remote(
        &self,
        device_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<DeviceToolResponse> {
        let devices = self.devices.read().await;
        let handle = devices.get(device_name)
            .ok_or_else(|| anyhow::anyhow!("Device '{}' not connected", device_name))?;

        let request_id = uuid::Uuid::new_v4().to_string();
        let request = DeviceToolRequest {
            request_id: request_id.clone(),
            tool_name: tool_name.to_string(),
            arguments,
        };

        let (response_tx, response_rx) = oneshot::channel();

        handle.request_tx.send((request, response_tx)).await
            .map_err(|_| anyhow::anyhow!("Device '{}' connection lost", device_name))?;

        // Wait for response with timeout
        match tokio::time::timeout(std::time::Duration::from_secs(60), response_rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => bail!("Device '{}' dropped the request", device_name),
            Err(_) => bail!("Tool call to device '{}' timed out (60s)", device_name),
        }
    }

    /// Check if a tool call should be routed to a remote device
    pub async fn should_route_remote(&self, tool_name: &str) -> Option<String> {
        let active = self.active_device.read().await;
        let device_name = active.as_ref()?;

        let devices = self.devices.read().await;
        let handle = devices.get(device_name.as_str())?;

        // Route if the device has this capability
        if handle.info.capabilities.contains(&tool_name.to_string()) {
            Some(device_name.clone())
        } else {
            None
        }
    }
}

// ─── WebSocket handler for device agent connections ───

use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, State, Query},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};

use super::ServerState;

#[derive(Debug, Deserialize)]
pub struct DeviceConnectParams {
    pub name: String,
    #[serde(default = "default_platform")]
    pub platform: String,
    pub token: String,
}

fn default_platform() -> String {
    "unknown".to_string()
}

/// WebSocket upgrade handler for device agent connections
pub async fn device_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<ServerState>,
    Query(params): Query<DeviceConnectParams>,
) -> Response {
    // Validate JWT token
    match state.auth_state.validate_token(&params.token) {
        Ok(claims) => {
            if claims.token_type != crate::server::auth::TokenType::Access {
                return axum::response::IntoResponse::into_response(
                    (axum::http::StatusCode::UNAUTHORIZED, "Invalid token type")
                );
            }
        }
        Err(_) => {
            return axum::response::IntoResponse::into_response(
                (axum::http::StatusCode::UNAUTHORIZED, "Invalid or expired token")
            );
        }
    }

    let device_name = params.name.clone();
    let platform = params.platform.clone();
    ws.on_upgrade(move |socket| handle_device_socket(socket, state, device_name, platform))
}

async fn handle_device_socket(
    socket: WebSocket,
    state: ServerState,
    device_name: String,
    platform: String,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Wait for the device to send its capability list as the first message
    let capabilities: Vec<String> = match ws_rx.next().await {
        Some(Ok(Message::Text(text))) => {
            match serde_json::from_str::<Vec<String>>(&text) {
                Ok(caps) => caps,
                Err(e) => {
                    error!("Device '{}' sent invalid capabilities: {}", device_name, e);
                    let _ = ws_tx.send(Message::Text(
                        serde_json::json!({"error": "Send capability list as first message"}).to_string().into()
                    )).await;
                    return;
                }
            }
        }
        _ => {
            error!("Device '{}' disconnected before sending capabilities", device_name);
            return;
        }
    };

    let info = DeviceInfo {
        name: device_name.clone(),
        capabilities,
        platform,
        connected_at: chrono::Utc::now(),
    };

    // Channel for sending tool requests to this device
    let (request_tx, mut request_rx) = mpsc::channel::<(DeviceToolRequest, oneshot::Sender<DeviceToolResponse>)>(32);

    // Register the device
    let device_id = state.device_registry.register(info, request_tx).await;

    // Send confirmation
    let _ = ws_tx.send(Message::Text(
        serde_json::json!({"status": "connected", "device_id": device_id}).to_string().into()
    )).await;

    // Track pending requests waiting for responses
    let pending: Arc<RwLock<HashMap<String, oneshot::Sender<DeviceToolResponse>>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let pending_for_rx = pending.clone();

    // Task: forward incoming tool requests to the device via WebSocket
    let send_task = tokio::spawn(async move {
        while let Some((request, response_tx)) = request_rx.recv().await {
            let request_id = request.request_id.clone();
            // Store the response channel
            pending_for_rx.write().await.insert(request_id, response_tx);

            let msg = serde_json::to_string(&request).unwrap();
            if ws_tx.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Task: read responses from device WebSocket
    let pending_for_read = pending.clone();
    let device_name_for_read = device_name.clone();
    let read_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Text(text) => {
                    match serde_json::from_str::<DeviceToolResponse>(&text) {
                        Ok(response) => {
                            let mut p = pending_for_read.write().await;
                            if let Some(tx) = p.remove(&response.request_id) {
                                let _ = tx.send(response);
                            } else {
                                warn!("Device '{}': response for unknown request {}",
                                    device_name_for_read, response.request_id);
                            }
                        }
                        Err(e) => {
                            debug!("Device '{}': non-response message: {}", device_name_for_read, e);
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // Wait for either task to finish (device disconnected)
    tokio::select! {
        _ = send_task => {},
        _ = read_task => {},
    }

    // Clean up
    state.device_registry.unregister(&device_id).await;
    info!("Device '{}' disconnected", device_name);
}
