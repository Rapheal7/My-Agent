//! Web server module with JWT authentication

pub mod http;
pub mod auth;
pub mod realtime_voice;
pub mod device;

use anyhow::{Result, Context};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::{Html, Response},
    routing::{get, post},
    Router,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use axum::middleware;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{info, debug};

use crate::agent::llm::OpenRouterClient;
use crate::config::Config;
use crate::server::auth::{AuthState, AuthConfig};

const OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// Voice processing mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VoiceMode {
    /// Use local Whisper + Piper TTS + OpenRouter LLM
    Local,
    /// Text-only mode (no voice)
    TextOnly,
}

/// Shared server state
#[derive(Clone)]
pub struct ServerState {
    pub config: Arc<Config>,
    pub http_client: Client,
    pub voice_mode: VoiceMode,
    pub auth_state: Arc<AuthState>,
    pub device_registry: Arc<device::DeviceRegistry>,
}

/// WebSocket message types
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WsMessage {
    #[serde(rename = "chat")]
    Chat {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tts: Option<bool>,
    },
    #[serde(rename = "voice")]
    Voice {
        audio_data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_realtime: Option<bool>,
    },
    #[serde(rename = "transcribed")]
    Transcribed { text: String },
    #[serde(rename = "response")]
    Response { content: String },
    #[serde(rename = "audio")]
    Audio { audio_data: String },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "status")]
    Status { message: String },
}

/// Start the web server
pub async fn start(
    host: &str,
    port: u16,
    https: bool,
    cert: Option<String>,
    key: Option<String>,
) -> Result<()> {
    let config = Config::load()?;

    // Determine voice mode
    let voice_mode = if check_ollama().await {
        VoiceMode::Local
    } else {
        VoiceMode::TextOnly
    };

    // Create auth state
    let auth_config = AuthConfig {
        jwt_secret: config.auth.jwt_secret.clone().unwrap_or_else(|| auth::generate_jwt_secret()),
        access_token_expiry_minutes: config.auth.access_token_expiry_minutes,
        refresh_token_expiry_days: config.auth.refresh_token_expiry_days,
        max_login_attempts: config.auth.max_login_attempts,
        lockout_duration_minutes: config.auth.lockout_duration_minutes,
        require_https: config.security.require_https,
    };
    let auth_state = AuthState::new(auth_config);

    // Create server state
    let state = ServerState {
        config: Arc::new(config),
        http_client: Client::new(),
        voice_mode,
        auth_state,
        device_registry: device::DeviceRegistry::new(),
    };

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;

    // Build CORS layer
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Protected routes (require JWT auth)
    let protected = Router::new()
        .route("/api/chat", post(http::chat_handler))
        .route("/api/devices", get(http::list_devices_handler))
        .route("/api/devices/switch", post(http::switch_device_handler))
        .route("/ws", get(websocket_handler))
        .route("/voice-ws", get(voice_websocket_handler))
        .route("/realtime-voice-ws", get(realtime_voice::ws_handler))
        .layer(middleware::from_fn_with_state(
            state.auth_state.clone(),
            auth::auth_middleware,
        ));

    // Public routes (no auth required)
    // Note: /ws/device-agent uses token in query params (WebSocket can't set headers)
    let public = Router::new()
        .route("/", get(index_page))
        .route("/simple-voice", get(simple_voice_page))
        .route("/voice-chat", get(voice_chat_page))
        .route("/api/auth/login", post(http::login_handler))
        .route("/api/auth/refresh", post(http::refresh_handler))
        .route("/api/auth/logout", post(http::logout_handler))
        .route("/api/status", get(http::status_handler))
        .route("/ws/device-agent", get(device::device_ws_handler));

    // Merge protected and public routes
    let app = Router::new()
        .merge(protected)
        .merge(public)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Print startup message
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("     My Agent Server Starting");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("✓ Server binding to: {}", addr);

    if https {
        println!("✓ HTTPS enabled");
    } else {
        println!("⚠ HTTPS disabled");
    }

    match voice_mode {
        VoiceMode::Local => {
            println!("✓ Voice mode: Local (Whisper + Piper TTS)");
        }
        VoiceMode::TextOnly => {
            println!("✓ Voice mode: Text-only");
            println!("  Tip: Install Ollama for voice support");
        }
    }

    println!("✓ JWT authentication enabled");
    println!();
    println!("🚀 Listening on http{}://{}", if https { "s" } else { "" }, addr);
    println!();

    // HTTPS mode
    if https {
        if let (Some(cert_path), Some(key_path)) = (cert, key) {
            let cert_data = tokio::fs::read(&cert_path).await
                .context("Failed to read certificate file")?;
            let key_data = tokio::fs::read(&key_path).await
                .context("Failed to read key file")?;

            let config = axum_server::tls_rustls::RustlsConfig::from_pem(cert_data, key_data).await?;
            axum_server::bind_rustls(addr, config).serve(app.into_make_service()).await?;
            return Ok(());
        }
    }

    // HTTP mode
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;

    Ok(())
}

