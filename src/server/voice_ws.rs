//! Simple Voice WebSocket Handler
//!
//! Bridges the simple-voice HTML frontend with the Rust voice pipeline.
//! Provides real-time STT -> LLM -> TTS without requiring LiveKit.
//!
//! Pipeline: Audio → Whisper STT → OpenRouter LLM → Piper TTS → Audio

use anyhow::{Result, Context};
use axum::{
    extract::ws::{WebSocket, Message as WsMessage, WebSocketUpgrade},
    response::IntoResponse,
    extract::State,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, debug, error};
use futures::{sink::SinkExt, stream::StreamExt};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

// Helper to convert String to WebSocket text message
fn ws_text(msg: String) -> WsMessage {
    ws_text(msg.into())
}

use crate::server::ServerState;
use crate::voice::whisper::{WhisperEngine, WhisperConfig};
use crate::voice::tts::{TtsEngine, TtsConfig};
use crate::agent::llm::OpenRouterClient;

/// Voice WebSocket message from client
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum VoiceClientMessage {
    #[serde(rename = "audio")]
    Audio {
        /// Base64 encoded audio data (PCM or WAV)
        data: String,
        /// Sample rate of audio (default: 16000)
        #[serde(default = "default_sample_rate")]
        sample_rate: u32,
    },
    #[serde(rename = "voice")]
    Voice {
        /// Legacy audio_data field for compatibility
        audio_data: String
    },
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "init")]
    Init {
        /// Preferred whisper model size (tiny, base, small)
        #[serde(default = "default_whisper_model")]
        whisper_model: String,
        /// Enable TTS response
        #[serde(default = "default_true")]
        tts: bool,
    },
}

fn default_sample_rate() -> u32 { 16000 }
fn default_whisper_model() -> String { "base".to_string() }
fn default_true() -> bool { true }

/// Voice WebSocket message to client
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum VoiceServerMessage {
    #[serde(rename = "status")]
    Status { text: String },
    #[serde(rename = "transcription")]
    Transcription {
        text: String,
        confidence: f32,
    },
    #[serde(rename = "response")]
    Response {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        transcript: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        audio: Option<String>,
    },
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "error")]
    Error { text: String },
    #[serde(rename = "ready")]
    Ready {
        whisper_model: String,
        tts_enabled: bool,
    },
}

/// Voice processing session state
struct VoiceSession {
    whisper: Option<Arc<Mutex<WhisperEngine>>>,
    tts: Option<Arc<Mutex<TtsEngine>>>,
    tts_enabled: bool,
    whisper_model: String,
}

impl VoiceSession {
    fn new() -> Self {
        Self {
            whisper: None,
            tts: None,
            tts_enabled: true,
            whisper_model: "base".to_string(),
        }
    }

    /// Initialize Whisper STT engine
    async fn init_whisper(&mut self, model_size: &str) -> Result<()> {
        info!("Initializing Whisper with model: {}", model_size);

        let config = WhisperConfig::with_model_size(model_size);

        match WhisperEngine::with_config(config) {
            Ok(engine) => {
                self.whisper = Some(Arc::new(Mutex::new(engine)));
                self.whisper_model = model_size.to_string();
                info!("Whisper initialized successfully");
                Ok(())
            }
            Err(e) => {
                error!("Failed to initialize Whisper: {}", e);
                Err(e)
            }
        }
    }

    /// Initialize TTS engine
    async fn init_tts(&mut self) -> Result<()> {
        info!("Initializing TTS engine");

        let config = TtsConfig::default();

        match TtsEngine::with_config(config) {
            Ok(engine) => {
                self.tts = Some(Arc::new(Mutex::new(engine)));
                info!("TTS initialized successfully");
                Ok(())
            }
            Err(e) => {
                error!("Failed to initialize TTS: {}", e);
                self.tts_enabled = false;
                Ok(())
            }
        }
    }

