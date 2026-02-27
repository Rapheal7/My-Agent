//! Local TTS using Kokorox HTTP API
//!
//! Wraps the Kokorox (Kokoro-82M) TTS server which exposes an OpenAI-compatible
//! `/v1/audio/speech` endpoint. Requests WAV format and converts to raw PCM
//! Int16 24kHz mono for WebSocket streaming.

use anyhow::{Result, Context};
use reqwest::Client;
use serde::Serialize;
use tracing::{info, debug};

/// Local TTS client using Kokorox HTTP API
pub struct LocalTts {
    /// Base URL of the Kokorox server
    base_url: String,
    /// Voice name (e.g., "af_heart")
    voice: String,
    /// HTTP client
    client: Client,
}

#[derive(Serialize)]
struct SpeechRequest<'a> {
    model: &'a str,
    input: &'a str,
    voice: &'a str,
    response_format: &'a str,
}

impl LocalTts {
    /// Create a new LocalTts client
    pub fn new(base_url: &str, voice: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            voice: voice.to_string(),
            client: Client::new(),
        }
    }

    /// Create from VoiceConfig
    pub fn from_config(config: &crate::config::VoiceConfig) -> Self {
        Self::new(&config.tts_url, &config.tts_voice)
    }

    /// Synthesize text to raw PCM Int16 LE bytes (24kHz mono)
    ///
    /// Requests WAV from Kokorox, then converts IEEE Float32 samples to Int16.
    pub async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        debug!("TTS synthesizing: \"{}\"", crate::truncate_safe(text, 80));

        let request = SpeechRequest {
            model: "kokoro",
            input: text,
            voice: &self.voice,
            response_format: "wav",
        };

        let response = self
            .client
            .post(format!("{}/v1/audio/speech", self.base_url))
            .json(&request)
            .send()
            .await
            .context("Failed to connect to Kokorox TTS server")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Kokorox TTS error ({}): {}",
                status,
                body
            ));
        }

        let wav_bytes = response
            .bytes()
            .await
            .context("Failed to read TTS response")?
            .to_vec();

        // Parse WAV and convert to Int16 PCM
        let pcm_bytes = wav_to_pcm_i16(&wav_bytes)?;

        info!(
            "TTS produced {} bytes ({:.1}s of audio at 24kHz)",
            pcm_bytes.len(),
            pcm_bytes.len() as f64 / (24000.0 * 2.0)
        );

        Ok(pcm_bytes)
    }

    /// Check if the TTS server is available
    pub async fn is_available(&self) -> bool {
        self.client
            .get(format!("{}/v1/audio/speech", self.base_url))
            .send()
            .await
            .is_ok()
    }
}

/// Convert WAV bytes to raw PCM Int16 LE bytes.
///
/// Handles streaming WAV files from Kokorox that have 0xFFFFFFFF chunk sizes
/// (which hound cannot parse). Parses the WAV header manually and converts
/// IEEE Float32 samples to Int16.
fn wav_to_pcm_i16(wav_bytes: &[u8]) -> Result<Vec<u8>> {
    // Minimum WAV header: 44 bytes (RIFF + fmt + data headers)
    if wav_bytes.len() < 44 {
        return Err(anyhow::anyhow!("WAV data too short: {} bytes", wav_bytes.len()));
    }

    // Verify RIFF header
    if &wav_bytes[0..4] != b"RIFF" || &wav_bytes[8..12] != b"WAVE" {
        return Err(anyhow::anyhow!("Not a valid WAV file"));
    }

    // Find the "data" chunk - scan past fmt chunk
    let mut pos = 12; // After "WAVE"
    let mut data_start = 0usize;
    let mut audio_format = 0u16;
    let mut bits_per_sample = 0u16;

    while pos + 8 <= wav_bytes.len() {
        let chunk_id = &wav_bytes[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            wav_bytes[pos + 4], wav_bytes[pos + 5],
            wav_bytes[pos + 6], wav_bytes[pos + 7],
        ]);

        if chunk_id == b"fmt " {
            if pos + 8 + 16 <= wav_bytes.len() {
                audio_format = u16::from_le_bytes([wav_bytes[pos + 8], wav_bytes[pos + 9]]);
                bits_per_sample = u16::from_le_bytes([wav_bytes[pos + 22], wav_bytes[pos + 23]]);
            }
            // Handle 0xFFFFFFFF size: use the known fmt size (16 for PCM, 18+ for float)
            let real_size = if chunk_size == 0xFFFFFFFF { 16 } else { chunk_size as usize };
            pos += 8 + real_size;
        } else if chunk_id == b"data" {
            data_start = pos + 8;
            break;
        } else {
            // Skip unknown chunk
            let real_size = if chunk_size == 0xFFFFFFFF { 0 } else { chunk_size as usize };
            pos += 8 + real_size;
        }
    }

    if data_start == 0 || data_start >= wav_bytes.len() {
        return Err(anyhow::anyhow!("Could not find data chunk in WAV"));
    }

    let audio_data = &wav_bytes[data_start..];
    let mut pcm_bytes = Vec::new();

    match audio_format {
        3 => {
            // IEEE Float32 -> Int16
            for chunk in audio_data.chunks_exact(4) {
                let f = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let i16_val = (f * 32767.0).clamp(-32768.0, 32767.0) as i16;
                pcm_bytes.extend_from_slice(&i16_val.to_le_bytes());
            }
        }
        1 => {
            // PCM Int
            if bits_per_sample == 16 {
                // Already Int16 LE, just copy
                pcm_bytes.extend_from_slice(audio_data);
            } else if bits_per_sample == 32 {
                // Int32 -> Int16
                for chunk in audio_data.chunks_exact(4) {
                    let i32_val = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    let i16_val = (i32_val >> 16) as i16;
                    pcm_bytes.extend_from_slice(&i16_val.to_le_bytes());
                }
            } else {
                return Err(anyhow::anyhow!("Unsupported WAV bit depth: {}", bits_per_sample));
            }
        }
        _ => {
            return Err(anyhow::anyhow!("Unsupported WAV audio format: {}", audio_format));
        }
    }

    Ok(pcm_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_tts_creation() {
        let tts = LocalTts::new("http://localhost:3001", "af_heart");
        assert_eq!(tts.base_url, "http://localhost:3001");
        assert_eq!(tts.voice, "af_heart");
    }

    #[test]
    fn test_tts_url_trailing_slash() {
        let tts = LocalTts::new("http://localhost:3001/", "af_heart");
        assert_eq!(tts.base_url, "http://localhost:3001");
    }
}
