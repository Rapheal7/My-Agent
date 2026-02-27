//! Real-time voice WebSocket handler
//!
//! Handles voice conversations over WebSocket:
//! - Client sends WebM/Opus chunks every 500ms (from MediaRecorder)
//! - Server accumulates chunks, periodically decodes via ffmpeg
//! - RMS-based VAD detects speech start/end
//! - On end-of-speech: STT → LLM → TTS pipeline
//! - Server sends back PCM audio (24kHz 16-bit mono) and JSON messages

use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, State},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::io::AsyncWriteExt;
use tracing::{info, debug, warn, error};

use crate::agent::llm::{ChatMessage, OpenRouterClient};
use crate::agent::tools::{builtin_tools, ToolContext};
use crate::agent::tool_loop::{run_tool_loop, ToolLoopConfig};
use crate::agent::compaction::SessionCompactor;
use crate::security::approval::{ApprovalManager, ApprovalConfig, RiskLevel};
use crate::config::Config;
use crate::memory::MemoryStore;
use crate::memory::retrieval::SemanticSearch;
use crate::voice::stt_local::LocalStt;
use crate::voice::tts_local::LocalTts;

use super::ServerState;

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "init")]
    Init { #[serde(default)] voice: Option<String> },
    #[serde(rename = "interrupt")]
    Interrupt,
    #[serde(rename = "text")]
    Text { content: String },
    #[serde(rename = "audio_format")]
    AudioFormat { format: String, #[serde(default)] mime: Option<String> },
    #[serde(rename = "mic_stop")]
    MicStop,
    #[serde(rename = "ping")]
    Ping,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "transcript")]
    Transcript { text: String, is_final: bool },
    #[serde(rename = "chunk")]
    Chunk { text: String },
    #[serde(rename = "done")]
    Done { full_text: String },
    #[serde(rename = "status")]
    Status { state: String },
    #[serde(rename = "task_update")]
    TaskUpdate { tool: String, status: String, summary: String },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "pong")]
    Pong,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SessionState { Listening, Hearing, Processing, Speaking }


const VOICE_SYSTEM_PROMPT: &str = r#"You are a helpful AI assistant in a real-time voice conversation. Keep responses concise and conversational - aim for 1-3 sentences unless the user asks for detail. Speak naturally as if in a phone call.

You have access to tools that let you control the user's computer:
- read_file, write_file, list_directory: Browse and edit files
- execute_command: Run shell commands (git, npm, cargo, etc.)
- web_fetch: Fetch web pages
- search_files, search_content: Find files and search code
- And more filesystem and system tools

When the user asks you to do something that requires tools, use them. You can read files, run commands, write code, manage projects, and more. Use tools as needed to accomplish the task, then give a conversational summary of what you did.

Do not use markdown, bullet points, or code blocks - your responses will be spoken aloud. Use natural speech patterns instead."#;

// ─── FFmpeg WebM Decoder ─────────────────────────────────────