    /// Transcribe audio using Whisper
    async fn transcribe(&self, audio_data: &[f32]) -> Result<(String, f32)> {
        if let Some(ref whisper) = self.whisper {
            let whisper = whisper.lock().await;
            let result = whisper.transcribe(audio_data)
                .context("Whisper transcription failed")?;

            let confidence = if result.segments.is_empty() {
                0.0
            } else {
                result.segments.iter()
                    .map(|s| s.probability)
                    .sum::<f32>() / result.segments.len() as f32
            };

            Ok((result.text, confidence))
        } else {
            anyhow::bail!("Whisper not initialized")
        }
    }

    /// Synthesize text to speech
    async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        if !self.tts_enabled {
            anyhow::bail!("TTS not enabled")
        }

        if let Some(ref tts) = self.tts {
            let tts = tts.lock().await;
            let result = tts.synthesize(text)
                .context("TTS synthesis failed")?;

            let bytes: Vec<u8> = result.samples.iter()
                .flat_map(|&s| {
                    let s = (s * 32767.0) as i16;
                    s.to_le_bytes().to_vec()
                })
                .collect();

            Ok(bytes)
        } else {
            anyhow::bail!("TTS not initialized")
        }
    }
}

/// Handle voice WebSocket connection
pub async fn voice_websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<ServerState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_voice_socket(socket, state))
}

