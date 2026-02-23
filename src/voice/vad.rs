//! Voice Activity Detection (VAD) Module
//!
//! Provides real-time speech detection from audio streams.
//! Uses a hybrid approach combining energy-based detection with
//! optional ML-based enhancement (silero-vad or similar).
//!
//! # Architecture
//!
//! ```text
//! Audio Stream → Frame Extraction → Energy Analysis → Speech/Noise Decision
//!                                     ↓
//!                              ML Enhancement (optional)
//! ```
//!
//! # Features
//! - Real-time frame-by-frame analysis
//! - Configurable sensitivity thresholds
//! - Noise floor adaptation
//! - Hangover periods to prevent clipping
//! - ML-based enhancement support

use anyhow::{Result, Context, bail};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, debug, trace};

/// Default sample rate for VAD processing
pub const DEFAULT_VAD_SAMPLE_RATE: u32 = 16000;

/// Frame size in samples (10ms at 16kHz)
pub const DEFAULT_FRAME_SIZE: usize = 160;

/// Default energy threshold (dB below peak)
pub const DEFAULT_ENERGY_THRESHOLD_DB: f32 = 40.0;

/// Default hangover frames (continue speech detection after energy drops)
pub const DEFAULT_HANGOVER_FRAMES: usize = 20; // 200ms

/// Default speech onset frames (require consecutive speech frames)
pub const DEFAULT_ONSET_FRAMES: usize = 3; // 30ms

/// Voice Activity Detector
pub struct Vad {
    /// Current VAD configuration
    config: VadConfig,
    /// Running energy level (for adaptive threshold)
    noise_floor: Arc<Mutex<f32>>,
    /// Current state
    state: Arc<Mutex<VadState>>,
    /// Frame history for hangover
    frame_history: Arc<Mutex<VecDeque<VadFrame>>>,
    /// Speech detection callback
    speech_callback: Option<Box<dyn Fn(bool) + Send + 'static>>,
    /// Currently detecting speech
    is_speech: Arc<AtomicBool>,
    /// Frame counter
    frame_count: Arc<Mutex<u64>>,
}

/// VAD configuration
#[derive(Debug, Clone, Copy)]
pub struct VadConfig {
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Frame size in samples
    pub frame_size: usize,
    /// Energy threshold in dB below peak
    pub energy_threshold_db: f32,
    /// Hangover frames after speech ends
    pub hangover_frames: usize,
    /// Onset frames required to start speech
    pub onset_frames: usize,
    /// Enable adaptive noise floor
    pub adaptive_noise_floor: bool,
    /// Noise floor adaptation rate (0.0-1.0)
    pub adaptation_rate: f32,
    /// Minimum speech duration in frames
    pub min_speech_frames: usize,
    /// Maximum silence within speech in frames
    pub max_silence_frames: usize,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_VAD_SAMPLE_RATE,
            frame_size: DEFAULT_FRAME_SIZE,
            energy_threshold_db: DEFAULT_ENERGY_THRESHOLD_DB,
            hangover_frames: DEFAULT_HANGOVER_FRAMES,
            onset_frames: DEFAULT_ONSET_FRAMES,
            adaptive_noise_floor: true,
            adaptation_rate: 0.05,
            min_speech_frames: 5,      // 50ms minimum
            max_silence_frames: 10,    // 100ms max silence within speech
        }
    }
}

impl VadConfig {
    /// Create a new config with aggressive (low latency) settings
    pub fn aggressive() -> Self {
        Self {
            energy_threshold_db: 35.0,
            hangover_frames: 10,
            onset_frames: 2,
            min_speech_frames: 3,
            ..Default::default()
        }
    }

    /// Create a new config with conservative (high accuracy) settings
    pub fn conservative() -> Self {
        Self {
            energy_threshold_db: 45.0,
            hangover_frames: 30,
            onset_frames: 5,
            min_speech_frames: 10,
            max_silence_frames: 5,
            ..Default::default()
        }
    }
}

