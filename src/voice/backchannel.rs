//! Backchannel detection and response generation
//!
//! Detects conversational pauses during speech and produces short
//! backchannel responses ("mhm", "right", "I see") to make the
//! conversation feel more natural.

use std::time::{Duration, Instant};
use tracing::debug;

/// Phrases used for backchannel responses
const BACKCHANNEL_PHRASES: &[&str] = &["mhm", "right", "I see", "okay", "yeah"];

/// Backchannel detector and clip manager
pub struct BackchannelDetector {
    /// Cached TTS audio clips for each phrase (PCM bytes)
    clips: Vec<BackchannelClip>,
    /// Last time a backchannel was emitted
    last_backchannel: Instant,
    /// Minimum time between backchannels
    cooldown: Duration,
    /// Minimum pause duration to trigger a backchannel
    pause_threshold: Duration,
    /// Minimum speech duration before allowing backchannel
    speech_min: Duration,
    /// Current clip index (round-robin)
    next_clip: usize,
    /// When the current speech segment started
    speech_start: Option<Instant>,
}

/// A pre-generated backchannel audio clip
#[derive(Clone)]
pub struct BackchannelClip {
    /// The phrase text
    pub text: String,
    /// PCM audio bytes (24kHz 16-bit mono from Kokorox)
    pub pcm_data: Vec<u8>,
}

impl BackchannelDetector {
    /// Create a new detector with default settings
    pub fn new() -> Self {
        Self::with_config(Duration::from_millis(400), Duration::from_secs(4))
    }

    /// Create with custom pause threshold and cooldown
    pub fn with_config(pause_threshold: Duration, cooldown: Duration) -> Self {
        Self {
            clips: Vec::new(),
            last_backchannel: Instant::now() - cooldown, // Allow immediate first use
            cooldown,
            pause_threshold,
            speech_min: Duration::from_secs(1),
            next_clip: 0,
            speech_start: None,
        }
    }

    /// Create from VoiceConfig
    pub fn from_config(config: &crate::config::VoiceConfig) -> Self {
        if config.backchannel_enabled {
            Self::with_config(
                Duration::from_millis(config.backchannel_pause_ms),
                Duration::from_secs(4),
            )
        } else {
            let mut detector = Self::new();
            // Disable by setting impossibly high threshold
            detector.pause_threshold = Duration::from_secs(999);
            detector
        }
    }

    /// Generate backchannel clips by calling the TTS API at startup
    pub async fn generate_clips(&mut self, tts: &super::tts_local::LocalTts) {
        for phrase in BACKCHANNEL_PHRASES {
            match tts.synthesize(phrase).await {
                Ok(pcm_data) => {
                    self.clips.push(BackchannelClip {
                        text: phrase.to_string(),
                        pcm_data,
                    });
                    debug!("Generated backchannel clip: \"{}\"", phrase);
                }
                Err(e) => {
                    tracing::warn!("Failed to generate backchannel clip for \"{}\": {}", phrase, e);
                }
            }
        }
    }

    /// Notify that speech has started
    pub fn on_speech_start(&mut self) {
        self.speech_start = Some(Instant::now());
    }

    /// Notify that speech has ended
    pub fn on_speech_end(&mut self) {
        self.speech_start = None;
    }

    /// Check if a backchannel should be emitted for the given pause duration.
    /// Returns the clip to play if a backchannel is appropriate.
    pub fn check_pause(&mut self, pause_duration: Duration) -> Option<&BackchannelClip> {
        // No clips loaded
        if self.clips.is_empty() {
            return None;
        }

        // Pause not long enough
        if pause_duration < self.pause_threshold {
            return None;
        }

        // Cooldown not elapsed
        let now = Instant::now();
        if now.duration_since(self.last_backchannel) < self.cooldown {
            return None;
        }

        // User hasn't been speaking long enough
        if let Some(speech_start) = self.speech_start {
            if now.duration_since(speech_start) < self.speech_min {
                return None;
            }
        } else {
            return None; // Not in speech
        }

        // Emit a backchannel
        self.last_backchannel = now;
        let clip_idx = self.next_clip % self.clips.len();
        self.next_clip += 1;

        debug!("Backchannel triggered: \"{}\"", self.clips[clip_idx].text);
        Some(&self.clips[clip_idx])
    }

    /// Get all phrase texts
    pub fn phrases() -> &'static [&'static str] {
        BACKCHANNEL_PHRASES
    }

    /// Check if clips are loaded
    pub fn has_clips(&self) -> bool {
        !self.clips.is_empty()
    }

    /// Number of loaded clips
    pub fn clip_count(&self) -> usize {
        self.clips.len()
    }
}

impl Default for BackchannelDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backchannel_detector_creation() {
        let detector = BackchannelDetector::new();
        assert!(!detector.has_clips());
        assert_eq!(detector.clip_count(), 0);
    }

    #[test]
    fn test_backchannel_no_clips() {
        let mut detector = BackchannelDetector::new();
        detector.on_speech_start();
        // No clips loaded, should return None
        assert!(detector.check_pause(Duration::from_secs(1)).is_none());
    }

    #[test]
    fn test_backchannel_cooldown() {
        let mut detector = BackchannelDetector::with_config(
            Duration::from_millis(100),
            Duration::from_secs(10),
        );

        // Add a fake clip
        detector.clips.push(BackchannelClip {
            text: "mhm".to_string(),
            pcm_data: vec![0; 100],
        });

        detector.on_speech_start();
        // Wait briefly for speech_min
        std::thread::sleep(Duration::from_millis(50));

        // Manually set speech_start far enough back
        detector.speech_start = Some(Instant::now() - Duration::from_secs(2));

        // First check should succeed (cooldown elapsed from constructor)
        let result = detector.check_pause(Duration::from_millis(500));
        assert!(result.is_some());

        // Second check should fail (cooldown)
        let result = detector.check_pause(Duration::from_millis(500));
        assert!(result.is_none());
    }

    #[test]
    fn test_backchannel_phrases() {
        let phrases = BackchannelDetector::phrases();
        assert!(phrases.len() >= 3);
        assert!(phrases.contains(&"mhm"));
    }
}