async fn decode_webm_to_pcm(webm_data: &[u8]) -> Result<Vec<i16>, String> {
    let mut child = tokio::process::Command::new("/usr/bin/ffmpeg")
        .args(["-f", "webm", "-i", "pipe:0", "-f", "s16le", "-ar", "16000", "-ac", "1", "-loglevel", "error", "pipe:1"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn ffmpeg: {}", e))?;

    let mut stdin = child.stdin.take().ok_or("Failed to get ffmpeg stdin")?;
    let owned = webm_data.to_vec();
    tokio::spawn(async move {
        let _ = stdin.write_all(&owned).await;
        let _ = stdin.shutdown().await;
    });

    let output = child.wait_with_output().await
        .map_err(|e| format!("ffmpeg error: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg: {}", stderr.trim()));
    }

    Ok(output.stdout.chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect())
}

/// Compute RMS energy of audio samples
fn rms_energy(samples: &[i16]) -> f32 {
    if samples.is_empty() { return 0.0; }
    (samples.iter().map(|&s| (s as f32).powi(2)).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Process transcription → LLM (with tools) → TTS
async fn process_voice_response(
    transcription: &str,
    conversation: &mut Vec<ChatMessage>,
    semantic_search: &Option<SemanticSearch>,
    client: &OpenRouterClient,
    model: &str,
    tts: &LocalTts,
    tool_ctx: &ToolContext,
    compactor: &SessionCompactor,
    tx: &mpsc::Sender<OutboundMessage>,
    interrupted: &mut bool,
) {
    let _ = tx.send(OutboundMessage::Json(ServerMessage::Transcript {
        text: transcription.to_string(), is_final: true,
    })).await;

    let context = if let Some(ref search) = semantic_search {
        match search.get_context(transcription, 5).await {
            Ok(ctx) if !ctx.context_text.is_empty() => Some(ctx.context_text),
            _ => None,
        }
    } else { None };

    let user_msg = if let Some(ctx) = context {
        format!("[Relevant context from memory: {}]\n\nUser: {}", ctx, transcription)
    } else { transcription.to_string() };
    conversation.push(ChatMessage::user(&user_msg));

    // Auto-compact if conversation is getting long (>20 messages or >6000 tokens)
    if SessionCompactor::should_compact(conversation, 20, 6000) {
        info!("Compacting voice conversation ({} messages)", conversation.len());
        // Timeout compaction at 15s — if it takes longer, skip and keep original history
        match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            compactor.compact(conversation, 6),
        ).await {
            Ok(Ok(compacted)) => {
                info!("Compacted {} → {} messages", conversation.len(), compacted.len());
                *conversation = compacted;
            }
            Ok(Err(e)) => {
                warn!("Compaction failed, continuing with full history: {}", e);
            }
            Err(_) => {
                warn!("Compaction timed out, continuing with full history");
            }
        }
    }

    let _ = tx.send(OutboundMessage::Json(ServerMessage::Status { state: "speaking".to_string() })).await;
    *interrupted = false;

    // Use tool loop for full agent capabilities
    let tx_tool = tx.clone();
    let tx_complete = tx.clone();
    let loop_config = ToolLoopConfig {
        model: model.to_string(),
        system_prompt: VOICE_SYSTEM_PROMPT.to_string(),
        allowed_tools: builtin_tools(),
        max_iterations: 10,
        max_tokens: 4096,
        on_tool_start: Some(Box::new(move |name: &str| {
            let msg = ServerMessage::TaskUpdate {
                tool: name.to_string(),
                status: "running".to_string(),
                summary: format!("Running {}...", name),
            };
            let _ = tx_tool.try_send(OutboundMessage::Json(msg));
        })),
        on_tool_complete: Some(Box::new(move |name: &str, success: bool, message: &str| {
            let msg = ServerMessage::TaskUpdate {
                tool: name.to_string(),
                status: "done".to_string(),
                summary: if success {
                    format!("{} completed", name)
                } else {
                    format!("{} failed: {}", name, crate::truncate_safe(&message, 100))
                },
            };
            let _ = tx_complete.try_send(OutboundMessage::Json(msg));
        })),
        on_progress: None,
        timeout_secs: 300,
    };

    // Build messages for the tool loop (it adds its own system prompt)
    let loop_messages: Vec<ChatMessage> = conversation.iter()
        .filter(|m| {
            // Skip system messages — tool loop adds its own
            m.role.as_ref()
                .and_then(|r| r.as_str())
                .map(|r| r != "system")
                .unwrap_or(true)
        })
        .cloned()
        .collect();

    match run_tool_loop(client, loop_messages, tool_ctx, &loop_config).await {
        Ok(result) => {
            let response_text = result.final_response;
            if result.tool_calls_made > 0 {
                info!("Tool loop: {} iterations, {} tool calls", result.iterations, result.tool_calls_made);
            }
            info!("LLM response ({} chars): \"{}\"", response_text.len(),
                crate::truncate_safe(&response_text, 100));

            for sentence in &split_sentences(&response_text) {
                if *interrupted { break; }
                let _ = tx.send(OutboundMessage::Json(ServerMessage::Chunk { text: sentence.clone() })).await;
                match tts.synthesize(sentence).await {
                    Ok(pcm) if !*interrupted => { let _ = tx.send(OutboundMessage::Binary(pcm)).await; }
                    Err(e) => warn!("TTS error: {}", e),
                    _ => {}
                }
            }
            let _ = tx.send(OutboundMessage::Json(ServerMessage::Done { full_text: response_text.clone() })).await;
            conversation.push(ChatMessage::assistant(&response_text));
        }
        Err(e) => {
            error!("LLM/tool error: {}", e);
            let _ = tx.send(OutboundMessage::Json(ServerMessage::Error { message: format!("Error: {}", e) })).await;
        }
    }
    let _ = tx.send(OutboundMessage::Json(ServerMessage::Status { state: "listening".to_string() })).await;
}

// ─── WebSocket Handler ───────────────────────────────────────

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<ServerState>) -> Response {
    ws.on_upgrade(|socket| handle_voice_session(socket, state))
}

async fn handle_voice_session(ws: WebSocket, state: ServerState) {
    info!("Real-time voice session connected");

    let (mut ws_tx, mut ws_rx) = ws.split();
    let (tx, mut rx) = mpsc::channel::<OutboundMessage>(64);

    let sender_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let result = match msg {
                OutboundMessage::Json(m) => {
                    ws_tx.send(Message::Text(serde_json::to_string(&m).unwrap_or_default().into())).await
                }
                OutboundMessage::Binary(d) => ws_tx.send(Message::Binary(d.into())).await,
            };
            if result.is_err() { break; }
        }
    });

    let config = Config::load().unwrap_or_default();
    let stt = LocalStt::from_config(&config.voice);
    let tts = LocalTts::from_config(&config.voice);

    let client = match OpenRouterClient::from_keyring() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(OutboundMessage::Json(ServerMessage::Error {
                message: format!("LLM client init failed: {}", e),
            })).await;
            return;
        }
    };

    let semantic_search = match MemoryStore::default_store().await {
        Ok(store) => Some(SemanticSearch::new(Arc::new(store))),
        Err(e) => { debug!("Memory store unavailable: {}", e); None }
    };

    let mut session_state = SessionState::Listening;
    let mut conversation: Vec<ChatMessage> = vec![ChatMessage::system(VOICE_SYSTEM_PROMPT)];
    // Voice mode: auto-approve all tool actions (no terminal for interactive approval)
    let voice_approval = ApprovalManager::new(ApprovalConfig {
        approval_threshold: RiskLevel::Critical,
        auto_approve_low_risk: true,
        session_duration_minutes: 480,
        enable_audit_log: true,
    });
    let mut tool_ctx = ToolContext::new();
    tool_ctx.approver = voice_approval.clone();
    tool_ctx.shell = tool_ctx.shell.set_approver(voice_approval.clone());
    tool_ctx.web = tool_ctx.web.set_approver(voice_approval.clone());
    tool_ctx.filesystem = tool_ctx.filesystem.set_approver(voice_approval);
    tool_ctx.device_registry = Some(state.device_registry.clone());
    let model = "arcee-ai/trinity-large-preview:free".to_string();
    let compactor = SessionCompactor::from_config(client.clone());
    let mut interrupted = false;

    // WebM accumulation buffer
    let mut webm_buffer: Vec<u8> = Vec::new();
    let mut chunk_count: u32 = 0;
    // Tracks how many PCM samples we've already decoded from the buffer
    let mut last_decoded_samples: usize = 0;
    // All decoded PCM for the current utterance
    let mut all_pcm: Vec<i16> = Vec::new();
    // VAD state
    let mut consecutive_silent: u32 = 0;
    // Decode every N chunks to avoid excessive ffmpeg spawns
    const DECODE_INTERVAL: u32 = 2; // every 2 chunks = ~1s
    // RMS threshold for speech detection
    const SPEECH_RMS_THRESHOLD: f32 = 200.0;
    // Number of consecutive silent decodes before end-of-speech
    const SILENCE_COUNT_FOR_EOS: u32 = 2; // 2 silent decodes = ~2s silence
    // Echo cooldown: ignore audio for a bit after agent finishes speaking
    let mut echo_cooldown_until: Option<std::time::Instant> = None;
    const ECHO_COOLDOWN_MS: u64 = 1500;
    // After cooldown expires, skip one decode to fast-forward past accumulated echo audio
    let mut echo_skip_pending = false;

    let _ = tx.send(OutboundMessage::Json(ServerMessage::Status { state: "listening".to_string() })).await;

    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Binary(data) => {
                if data.len() > 4 && &data[..4] == b"WEBM" {
                    let chunk = &data[4..];

                    // If the buffer is empty, only accept chunks that start with
                    // the EBML header magic (0x1a 0x45 0xdf 0xa3). This ensures
                    // we don't prepend leftover data from a previous recorder.
                    if webm_buffer.is_empty() {
                        if chunk.len() < 4 || &chunk[..4] != b"\x1a\x45\xdf\xa3" {
                            debug!("Skipping non-EBML chunk ({} bytes)", chunk.len());
                            continue;
                        }
                    }

                    webm_buffer.extend_from_slice(chunk);
                    chunk_count += 1;

                    if chunk_count <= 3 || chunk_count % 20 == 0 {
                        info!("WebM chunk #{}: {} bytes, buffer={} bytes",
                            chunk_count, chunk.len(), webm_buffer.len());
                    }

                    // Only run VAD when we're actually listening for speech
                    // Skip during Processing/Speaking to avoid echo pickup
                    let vad_active = matches!(session_state, SessionState::Listening | SessionState::Hearing);
                    let in_cooldown = echo_cooldown_until
                        .map(|t| std::time::Instant::now() < t)
                        .unwrap_or(false);

                    if chunk_count % DECODE_INTERVAL == 0 && !webm_buffer.is_empty()
                        && vad_active && !in_cooldown
                    {
                        match decode_webm_to_pcm(&webm_buffer).await {
                            Ok(all_samples) => {
                                // After echo cooldown, skip accumulated samples once
                                // to discard agent's TTS audio picked up by the mic
                                if echo_skip_pending {
                                    info!("Echo skip: fast-forwarding past {} samples",
                                        all_samples.len().saturating_sub(last_decoded_samples));
                                    last_decoded_samples = all_samples.len();
                                    echo_skip_pending = false;
                                    continue;
                                }

                                // Extract only new samples since last decode
                                let new_count = all_samples.len().saturating_sub(last_decoded_samples);
                                let new_samples = if new_count > 0 {
                                    &all_samples[last_decoded_samples..]
                                } else {
                                    &[]
                                };
                                last_decoded_samples = all_samples.len();

                                let rms = rms_energy(new_samples);
                                let is_speech = rms > SPEECH_RMS_THRESHOLD;

                                info!("VAD decode #{}: {} new samples ({:.1}s), rms={:.0}, speech={}, state={:?}",
                                    chunk_count / DECODE_INTERVAL,
                                    new_samples.len(),
                                    new_samples.len() as f64 / 16000.0,
                                    rms, is_speech, session_state);

                                if is_speech {
                                    consecutive_silent = 0;

                                    if session_state == SessionState::Listening {
                                        info!("Speech started");
                                        session_state = SessionState::Hearing;
                                        // Start fresh with only the new speech samples
                                        all_pcm.clear();
                                        all_pcm.extend_from_slice(new_samples);
                                        let _ = tx.send(OutboundMessage::Json(ServerMessage::Status {
                                            state: "hearing".to_string(),
                                        })).await;
                                    } else if session_state == SessionState::Hearing {
                                        // Append new samples
                                        all_pcm.extend_from_slice(new_samples);
                                    }
                                } else {
                                    // Silent
                                    if session_state == SessionState::Hearing {
                                        // Include trailing silence
                                        all_pcm.extend_from_slice(new_samples);
                                        consecutive_silent += 1;

                                        info!("Silence #{}, buffer={:.1}s",
                                            consecutive_silent,
                                            all_pcm.len() as f64 / 16000.0);

                                        if consecutive_silent >= SILENCE_COUNT_FOR_EOS
                                            && all_pcm.len() > 4800
                                        {
                                            // End of speech — transcribe
                                            info!("End of speech, transcribing {:.1}s of audio",
                                                all_pcm.len() as f64 / 16000.0);

                                            session_state = SessionState::Processing;
                                            let _ = tx.send(OutboundMessage::Json(ServerMessage::Status {
                                                state: "processing".to_string(),
                                            })).await;

                                            match stt.transcribe(&all_pcm).await {
                                                Ok(text) if !text.is_empty() => {
                                                    info!("Transcription: \"{}\"", text);
                                                    session_state = SessionState::Speaking;
                                                    process_voice_response(
                                                        &text, &mut conversation,
                                                        &semantic_search, &client, &model, &tts,
                                                        &tool_ctx, &compactor, &tx, &mut interrupted,
                                                    ).await;
                                                    // Start echo cooldown so VAD ignores
                                                    // residual TTS audio from phone speaker
                                                    echo_cooldown_until = Some(
                                                        std::time::Instant::now() + std::time::Duration::from_millis(ECHO_COOLDOWN_MS)
                                                    );
                                                    echo_skip_pending = true;
                                                }
                                                Ok(_) => {
                                                    info!("Empty transcription");
                                                    let _ = tx.send(OutboundMessage::Json(ServerMessage::Status {
                                                        state: "listening".to_string(),
                                                    })).await;
                                                }
                                                Err(e) => {
                                                    error!("STT error: {}", e);
                                                    let _ = tx.send(OutboundMessage::Json(ServerMessage::Error {
                                                        message: format!("Transcription failed: {}", e),
                                                    })).await;
                                                    let _ = tx.send(OutboundMessage::Json(ServerMessage::Status {
                                                        state: "listening".to_string(),
                                                    })).await;
                                                }
                                            }

                                            // Reset VAD state for next utterance.
                                            // KEEP webm_buffer and last_decoded_samples intact —
                                            // the single MediaRecorder stream continues, and we
                                            // need the full buffer for ffmpeg to decode.
                                            session_state = SessionState::Listening;
                                            all_pcm.clear();
                                            consecutive_silent = 0;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                if chunk_count <= 6 {
                                    warn!("Decode error at chunk #{}: {}", chunk_count, e);
                                }
                            }
                        }
                    }
                }
            }
            Message::Text(text) => {
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Init { voice }) => {
                        info!("Session init, voice: {:?}", voice);
                    }
                    Ok(ClientMessage::AudioFormat { format, mime }) => {
                        info!("Audio format: {} ({:?})", format, mime);
                    }
                    Ok(ClientMessage::MicStop) => {
                        // User manually turned off mic — discard all buffered audio
                        info!("Mic stop (manual), discarding buffer={} bytes, pcm={} samples",
                            webm_buffer.len(), all_pcm.len());
                        session_state = SessionState::Listening;
                        webm_buffer.clear();
                        chunk_count = 0;
                        last_decoded_samples = 0;
                        all_pcm.clear();
                        consecutive_silent = 0;
                    }
                    Ok(ClientMessage::Interrupt) => {
                        info!("Interrupt");
                        interrupted = true;
                        session_state = SessionState::Listening;
                        let _ = tx.send(OutboundMessage::Json(ServerMessage::Status {
                            state: "listening".to_string(),
                        })).await;
                    }
                    Ok(ClientMessage::Text { content }) => {
                        info!("Text: \"{}\"", content);
                        session_state = SessionState::Speaking;
                        process_voice_response(
                            &content, &mut conversation,
                            &semantic_search, &client, &model, &tts,
                            &tool_ctx, &compactor, &tx, &mut interrupted,
                        ).await;
                        echo_cooldown_until = Some(
                            std::time::Instant::now() + std::time::Duration::from_millis(ECHO_COOLDOWN_MS)
                        );
                        echo_skip_pending = true;
                        session_state = SessionState::Listening;
                    }
                    Ok(ClientMessage::Ping) => {
                        let _ = tx.send(OutboundMessage::Json(ServerMessage::Pong)).await;
                    }
                    Err(e) => { debug!("Parse error: {}", e); }
                }
            }
            Message::Close(_) => { info!("Session disconnected"); break; }
            _ => {}
        }
    }

    // Save conversation to memory
    if conversation.len() > 1 {
        if let Ok(store) = MemoryStore::default_store().await {
            use crate::memory::ConversationRecord;
            let messages: Vec<crate::types::Message> = conversation.iter()
                .filter_map(|m| {
                    let role_str = m.role.as_ref()?.as_str()?;
                    let content = m.content.as_ref()?.as_str()?.to_string();
                    if role_str == "system" { return None; }
                    let role = match role_str {
                        "user" => crate::types::Role::User,
                        "assistant" => crate::types::Role::Assistant,
                        _ => return None,
                    };
                    Some(crate::types::Message { role, content, timestamp: chrono::Utc::now() })
                })
                .collect();
            if !messages.is_empty() {
                let record = ConversationRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    title: Some("Voice Chat".to_string()),
                    messages, summary: None, embedding: None,
                    created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
                    tags: vec!["voice-chat".to_string()],
                };
                let _ = store.save_conversation(&record).await;
            }
        }
    }
    sender_task.abort();
}

enum OutboundMessage { Json(ServerMessage), Binary(Vec<u8>) }

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?') {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() && trimmed.len() > 1 { sentences.push(trimmed); }
            current.clear();
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() && trimmed.len() > 2 { sentences.push(trimmed); }
    if sentences.is_empty() && !text.trim().is_empty() { sentences.push(text.trim().to_string()); }
    sentences
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_sentences() {
        assert_eq!(split_sentences("Hi. How are you?").len(), 2);
    }

    #[test]
    fn test_rms_energy() {
        assert_eq!(rms_energy(&[]), 0.0);
        assert!(rms_energy(&[1000, -1000, 1000, -1000]) > 900.0);
        assert!(rms_energy(&[0, 0, 0, 0]) < 1.0);
    }

    #[test]
    fn test_client_message() {
        let m: ClientMessage = serde_json::from_str(r#"{"type":"mic_stop"}"#).unwrap();
        assert!(matches!(m, ClientMessage::MicStop));
    }
}