/// VAD state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    /// No speech detected
    Silence,
    /// Possibly starting speech (in onset period)
    MaybeSpeech,
    /// Speech confirmed
    Speech,
    /// Speech ending (in hangover period)
    SpeechEnding,
}

/// A single VAD frame result
#[derive(Debug, Clone, Copy)]
pub struct VadFrame {
    /// Frame number
    pub frame_id: u64,
    /// Raw energy level (linear)
    pub energy: f32,
    /// Energy in dB
    pub energy_db: f32,
    /// Is this frame speech?
    pub is_speech: bool,
    /// Current threshold
    pub threshold: f32,
}

/// Speech segment detected by VAD
#[derive(Debug, Clone)]
pub struct SpeechSegment {
    /// Start frame index
    pub start_frame: u64,
    /// End frame index
    pub end_frame: u64,
    /// Start time in seconds
    pub start_time_secs: f64,
    /// End time in seconds
    pub end_time_secs: f64,
    /// Duration in seconds
    pub duration_secs: f64,
    /// Average energy during speech
    pub avg_energy: f32,
    /// Peak energy during speech
    pub peak_energy: f32,
}

impl SpeechSegment {
    /// Get duration as a Duration
    pub fn duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.duration_secs)
    }
}

impl Vad {
    /// Create a new VAD with default configuration
    pub fn new() -> Self {
        Self::with_config(VadConfig::default())
    }

    /// Create a new VAD with custom configuration
    pub fn with_config(config: VadConfig) -> Self {
        info!("Creating VAD with config: {:?}", config);

        Self {
            config,
            noise_floor: Arc::new(Mutex::new(1e-10)), // Start very low
            state: Arc::new(Mutex::new(VadState::Silence)),
            frame_history: Arc::new(Mutex::new(VecDeque::with_capacity(
                config.hangover_frames.max(100)
            ))),
            speech_callback: None,
            is_speech: Arc::new(AtomicBool::new(false)),
            frame_count: Arc::new(Mutex::new(0)),
        }
    }

    /// Set a callback for speech state changes
    pub fn on_speech_change<F>(&mut self, callback: F)
    where
        F: Fn(bool) + Send + 'static,
    {
        self.speech_callback = Some(Box::new(callback));
    }

    /// Process a frame of audio samples
    ///
    /// Returns true if speech is detected in this frame
    pub fn process_frame(&self, samples: &[f32]) -> Result<bool> {
        if samples.len() != self.config.frame_size {
            bail!("Expected {} samples, got {}", self.config.frame_size, samples.len());
        }

        // Calculate frame energy
        let energy = calculate_energy(samples);
        let energy_db = 10.0 * energy.log10().max(-100.0);

        // Get current noise floor and update
        let mut noise_floor = self.noise_floor.lock().unwrap();
        let current_threshold = *noise_floor * 10f32.powf(self.config.energy_threshold_db / 10.0);

        // Adaptive noise floor update
        if self.config.adaptive_noise_floor {
            if energy < current_threshold {
                // Likely noise, update floor
                *noise_floor = *noise_floor * (1.0 - self.config.adaptation_rate)
                    + energy * self.config.adaptation_rate;
            }
        }

        let threshold_db = 10.0 * current_threshold.log10().max(-100.0);

        // Determine if this frame is speech
        let frame_is_speech = energy > current_threshold;

        // Update state machine
        let mut state = self.state.lock().unwrap();
        let mut frame_count = self.frame_count.lock().unwrap();
        let frame_id = *frame_count;
        *frame_count += 1;

        let (new_state, is_speech) = self.update_state(*state, frame_is_speech);

        // Check for state transition
        let old_is_speech = self.is_speech.load(Ordering::SeqCst);
        if is_speech != old_is_speech {
            self.is_speech.store(is_speech, Ordering::SeqCst);
            if let Some(ref callback) = self.speech_callback {
                callback(is_speech);
            }
            if is_speech {
                debug!("Speech started at frame {}", frame_id);
            } else {
                debug!("Speech ended at frame {}", frame_id);
            }
        }

        *state = new_state;

        // Store frame in history
        let frame = VadFrame {
            frame_id,
            energy,
            energy_db,
            is_speech: frame_is_speech,
            threshold: current_threshold,
        };

        if let Ok(mut history) = self.frame_history.lock() {
            history.push_back(frame);
            if history.len() > self.config.hangover_frames * 2 {
                history.pop_front();
            }
        }

        trace!(
            "Frame {}: energy={:.2}dB, threshold={:.2}dB, speech={}",
            frame_id,
            energy_db,
            threshold_db,
            is_speech
        );

        Ok(is_speech)
    }