/// Check if Ollama is available for local voice
async fn check_ollama() -> bool {
    match reqwest::get(format!("{}/api/version", OLLAMA_URL)).await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Handler for WebSocket connections
async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<ServerState>,
) -> Response {
    ws.on_upgrade(|socket| handle_websocket(socket, state))
}

/// Handle WebSocket connection
async fn handle_websocket(mut ws: WebSocket, state: ServerState) {
    println!("🔗 WebSocket connected");

    // Simple chat loop
    while let Some(Ok(msg)) = ws.recv().await {
        match msg {
            Message::Text(text) => {
                // Parse message
                match serde_json::from_str::<WsMessage>(&text) {
                    Ok(WsMessage::Chat { content, tts: _ }) => {
                        // Process chat message
                        match process_chat(&content, &state).await {
                            Ok(response) => {
                                let reply = WsMessage::Response {
                                    content: response,
                                };
                                let _ = ws.send(Message::Text(
                                    serde_json::to_string(&reply).unwrap_or_default().into()
                                )).await;
                            }
                            Err(e) => {
                                let error = WsMessage::Error {
                                    message: e.to_string(),
                                };
                                let _ = ws.send(Message::Text(
                                    serde_json::to_string(&error).unwrap_or_default().into()
                                )).await;
                            }
                        }
                    }
                    Ok(WsMessage::Voice { audio_data, is_realtime: _ }) => {
                        // Decode audio
                        if let Ok(audio_bytes) = BASE64.decode(&audio_data) {
                            // Transcribe using local Whisper
                            match process_voice(&audio_bytes, &state).await {
                                Ok(response) => {
                                    let reply = WsMessage::Response {
                                        content: response,
                                    };
                                    let _ = ws.send(Message::Text(
                                        serde_json::to_string(&reply).unwrap_or_default().into()
                                    )).await;
                                }
                                Err(e) => {
                                    let error = WsMessage::Error {
                                        message: format!("Voice processing error: {}", e),
                                    };
                                    let _ = ws.send(Message::Text(
                                        serde_json::to_string(&error).unwrap_or_default().into()
                                    )).await;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Message::Close(_) => {
                println!("👋 WebSocket disconnected");
                break;
            }
            _ => {}
        }
    }
}

/// Process chat message
async fn process_chat(content: &str, state: &ServerState) -> Result<String> {
    // Create OpenRouter client
    let client = OpenRouterClient::from_keyring()?;

    // Send to LLM
    let messages = vec![crate::agent::llm::ChatMessage {
        role: Some(serde_json::json!("user")),
        content: Some(serde_json::json!(content)),
        reasoning_details: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        reasoning: None,
        refusal: None,
    }];

    let response = client.complete(
        "anthropic/claude-3.5-sonnet",
        messages,
        Some(2048)
    ).await?;

    Ok(response)
}

/// Process voice input
async fn process_voice(audio_bytes: &[u8], state: &ServerState) -> Result<String> {
    tracing::info!("🎵 Processing audio: {} bytes", audio_bytes.len());
    // Transcribe using Whisper
    let transcribed = crate::voice::whisper::transcribe_audio(audio_bytes).await?;

    // Send transcription as chat
    process_chat(&transcribed, state).await
}

/// Handler for the index page
async fn index_page() -> Html<&'static str> {
    Html(r#"<!DOCTYPE html>
<html>
<head>
    <title>My Agent Server</title>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 20px;
            background: #1a1a1a;
            color: #e0e0e0;
        }
        h1 { color: #4CAF50; }
        .status {
            background: #2a2a2a;
            padding: 15px;
            border-radius: 8px;
            margin: 20px 0;
        }
        .endpoint {
            background: #333;
            padding: 10px;
            margin: 10px 0;
            border-radius: 4px;
            font-family: monospace;
        }
    </style>
</head>
<body>
    <h1>✅ My Agent Server Running</h1>
    <div class="status">
        <p>Server is active and ready to accept requests.</p>
        <p>JWT Authentication is enabled. Use /api/auth/login to get a token.</p>
    </div>
    <h2>API Endpoints:</h2>
    <div class="endpoint">POST /api/auth/login - Authenticate and get JWT token</div>
    <div class="endpoint">POST /api/auth/refresh - Refresh access token</div>
    <div class="endpoint">POST /api/auth/logout - Revoke current token</div>
    <div class="endpoint">POST /api/chat - Send chat messages</div>
    <div class="endpoint">GET /api/status - Server status</div>
    <div class="endpoint">GET /ws - WebSocket for real-time chat</div>
</body>
</html>"#)
}

/// Send status message over WebSocket
async fn send_status(ws_tx: &mut tokio::sync::mpsc::Sender<String>, message: &str) -> Result<()> {
    let msg = WsMessage::Status {
        message: message.to_string(),
    };
    ws_tx.send(serde_json::to_string(&msg)?).await
        .map_err(|e| anyhow::anyhow!("Failed to send: {}", e))?;
    Ok(())
}

/// Send error message over WebSocket
async fn send_error(ws_tx: &mut tokio::sync::mpsc::Sender<String>, message: &str) -> Result<()> {
    let msg = WsMessage::Error {
        message: message.to_string(),
    };
    ws_tx.send(serde_json::to_string(&msg)?).await
        .map_err(|e| anyhow::anyhow!("Failed to send: {}", e))?;
    Ok(())
}

/// Get current timestamp
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Serve the voice-chat HTML page (mobile real-time voice UI)
async fn voice_chat_page() -> (axum::http::HeaderMap, Html<String>) {
    let html_content = tokio::fs::read_to_string("/home/rapheal/Projects/my-agent/src/server/voice-chat.html").await
        .unwrap_or_else(|_| "<html><body>voice-chat.html not found</body></html>".to_string());
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("cache-control", "no-cache, no-store, must-revalidate".parse().unwrap());
    headers.insert("pragma", "no-cache".parse().unwrap());
    (headers, Html(html_content))
}

/// Serve the simple-voice HTML page
async fn simple_voice_page() -> Html<String> {
    // Read the simple-voice.html file from project root
    let html_content = tokio::fs::read_to_string("/home/rapheal/Projects/my-agent/simple-voice.html").await
        .unwrap_or_else(|_| "<html><body>simple-voice.html not found</body></html>".to_string());
    
    Html(html_content)
}

/// Voice WebSocket handler
async fn voice_websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<ServerState>,
) -> Response {
    ws.on_upgrade(|socket| handle_voice_websocket(socket, state))
}

/// Handle voice WebSocket connection
async fn handle_voice_websocket(mut ws: WebSocket, state: ServerState) {
    println!("🔊 Voice WebSocket connected");
    
    info!("Voice WebSocket connected from client");
    
    // Buffer for accumulating audio
    let mut audio_buffer: Vec<u8> = Vec::new();
    let mut last_chunk_time = std::time::Instant::now();
    
    while let Some(Ok(msg)) = ws.recv().await {
        match msg {
            Message::Text(text) => {
                match serde_json::from_str::<WsMessage>(&text) {
                    Ok(WsMessage::Voice { audio_data, is_realtime }) => {
                        let is_rt = is_realtime.unwrap_or(false);
                        
                        // Decode base64 audio
                        if let Ok(audio_bytes) = BASE64.decode(&audio_data) {
                            // Accumulate audio in buffer
                            audio_buffer.extend(&audio_bytes);
                            last_chunk_time = std::time::Instant::now();
                            
                            // For real-time mode, process every few chunks or after silence
                            // For now, just accumulate and let silence detection work
                            info!("Received {} bytes, buffer now {} bytes (realtime: {})", 
                                  audio_bytes.len(), audio_buffer.len(), is_rt);
                            
                            // After a certain buffer size or time, process it
                            // This prevents too many API calls
                            let should_process = audio_buffer.len() > 16000; // ~0.5 seconds at 16kHz
                            
                            if should_process {
                                info!("Processing accumulated audio: {} bytes", audio_buffer.len());
                                
                                // Clone buffer for processing
                                let audio_to_process = audio_buffer.clone();
                                audio_buffer.clear();
                                
                                // Send "processing" status to client
                                let status = WsMessage::Status {
                                    message: "Processing speech...".to_string(),
                                };
                                let _ = ws.send(Message::Text(
                                    serde_json::to_string(&status).unwrap_or_default().into()
                                )).await;
                                
                                match process_voice(&audio_to_process, &state).await {
                                    Ok(response) => {
                                        let reply = WsMessage::Response {
                                            content: response,
                                        };
                                        let _ = ws.send(Message::Text(
                                            serde_json::to_string(&reply).unwrap_or_default().into()
                                        )).await;
                                    }
                                    Err(e) => {
                                        let error = WsMessage::Error {
                                            message: format!("Voice processing error: {}", e),
                                        };
                                        let _ = ws.send(Message::Text(
                                            serde_json::to_string(&error).unwrap_or_default().into()
                                        )).await;
                                    }
                                }
                            }
                        }
                    }
                    Ok(WsMessage::Chat { content, tts: _ }) => {
                        // Handle text chat through voice WebSocket
                        match process_chat(&content, &state).await {
                            Ok(response) => {
                                let reply = WsMessage::Response {
                                    content: response,
                                };
                                let _ = ws.send(Message::Text(
                                    serde_json::to_string(&reply).unwrap_or_default().into()
                                )).await;
                            }
                            Err(e) => {
                                let error = WsMessage::Error {
                                    message: e.to_string(),
                                };
                                let _ = ws.send(Message::Text(
                                    serde_json::to_string(&error).unwrap_or_default().into()
                                )).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Message::Close(_) => {
                println!("👋 Voice WebSocket disconnected");
                break;
            }
            _ => {}
        }
    }
}
