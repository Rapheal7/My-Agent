//! Local STT using faster-whisper subprocess
//!
//! Wraps the faster-whisper Python script for speech-to-text transcription.
//! Converts PCM i16 samples to WAV, passes via base64 to the subprocess.

use anyhow::{Result, Context};
use std::time::Duration;
use tracing::{info, debug};

/// Local STT client using faster-whisper
pub struct LocalStt {
    /// Whisper model size (tiny, base, small, medium, large-v3)
    model: String,
    /// Path to faster-whisper Python script
    script_path: String,
    /// Timeout for transcription
    timeout: Duration,
}

impl LocalStt {
    /// Create a new LocalStt with the specified model
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            script_path: "/home/rapheal/.local/bin/faster-whisper-server.py".to_string(),
            timeout: Duration::from_secs(30),
        }
    }

    /// Create from VoiceConfig
    pub fn from_config(config: &crate::config::VoiceConfig) -> Self {
        Self::new(&config.whisper_model)
    }

    /// Transcribe PCM i16 16kHz mono samples to text.
    /// Retries once on failure (handles faster-whisper cold start).
    pub async fn transcribe(&self, pcm_samples: &[i16]) -> Result<String> {
        if pcm_samples.is_empty() {
            return Ok(String::new());
        }

        debug!("Transcribing {} samples ({:.1}s of audio)",
            pcm_samples.len(),
            pcm_samples.len() as f64 / 16000.0
        );

        // Convert PCM i16 to WAV bytes in memory
        let wav_bytes = pcm_to_wav(pcm_samples, 16000)?;

        // Base64 encode for passing to Python
        let audio_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &wav_bytes,
        );

        // Try up to 2 times (first call may fail due to model cold start)
        let mut last_err = None;
        for attempt in 0..2 {
            if attempt > 0 {
                info!("Retrying transcription (attempt {})", attempt + 1);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            match self.run_whisper(&audio_b64).await {
                Ok(text) => return Ok(text),
                Err(e) => {
                    info!("Transcription attempt {} failed: {}", attempt + 1, e);
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Transcription failed")))
    }

    async fn run_whisper(&self, audio_b64: &str) -> Result<String> {
        use tokio::io::AsyncWriteExt;

        info!("Running whisper: b64 len={}, model={}", audio_b64.len(), self.model);

        let mut child = tokio::process::Command::new("/usr/bin/python3")
            .arg(&self.script_path)
            .arg("-")  // read from stdin
            .arg(&self.model)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("Failed to spawn faster-whisper process")?;

        // Write base64 audio to stdin
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("No stdin"))?;
        let b64_owned = audio_b64.to_string();
        tokio::spawn(async move {
            let _ = stdin.write_all(b64_owned.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });

        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| anyhow::anyhow!("Transcription timeout ({}s)", self.timeout.as_secs()))?
            .context("Failed to wait for faster-whisper")?;

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "faster-whisper error: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Parse JSON response
        let result: serde_json::Value = serde_json::from_slice(&output.stdout)
            .context("Failed to parse transcription result")?;

        // Check for error
        if let Some(error) = result.get("error").and_then(|e| e.as_str()) {
            if !error.is_empty() {
                return Err(anyhow::anyhow!("Transcription error: {}", error));
            }
        }

        let text = result
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        info!("Transcribed: \"{}\"", text);
        Ok(text)
    }
}

/// Convert PCM i16 samples to WAV bytes in memory
fn pcm_to_wav(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(Vec::new());

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::new(&mut cursor, spec)
        .context("Failed to create WAV writer")?;

    for &sample in samples {
        writer.write_sample(sample)?;
    }

    writer.finalize()?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pcm_to_wav() {
        let samples = vec![0i16; 16000]; // 1 second of silence
        let wav = pcm_to_wav(&samples, 16000).unwrap();
        // WAV header is 44 bytes, data is 16000 * 2 = 32000 bytes
        assert_eq!(wav.len(), 44 + 32000);
        // Check RIFF header
        assert_eq!(&wav[0..4], b"RIFF");
    }

    #[test]
    fn test_local_stt_creation() {
        let stt = LocalStt::new("medium");
        assert_eq!(stt.model, "medium");
    }
}