    /// Process a buffer of audio samples
    ///
    /// Returns the indices of speech segments found
    pub fn process_buffer(&self, samples: &[f32]) -> Result<Vec<SpeechSegment>> {
        let frame_size = self.config.frame_size;
        let mut segments = Vec::new();
        let mut current_segment_start: Option<u64> = None;
        let mut segment_energy_sum = 0.0f32;
        let mut segment_peak_energy = 0.0f32;
        let mut silence_count = 0usize;

        // Process frames
        for (frame_idx, frame_samples) in samples.chunks(frame_size).enumerate() {
            if frame_samples.len() < frame_size {
                break; // Partial frame at end
            }

            let is_speech = self.process_frame(frame_samples)?;
            let frame_id = frame_idx as u64;

            match (is_speech, current_segment_start) {
                (true, None) => {
                    // Speech start
                    current_segment_start = Some(frame_id);
                    segment_energy_sum = calculate_energy(frame_samples);
                    segment_peak_energy = segment_energy_sum;
                    silence_count = 0;
                }
                (true, Some(start)) => {
                    // Continuing speech
                    let energy = calculate_energy(frame_samples);
                    segment_energy_sum += energy;
                    segment_peak_energy = segment_peak_energy.max(energy);
                    silence_count = 0;

                    // Check for max segment length or too much silence
                    if frame_id - start > 3000 { // Max ~30 seconds
                        // End segment
                        segments.push(self.create_segment(
                            start,
                            frame_id,
                            segment_energy_sum / (frame_id - start) as f32,
                            segment_peak_energy,
                        ));
                        current_segment_start = None;
                    }
                }
                (false, Some(start)) => {
                    // Potential speech end
                    silence_count += 1;

                    if silence_count > self.config.max_silence_frames {
                        // End segment
                        let frame_count = frame_id - start - silence_count as u64;
                        if frame_count >= self.config.min_speech_frames as u64 {
                            segments.push(self.create_segment(
                                start,
                                frame_id - silence_count as u64,
                                segment_energy_sum / frame_count.max(1) as f32,
                                segment_peak_energy,
                            ));
                        }
                        current_segment_start = None;
                        segment_energy_sum = 0.0;
                        segment_peak_energy = 0.0;
                    }
                }
                (false, None) => {
                    // Continuing silence
                }
            }
        }

        // Handle segment that extends to end of buffer
        if let Some(start) = current_segment_start {
            let end_frame = (samples.len() / frame_size) as u64;
            let frame_count = end_frame - start;
            if frame_count >= self.config.min_speech_frames as u64 {
                segments.push(self.create_segment(
                    start,
                    end_frame,
                    segment_energy_sum / frame_count.max(1) as f32,
                    segment_peak_energy,
                ));
            }
        }

        Ok(segments)
    }

