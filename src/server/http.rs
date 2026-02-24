//! HTTP server handlers with JWT authentication

use anyhow::{Result, Context};
use axum::{
    extract::{State, Json},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::server::{ServerState, VoiceMode};
use crate::server::auth::{
    LoginRequest, LoginResponse, RefreshRequest, LogoutRequest,
    Claims, TokenType
};
use crate::agent::llm::OpenRouterClient;

/// Chat request
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
}

/// Chat response
#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub response: String,
    pub conversation_id: Option<String>,
}

/// Status response
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub status: String,
    pub version: String,
    pub voice_mode: String,
    pub auth_enabled: bool,
}

/// Voice process request
#[derive(Debug, Deserialize)]
pub struct VoiceProcessRequest {
    pub audio_data: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
}

/// Voice process response
#[derive(Debug, Serialize)]
pub struct VoiceProcessResponse {
    pub transcribed_text: String,
    pub response_text: String,
    pub response_audio: Option<String>,
    pub conversation_id: Option<String>,
}

/// JWT Login handler — verifies password before issuing tokens
pub async fn login_handler(
    State(state): State<ServerState>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    // Check lockout
    if let Some(remaining) = state.auth_state.is_locked(&req.username) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "error": "Account temporarily locked due to too many failed attempts",
                "retry_after_seconds": remaining.num_seconds()
            }))
        ).into_response();
    }

    // Get stored password hash
    let stored_hash = match crate::security::keyring::get_server_password_hash() {
        Ok(hash) => hash,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "Server password not configured. Run 'my-agent config --set-password' first."
                }))
            ).into_response();
        }
    };

    // Verify password
    match crate::server::auth::verify_password(&req.password, &stored_hash) {
        Ok(true) => {
            // Password correct — clear failed attempts and issue tokens
            state.auth_state.clear_login_attempts(&req.username);
        }
        Ok(false) => {
            let _ = state.auth_state.record_failed_login(&req.username);
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Invalid credentials" }))
            ).into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Password verification failed",
                    "details": e.to_string()
                }))
            ).into_response();
        }
    }

    let permissions = vec!["read".to_string(), "write".to_string()];

    let access_token = match state.auth_state.generate_access_token(&req.username, &permissions) {
        Ok(token) => token,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to generate access token",
                    "details": e.to_string()
                }))
            ).into_response();
        }
    };

    let refresh_token = match state.auth_state.generate_refresh_token(&req.username) {
        Ok(token) => token,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to generate refresh token",
                    "details": e.to_string()
                }))
            ).into_response();
        }
    };

    let response = LoginResponse {
        access_token,
        refresh_token,
        token_type: "Bearer".to_string(),
        expires_in: state.config.auth.access_token_expiry_minutes * 60,
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// JWT Refresh handler
pub async fn refresh_handler(
    State(state): State<ServerState>,
    Json(req): Json<RefreshRequest>,
) -> impl IntoResponse {
    let claims = match state.auth_state.validate_token(&req.refresh_token) {
        Ok(claims) => claims,
        Err(e) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "error": "Invalid refresh token",
                    "details": e.to_string()
                }))
            ).into_response();
        }
    };
    
    if claims.token_type != TokenType::Refresh {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid token type" }))
        ).into_response();
    }
    
    let _ = state.auth_state.revoke_token(&claims.jti);
    
    let permissions = vec!["read".to_string(), "write".to_string()];
    
    let access_token = match state.auth_state.generate_access_token(&claims.sub, &permissions) {
        Ok(token) => token,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to generate access token",
                    "details": e.to_string()
                }))
            ).into_response();
        }
    };
    
    let refresh_token = match state.auth_state.generate_refresh_token(&claims.sub) {
        Ok(token) => token,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to generate refresh token",
                    "details": e.to_string()
                }))
            ).into_response();
        }
    };
    
    let response = LoginResponse {
        access_token,
        refresh_token,
        token_type: "Bearer".to_string(),
        expires_in: state.config.auth.access_token_expiry_minutes * 60,
    };
    
    (StatusCode::OK, Json(response)).into_response()
}

/// JWT Logout handler
pub async fn logout_handler(
    State(state): State<ServerState>,
    Json(req): Json<LogoutRequest>,
) -> impl IntoResponse {
    match state.auth_state.extract_jti(&req.token) {
        Ok(jti) => {
            let _ = state.auth_state.revoke_token(&jti);
            (StatusCode::OK, Json(json!({ "message": "Logged out successfully" }))).into_response()
        }
        Err(e) => {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "Invalid token",
                    "details": e.to_string()
                }))
            ).into_response()
        }
    }
}