/// Handle the WebSocket connection
async fn handle_voice_socket(socket: WebSocket, state: ServerState) {
    info!("Voice WebSocket connected");

    let (mut sender, mut receiver) = socket.split();
    let mut session = VoiceSession::new();

    // Send initial status
    let initial_msg = serde_json::to_string(&VoiceServerMessage::Status {
        text: "Connected. Send 'init' to initialize voice engines.".to_string()
    }).unwrap_or_default();

    if sender.send(ws_text(initial_msg)).await.is_err() {
        error!("Failed to send initial status");
        return;
    }

    // Process incoming messages
    while let Some(msg_result) = receiver.next().await {
        match msg_result {
            Ok(WsMessage::Text(text)) => {
                match serde_json::from_str::<VoiceClientMessage>(&text) {
                    Ok(VoiceClientMessage::Init { whisper_model, tts }) => {
                        info!("Initializing voice session: whisper={}, tts={}", whisper_model, tts);

                        let status_msg = serde_json::to_string(&VoiceServerMessage::Status {
                            text: format!("Loading Whisper '{}' model...", whisper_model)
                        }).unwrap_or_default();
                        let _ = sender.send(ws_text(status_msg)).await;

                        match session.init_whisper(&whisper_model).await {
                            Ok(_) => {
                                session.tts_enabled = tts;

                                if tts {
                                    let _ = session.init_tts().await;
                                }

                                let ready_msg = serde_json::to_string(&VoiceServerMessage::Ready {
                                    whisper_model: whisper_model.clone(),
                                    tts_enabled: session.tts.is_some(),
                                }).unwrap_or_default();
                                let _ = sender.send(ws_text(ready_msg)).await;
                            }
                            Err(e) => {
                                let error_msg = serde_json::to_string(&VoiceServerMessage::Error {
                                    text: format!("Failed to load Whisper: {}", e)
                                }).unwrap_or_default();
                                let _ = sender.send(ws_text(error_msg)).await;
                            }
                        }
                    }
                    Ok(VoiceClientMessage::Audio { data, sample_rate: _ }) => {
                        if session.whisper.is_none() {
                            let _ = session.init_whisper("base").await;
                            if session.tts_enabled {
                                let _ = session.init_tts().await;
                            }
                        }

                        let audio_bytes = match BASE64.decode(&data) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                error!("Failed to decode base64 audio: {}", e);
                                let error_msg = serde_json::to_string(&VoiceServerMessage::Error {
                                    text: "Invalid audio data".to_string()
                                }).unwrap_or_default();
                                let _ = sender.send(ws_text(error_msg)).await;
                                continue;
                            }
                        };

                        let audio_f32: Vec<f32> = audio_bytes.chunks_exact(2)
                            .map(|chunk| {
                                let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as f32;
                                sample / 32768.0
                            })
                            .collect();

                        let status_msg = serde_json::to_string(&VoiceServerMessage::Status {
                            text: "Transcribing...".to_string()
                        }).unwrap_or_default();
                        let _ = sender.send(ws_text(status_msg)).await;

                        match session.transcribe(&audio_f32).await {
                            Ok((transcript, confidence)) => {
                                debug!("Transcription: '{}' (confidence: {})", transcript, confidence);

                                let trans_msg = serde_json::to_string(&VoiceServerMessage::Transcription {
                                    text: transcript.clone(),
                                    confidence,
                                }).unwrap_or_default();
                                let _ = sender.send(ws_text(trans_msg)).await;

                                if transcript.trim().is_empty() {
                                    let response_msg = serde_json::to_string(&VoiceServerMessage::Response {
                                        text: "I didn't catch that. Could you speak again?".to_string(),
                                        transcript: Some(transcript),
                                        audio: None,
                                    }).unwrap_or_default();
                                    let _ = sender.send(ws_text(response_msg)).await;
                                    continue;
                                }

                                let status_msg = serde_json::to_string(&VoiceServerMessage::Status {
                                    text: "Thinking...".to_string()
                                }).unwrap_or_default();
                                let _ = sender.send(ws_text(status_msg)).await;

                                let llm_response = match get_llm_response(&state, &transcript).await {
                                    Ok(response) => response,
                                    Err(e) => {
                                        error!("LLM error: {}", e);
                                        format!("Sorry, I had trouble thinking: {}", e)
                                    }
                                };

                                let audio_response = if session.tts_enabled {
                                    let status_msg = serde_json::to_string(&VoiceServerMessage::Status {
                                        text: "Synthesizing speech...".to_string()
                                    }).unwrap_or_default();
                                    let _ = sender.send(ws_text(status_msg)).await;

                                    match session.synthesize(&llm_response).await {
                                        Ok(audio_bytes) => Some(BASE64.encode(&audio_bytes)),
                                        Err(e) => {
                                            debug!("TTS synthesis failed: {}", e);
                                            None
                                        }
                                    }
                                } else {
                                    None
                                };

                                let response_msg = serde_json::to_string(&VoiceServerMessage::Response {
                                    text: llm_response,
                                    transcript: Some(transcript),
                                    audio: audio_response,
                                }).unwrap_or_default();
                                let _ = sender.send(ws_text(response_msg)).await;
                            }
                            Err(e) => {
                                error!("Transcription error: {}", e);
                                let error_msg = serde_json::to_string(&VoiceServerMessage::Error {
                                    text: format!("Transcription failed: {}", e)
                                }).unwrap_or_default();
                                let _ = sender.send(ws_text(error_msg)).await;
                            }
                        }
                    }
                    Ok(VoiceClientMessage::Voice { .. }) => {
                        let status_msg = serde_json::to_string(&VoiceServerMessage::Status {
                            text: "Legacy 'voice' message received. Use 'audio' type.".to_string()
                        }).unwrap_or_default();
                        let _ = sender.send(ws_text(status_msg)).await;
                    }
                    Ok(VoiceClientMessage::Ping) => {
                        let pong_msg = serde_json::to_string(&VoiceServerMessage::Pong).unwrap_or_default();
                        let _ = sender.send(ws_text(pong_msg)).await;
                    }
                    Err(e) => {
                        error!("Failed to parse client message: {}", e);
                    }
                }
            }
            Ok(WsMessage::Close(_)) => {
                info!("Voice WebSocket closed by client");
                break;
            }
            Ok(WsMessage::Binary(_)) => {
                debug!("Received binary message (not supported)");
            }
            Err(e) => {
                error!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    info!("Voice WebSocket disconnected");
}

/// Get LLM response via OpenRouter
async fn get_llm_response(state: &ServerState, user_input: &str) -> Result<String> {
    let client = OpenRouterClient::from_config(&state.config)?;
    let model = &state.config.local_model.voice_chat_model;

    debug!("Sending to LLM ({}): {}", model, user_input);

    let response = client.chat_with_model(model, user_input).await
        .context("LLM request failed")?;

    Ok(response)
}