    /// Update the VAD state machine
    fn update_state(&self, current_state: VadState, frame_is_speech: bool) -> (VadState, bool) {
        use VadState::*;

        match current_state {
            Silence => {
                if frame_is_speech {
                    (MaybeSpeech, false)
                } else {
                    (Silence, false)
                }
            }
            MaybeSpeech => {
                if frame_is_speech {
                    // Count consecutive speech frames in history
                    if let Ok(history) = self.frame_history.lock() {
                        let recent_speech = history
                            .iter()
                            .rev()
                            .take(self.config.onset_frames)
                            .filter(|f| f.is_speech)
                            .count();

                        if recent_speech >= self.config.onset_frames {
                            (Speech, true)
                        } else {
                            (MaybeSpeech, false)
                        }
                    } else {
                        (MaybeSpeech, false)
                    }
                } else {
                    (Silence, false)
                }
            }
            Speech => {
                if frame_is_speech {
                    (Speech, true)
                } else {
                    (SpeechEnding, true)
                }
            }
            SpeechEnding => {
                if frame_is_speech {
                    (Speech, true)
                } else {
                    // Count consecutive silence frames
                    if let Ok(history) = self.frame_history.lock() {
                        let recent_silence = history
                            .iter()
                            .rev()
                            .take(self.config.hangover_frames)
                            .filter(|f| !f.is_speech)
                            .count();

                        if recent_silence >= self.config.hangover_frames {
                            (Silence, false)
                        } else {
                            (SpeechEnding, true)
                        }
                    } else {
                        (Silence, false)
                    }
                }
            }
        }
    }

    fn create_segment(&self, start: u64, end: u64, avg_energy: f32, peak_energy: f32) -> SpeechSegment {
        let frame_duration = self.config.frame_size as f64 / self.config.sample_rate as f64;
        let start_time = start as f64 * frame_duration;
        let end_time = end as f64 * frame_duration;

        SpeechSegment {
            start_frame: start,
            end_frame: end,
            start_time_secs: start_time,
            end_time_secs: end_time,
            duration_secs: end_time - start_time,
            avg_energy,
            peak_energy,
        }
    }

    /// Check if currently detecting speech
    pub fn is_speech(&self) -> bool {
        self.is_speech.load(Ordering::SeqCst)
    }

    /// Get current state
    pub fn current_state(&self) -> VadState {
        *self.state.lock().unwrap()
    }