/// Status handler
pub async fn status_handler(
    State(state): State<ServerState>,
) -> impl IntoResponse {
    let voice_mode_str = match state.voice_mode {
        VoiceMode::Local => "local".to_string(),
        VoiceMode::TextOnly => "text-only".to_string(),
    };
    
    let response = StatusResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        voice_mode: voice_mode_str,
        auth_enabled: true,
    };
    
    (StatusCode::OK, Json(response)).into_response()
}

/// Chat handler (requires JWT auth)
pub async fn chat_handler(
    State(state): State<ServerState>,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    let client = match OpenRouterClient::from_keyring() {
        Ok(client) => client,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "OpenRouter client not available",
                    "details": e.to_string()
                }))
            ).into_response();
        }
    };
    
    let messages = vec![crate::agent::llm::ChatMessage {
        role: Some(serde_json::json!("user")),
        content: Some(serde_json::json!(req.message)),
        reasoning_details: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        reasoning: None,
        refusal: None,
    }];
    
    let model = &state.config.openrouter.default_model;
    
    match client.complete(model, messages, Some(2048)).await {
        Ok(response) => {
            let chat_response = ChatResponse {
                response,
                conversation_id: req.conversation_id,
            };
            (StatusCode::OK, Json(chat_response)).into_response()
        }
        Err(e) => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to get AI response",
                    "details": e.to_string()
                }))
            ).into_response()
        }
    }
}

/// Voice process handler
pub async fn voice_process_handler(
    State(state): State<ServerState>,
    Json(req): Json<VoiceProcessRequest>,
) -> impl IntoResponse {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    
    let audio_bytes = match BASE64.decode(&req.audio_data) {
        Ok(bytes) => bytes,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "Invalid audio data",
                    "details": e.to_string()
                }))
            ).into_response();
        }
    };
    
    let transcribed = match transcribe_audio(&audio_bytes).await {
        Ok(text) => text,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Transcription failed",
                    "details": e.to_string()
                }))
            ).into_response();
        }
    };
    
    if transcribed.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "No speech detected" }))
        ).into_response();
    }
    
    let client = match OpenRouterClient::from_keyring() {
        Ok(client) => client,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "OpenRouter client not available",
                    "details": e.to_string()
                }))
            ).into_response();
        }
    };
    
    let messages = vec![crate::agent::llm::ChatMessage {
        role: Some(serde_json::json!("user")),
        content: Some(serde_json::json!(&transcribed)),
        reasoning_details: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        reasoning: None,
        refusal: None,
    }];
    
    let model = &state.config.openrouter.default_model;
    
    let llm_response = match client.complete(model, messages, Some(2048)).await {
        Ok(response) => response,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to get AI response",
                    "details": e.to_string()
                }))
            ).into_response();
        }
    };
    
    let response_audio = match synthesize(&llm_response).await {
        Ok(audio) => Some(BASE64.encode(&audio)),
        Err(_) => None,
    };
    
    let response = VoiceProcessResponse {
        transcribed_text: transcribed,
        response_text: llm_response,
        response_audio,
        conversation_id: req.conversation_id,
    };
    
    (StatusCode::OK, Json(response)).into_response()
}

/// Stub transcribe function
async fn transcribe_audio(audio: &[u8]) -> anyhow::Result<String> {
    Ok("[Transcription not yet implemented]".to_string())
}

/// Stub synthesize function
async fn synthesize(text: &str) -> anyhow::Result<Vec<u8>> {
    Ok(vec![])
}

// ─── Device routing handlers ───

/// List connected remote devices
pub async fn list_devices_handler(
    State(state): State<ServerState>,
) -> impl IntoResponse {
    let devices = state.device_registry.list_devices().await;
    let active = state.device_registry.get_active_device().await;
    (StatusCode::OK, Json(json!({
        "devices": devices,
        "active_device": active,
        "local": active.is_none(),
    }))).into_response()
}

/// Switch request body
#[derive(Debug, Deserialize)]
pub struct SwitchDeviceRequest {
    /// Device name to switch to, or null/empty for local server
    pub device: Option<String>,
}

/// Switch active device for tool routing
pub async fn switch_device_handler(
    State(state): State<ServerState>,
    Json(req): Json<SwitchDeviceRequest>,
) -> impl IntoResponse {
    let target = req.device.as_deref().filter(|s| !s.is_empty());

    match state.device_registry.set_active_device(target).await {
        Ok(()) => {
            let label = target.unwrap_or("local server");
            (StatusCode::OK, Json(json!({
                "status": "switched",
                "active_device": label,
            }))).into_response()
        }
        Err(e) => {
            (StatusCode::BAD_REQUEST, Json(json!({
                "error": e.to_string(),
            }))).into_response()
        }
    }
}