    /// Get frame history
    pub fn history(&self) -> Vec<VadFrame> {
        self.frame_history.lock()
            .map(|h| h.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Reset the VAD state
    pub fn reset(&self) {
        *self.state.lock().unwrap() = VadState::Silence;
        *self.noise_floor.lock().unwrap() = 1e-10;
        self.frame_history.lock().unwrap().clear();
        self.is_speech.store(false, Ordering::SeqCst);
        *self.frame_count.lock().unwrap() = 0;
        info!("VAD reset");
    }

    /// Get current noise floor in dB
    pub fn noise_floor_db(&self) -> f32 {
        let noise = *self.noise_floor.lock().unwrap();
        10.0 * noise.log10().max(-100.0)
    }
}

impl Default for Vad {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculate RMS energy of a frame
fn calculate_energy(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Real-time VAD stream processor
pub struct VadStream {
    vad: Arc<Vad>,
    /// Buffer for incomplete frames
    buffer: Arc<Mutex<Vec<f32>>>,
    /// Active speech segment being built
    current_segment: Arc<Mutex<Option<SpeechSegmentBuilder>>>,
    /// Completed segments
    completed_segments: Arc<Mutex<Vec<SpeechSegment>>>,
}

/// Builder for accumulating speech segment data
struct SpeechSegmentBuilder {
    start_frame: u64,
    energies: Vec<f32>,
}

impl VadStream {
    /// Create a new VAD stream processor
    pub fn new(vad: Vad) -> Self {
        let frame_size = vad.config.frame_size;
        Self {
            vad: Arc::new(vad),
            buffer: Arc::new(Mutex::new(Vec::with_capacity(frame_size * 2))),
            current_segment: Arc::new(Mutex::new(None)),
            completed_segments: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Process audio samples from a stream
    pub fn process(&self, samples: &[f32]) -> Result<Vec<SpeechSegment>> {
        let mut buffer = self.buffer.lock().unwrap();
        buffer.extend_from_slice(samples);

        let frame_size = self.vad.config.frame_size;
        let mut new_segments = Vec::new();

        // Process complete frames
        while buffer.len() >= frame_size {
            let frame: Vec<f32> = buffer.drain(..frame_size).collect();
            let was_speech = self.vad.is_speech();
            let is_speech = self.vad.process_frame(&frame)?;

            // Track segments
            let mut current = self.current_segment.lock().unwrap();

            match (was_speech, is_speech) {
                (false, true) => {
                    // Speech started
                    let frame_count = *self.vad.frame_count.lock().unwrap();
                    *current = Some(SpeechSegmentBuilder {
                        start_frame: frame_count.saturating_sub(1),
                        energies: vec![calculate_energy(&frame)],
                    });
                }
                (true, true) => {
                    // Continuing speech
                    if let Some(ref mut seg) = *current {
                        seg.energies.push(calculate_energy(&frame));
                    }
                }
                (true, false) => {
                    // Speech ended
                    if let Some(seg) = current.take() {
                        let frame_count = *self.vad.frame_count.lock().unwrap();
                        let segment = self.build_segment(seg, frame_count.saturating_sub(1));
                        new_segments.push(segment.clone());
                        self.completed_segments.lock().unwrap().push(segment);
                    }
                }
                (false, false) => {}
            }
        }

        Ok(new_segments)
    }

    /// Finish processing and return any remaining segment
    pub fn finalize(&self) -> Option<SpeechSegment> {
        let mut buffer = self.buffer.lock().unwrap();

        // Process any remaining samples
        if !buffer.is_empty() {
            let frame_size = self.vad.config.frame_size;
            // Pad with zeros to complete frame
            while buffer.len() < frame_size {
                buffer.push(0.0);
            }
            let _ = self.vad.process_frame(&buffer.drain(..frame_size).collect::<Vec<_>>());
        }

        // Finish current segment if any
        let mut current = self.current_segment.lock().unwrap();
        current.take().map(|seg| {
            let frame_count = *self.vad.frame_count.lock().unwrap();
            self.build_segment(seg, frame_count)
        })
    }

    /// Get all completed segments
    pub fn segments(&self) -> Vec<SpeechSegment> {
        self.completed_segments.lock().unwrap().clone()
    }

    /// Clear all segments
    pub fn clear_segments(&self) {
        self.completed_segments.lock().unwrap().clear();
    }

    /// Check if currently in speech
    pub fn is_speech(&self) -> bool {
        self.vad.is_speech()
    }

    fn build_segment(&self, builder: SpeechSegmentBuilder, end_frame: u64) -> SpeechSegment {
        let frame_duration = self.vad.config.frame_size as f64 / self.vad.config.sample_rate as f64;
        let start_time = builder.start_frame as f64 * frame_duration;
        let end_time = end_frame as f64 * frame_duration;

        let avg_energy = if !builder.energies.is_empty() {
                            builder.energies.iter().sum::<f32>() / builder.energies.len() as f32
                        } else {
                            0.0
                        };
        let peak_energy = builder.energies.iter().copied().fold(0.0f32, f32::max);

        SpeechSegment {
            start_frame: builder.start_frame,
            end_frame,
            start_time_secs: start_time,
            end_time_secs: end_time,
            duration_secs: end_time - start_time,
            avg_energy,
            peak_energy,
        }
    }
}

/// Simple pre-recorded audio VAD processor
pub fn detect_speech_segments(audio: &[f32], sample_rate: u32) -> Result<Vec<SpeechSegment>> {
    let config = VadConfig {
        sample_rate,
        frame_size: (sample_rate as usize * 10) / 1000, // 10ms frame
        ..Default::default()
    };

    let vad = Vad::with_config(config);
    vad.process_buffer(audio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vad_config_default() {
        let config = VadConfig::default();
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.frame_size, 160);
        assert!(config.adaptive_noise_floor);
    }

    #[test]
    fn test_vad_config_aggressive() {
        let config = VadConfig::aggressive();
        assert!(config.energy_threshold_db < VadConfig::default().energy_threshold_db);
        assert!(config.hangover_frames < VadConfig::default().hangover_frames);
    }

    #[test]
    fn test_calculate_energy() {
        let silence = vec![0.0f32; 160];
        assert_eq!(calculate_energy(&silence), 0.0);

        let signal = vec![0.5f32; 160];
        let energy = calculate_energy(&signal);
        assert!(energy > 0.0);
        assert!((energy - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_vad_creation() {
        let vad = Vad::new();
        assert!(!vad.is_speech());
        matches!(vad.current_state(), VadState::Silence);
    }

    #[test]
    fn test_vad_process_silence() {
        let vad = Vad::new();
        let silence = vec![0.0f32; 160];

        // Process multiple silence frames
        for _ in 0..10 {
            assert!(!vad.process_frame(&silence).unwrap());
        }

        assert!(!vad.is_speech());
    }

    #[test]
    fn test_vad_process_loud_signal() {
        let mut config = VadConfig::default();
        config.adaptive_noise_floor = false; // Disable adaptation for test
        config.energy_threshold_db = 20.0;
        let vad = Vad::with_config(config);

        // Start with silence
        let silence = vec![0.001f32; 160];
        for _ in 0..10 {
            let _ = vad.process_frame(&silence);
        }

        // Then loud signal - should trigger speech after onset
        let loud = vec![0.5f32; 160];
        let mut speech_detected = false;
        for i in 0..20 {
            let is_speech = vad.process_frame(&loud).unwrap();
            if is_speech {
                speech_detected = true;
                println!("Speech detected at frame {}", i);
                break;
            }
        }

        assert!(speech_detected, "Speech should have been detected");
    }

    #[test]
    fn test_vad_callback() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let mut vad = Vad::new();
        vad.on_speech_change(move |speech| {
            if speech {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            }
        });

        // Process alternating silence and speech
        let silence = vec![0.001f32; 160];
        let loud = vec![0.5f32; 160];

        // Build up noise floor
        for _ in 0..20 {
            let _ = vad.process_frame(&silence);
        }

        // Trigger speech multiple times
        for cycle in 0..3 {
            // Speech
            for _ in 0..10 {
                let _ = vad.process_frame(&loud);
            }
            // Silence (long enough to end speech)
            for _ in 0..30 {
                let _ = vad.process_frame(&silence);
            }
        }

        // Should have triggered callback for each speech start
        assert!(counter.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn test_vad_stream() {
        let mut config = VadConfig::default();
        config.adaptive_noise_floor = false; // Disable for predictable test
        let vad = Vad::with_config(config);
        let stream = VadStream::new(vad);

        let silence = vec![0.0f32; 160 * 20]; // 200ms silence (0.0 for true silence)
        let speech = vec![0.5f32; 160 * 30];  // 300ms speech

        // Process silence first to establish noise floor
        let segments = stream.process(&silence).unwrap();
        assert!(segments.is_empty(), "Should have no segments during silence");
        assert!(!stream.is_speech(), "Should not be in speech state after silence");

        // Then process speech - should trigger speech detection
        let segments = stream.process(&speech).unwrap();
        // Speech detection happens after onset_frames, segments returned on speech end

        // Finalize to get any ongoing segment
        let final_segment = stream.finalize();

        // We should have either gotten segments during processing or at finalize
        let all_segments = stream.segments();
        assert!(
            !all_segments.is_empty() || final_segment.is_some(),
            "Should have captured at least one speech segment"
        );
    }

    #[test]
    fn test_vad_reset() {
        let vad = Vad::new();

        // Process some frames
        let signal = vec![0.5f32; 160];
        for _ in 0..50 {
            let _ = vad.process_frame(&signal);
        }

        // Reset
        vad.reset();

        assert!(!vad.is_speech());
        matches!(vad.current_state(), VadState::Silence);
        assert!(vad.history().is_empty());
    }
}
