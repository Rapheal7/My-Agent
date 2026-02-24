//! Result Synthesis Module
//!
//! Intelligently combines outputs from multiple agents into cohesive voice responses.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────┐     ┌──────────────┐     ┌─────────────────┐
//! │ Agent 1     │────→│              │     │                 │
//! │ Output      │     │   Synthesis  │────→│  Voice Response │
//! └─────────────┘     │   Engine     │     │  (TTS-ready)    │
//! ┌─────────────┐     │              │     │                 │
//! │ Agent 2     │────→│  - Merge     │     └─────────────────┘
//! │ Output      │     │  - Summarize │
//! └─────────────┘     │  - Prioritize│
//! ┌─────────────┐     │  - Format    │
//! │ Agent 3     │────→│              │
//! │ Output      │     └──────────────┘
//! └─────────────┘
//! ```
//!
//! # Features
//!
//! - **Multi-agent merging**: Combines parallel agent outputs intelligently
//! - **Sequential synthesis**: Builds cumulative results for multi-step tasks
//! - **Confidence scoring**: Ranks and prioritizes agent outputs
//! - **Voice-optimized formatting**: Structures text for natural TTS playback
//! - **Context preservation**: Maintains conversation flow across agent handoffs

use anyhow::{Result, Context, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, debug, warn};

use crate::orchestrator::{OrchestrationPlan, AgentSpec, ExecutionMode};
use crate::orchestrator::bus::AgentMessage;
#[cfg(feature = "voice")]
use crate::voice::tts::{TtsEngine, TtsConfig};

/// Synthesis strategy for combining agent outputs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SynthesisStrategy {
    /// Concatenate outputs in order (simplest)
    Concatenate,
    /// Merge outputs, removing duplicates
    Merge,
    /// Summarize all outputs into a cohesive response
    Summarize,
    /// Prioritize by confidence score
    Prioritize,
    /// Use best single output
    BestOnly,
    /// Custom merge based on agent types
    SmartMerge,
}

impl Default for SynthesisStrategy {
    fn default() -> Self {
        SynthesisStrategy::SmartMerge
    }
}

/// Configuration for result synthesis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisConfig {
    /// Strategy for combining outputs
    pub strategy: SynthesisStrategy,
    /// Maximum length of synthesized response (characters)
    pub max_response_length: usize,
    /// Enable voice-optimized formatting
    pub voice_optimized: bool,
    /// Add transition phrases between agent outputs
    pub add_transitions: bool,
    /// Confidence threshold for including agent output (0.0-1.0)
    pub confidence_threshold: f32,
    /// Enable contradiction detection
    pub detect_contradictions: bool,
    /// TTS configuration for voice output
    #[cfg(feature = "voice")]
    pub tts_config: Option<TtsConfig>,
}

impl Default for SynthesisConfig {
    fn default() -> Self {
        Self {
            strategy: SynthesisStrategy::default(),
            max_response_length: 2000,
            voice_optimized: true,
            add_transitions: true,
            confidence_threshold: 0.5,
            detect_contradictions: true,
            #[cfg(feature = "voice")]
            tts_config: None,
        }
    }
}

/// Individual agent result with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    /// Agent ID
    pub agent_id: String,
    /// Agent type/capability
    pub agent_type: String,
    /// Model used
    pub model: String,
    /// Raw output text
    pub output: String,
    /// Confidence score (0.0-1.0)
    pub confidence: f32,
    /// Execution time in seconds
    pub execution_time_secs: f64,
    /// Tokens used
    pub tokens_used: Option<u32>,
    /// Whether this result has errors
    pub has_error: bool,
    /// Error message if any
    pub error_message: Option<String>,
}

impl AgentResult {
    /// Create a new agent result
    pub fn new(
        agent_id: impl Into<String>,
        agent_type: impl Into<String>,
        model: impl Into<String>,
        output: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            agent_type: agent_type.into(),
            model: model.into(),
            output: output.into(),
            confidence: 1.0,
            execution_time_secs: 0.0,
            tokens_used: None,
            has_error: false,
            error_message: None,
        }
    }

    /// Create an error result
    pub fn error(
        agent_id: impl Into<String>,
        agent_type: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            agent_type: agent_type.into(),
            model: String::new(),
            output: String::new(),
            confidence: 0.0,
            execution_time_secs: 0.0,
            tokens_used: None,
            has_error: true,
            error_message: Some(error.into()),
        }
    }

    /// Calculate quality score based on multiple factors
    pub fn quality_score(&self) -> f32 {
        if self.has_error {
            return 0.0;
        }

        let length_score = if self.output.len() > 10 { 1.0 } else { 0.5 };
        let confidence_score = self.confidence;

        // Combine scores (weighted average)
        (confidence_score * 0.6 + length_score * 0.4).min(1.0)
    }
}

/// Synthesized result ready for voice output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesizedResult {
    /// Combined text response
    pub text: String,
    /// Voice-optimized version (if enabled)
    pub voice_text: Option<String>,
    /// Contributing agent IDs
    pub contributing_agents: Vec<String>,
    /// Overall confidence
    pub overall_confidence: f32,
    /// Synthesis metadata
    pub metadata: SynthesisMetadata,
}

/// Metadata about the synthesis process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisMetadata {
    /// Strategy used
    pub strategy: SynthesisStrategy,
    /// Number of inputs
    pub input_count: usize,
    /// Number of outputs used
    pub outputs_used: usize,
    /// Synthesis duration
    pub synthesis_time_ms: u64,
    /// Any conflicts detected
    pub conflicts_detected: Vec<String>,
    /// Processing steps taken
    pub processing_steps: Vec<String>,
}

impl Default for SynthesisMetadata {
    fn default() -> Self {
        Self {
            strategy: SynthesisStrategy::default(),
            input_count: 0,
            outputs_used: 0,
            synthesis_time_ms: 0,
            conflicts_detected: Vec::new(),
            processing_steps: Vec::new(),
        }
    }
}

/// Result synthesis engine
pub struct SynthesisEngine {
    config: SynthesisConfig,
}

impl SynthesisEngine {
    /// Create a new synthesis engine with default config
    pub fn new() -> Self {
        Self {
            config: SynthesisConfig::default(),
        }
    }

    /// Create with custom config
    pub fn with_config(config: SynthesisConfig) -> Self {
        Self { config }
    }

    /// Synthesize results from multiple agents
    pub fn synthesize(&self, results: Vec<AgentResult>) -> Result<SynthesizedResult> {
        let start = std::time::Instant::now();
        let mut metadata = SynthesisMetadata::default();
        metadata.strategy = self.config.strategy;
        metadata.input_count = results.len();

        debug!("Synthesizing {} agent results using {:?} strategy",
               results.len(), self.config.strategy);

        // Filter out error results and low confidence
        let valid_results: Vec<&AgentResult> = results
            .iter()
            .filter(|r| !r.has_error && r.confidence >= self.config.confidence_threshold)
            .collect();

        if valid_results.is_empty() {
            warn!("No valid results to synthesize");
            return Ok(SynthesizedResult {
                text: "I wasn't able to get any results. Could you try rephrasing your request?".to_string(),
                voice_text: None,
                contributing_agents: Vec::new(),
                overall_confidence: 0.0,
                metadata,
            });
        }

        metadata.outputs_used = valid_results.len();

        // Apply synthesis strategy
        let synthesized = match self.config.strategy {
            SynthesisStrategy::Concatenate => {
                metadata.processing_steps.push("Concatenated outputs".to_string());
                self.concatenate(&valid_results)
            }
            SynthesisStrategy::Merge => {
                metadata.processing_steps.push("Merged outputs".to_string());
                self.merge(&valid_results)
            }
            SynthesisStrategy::Summarize => {
                metadata.processing_steps.push("Summarized outputs".to_string());
                self.summarize(&valid_results)
            }
            SynthesisStrategy::Prioritize => {
                metadata.processing_steps.push("Prioritized by confidence".to_string());
                self.prioritize(&valid_results)
            }
            SynthesisStrategy::BestOnly => {
                metadata.processing_steps.push("Selected best output".to_string());
                self.best_only(&valid_results)
            }
            SynthesisStrategy::SmartMerge => {
                metadata.processing_steps.push("Applied smart merge".to_string());
                self.smart_merge(&valid_results, &mut metadata)
            }
        };

        // Truncate if too long
        let text = if synthesized.len() > self.config.max_response_length {
            metadata.processing_steps.push(format!(
                "Truncated from {} to {} characters",
                synthesized.len(),
                self.config.max_response_length
            ));
            format!("{}...", &synthesized[..self.config.max_response_length])
        } else {
            synthesized
        };

        // Calculate overall confidence
        let overall_confidence = if valid_results.is_empty() {
            0.0
        } else {
            valid_results.iter().map(|r| r.confidence).sum::<f32>() / valid_results.len() as f32
        };

        // Generate voice-optimized text if enabled
        let voice_text = if self.config.voice_optimized {
            metadata.processing_steps.push("Generated voice-optimized text".to_string());
            Some(self.optimize_for_voice(&text))
        } else {
            None
        };

        metadata.synthesis_time_ms = start.elapsed().as_millis() as u64;

        let contributing_agents = valid_results
            .iter()
            .map(|r| r.agent_id.clone())
            .collect();

        Ok(SynthesizedResult {
            text,
            voice_text,
            contributing_agents,
            overall_confidence,
            metadata,
        })
    }

    /// Concatenate all outputs with separators
    fn concatenate(&self, results: &[&AgentResult]) -> String {
        let parts: Vec<String> = results
            .iter()
            .map(|r| r.output.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if self.config.add_transitions && parts.len() > 1 {
            parts.join(". Additionally, ")
        } else {
            parts.join(" ")
        }
    }

    /// Merge outputs removing duplicates
    fn merge(&self, results: &[&AgentResult]) -> String {
        let mut seen_sentences = std::collections::HashSet::new();
        let mut merged = Vec::new();

        for result in results {
            let sentences: Vec<&str> = result.output
                .split(|c| c == '.' || c == '!' || c == '?')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();

            for sentence in sentences {
                let normalized = sentence.to_lowercase();
                if !seen_sentences.contains(&normalized) {
                    seen_sentences.insert(normalized);
                    merged.push(sentence);
                }
            }
        }

        merged.join(". ") + "."
    }

    /// Summarize all outputs (simple concatenation with summary intro)
    fn summarize(&self, results: &[&AgentResult]) -> String {
        let content = self.concatenate(results);

        // Simple summarization - in production would use LLM
        if results.len() == 1 {
            content
        } else {
            format!("Based on the analysis from {} agents: {}",
                    results.len(), content)
        }
    }

    /// Prioritize by confidence, using highest first
    fn prioritize(&self, results: &[&AgentResult]) -> String {
        let mut sorted: Vec<_> = results.to_vec();
        sorted.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());

        // Take top results until we reach length limit
        let mut output = String::new();
        for result in sorted {
            if output.len() + result.output.len() > self.config.max_response_length {
                break;
            }
            if !output.is_empty() {
                output.push_str(" ");
            }
            output.push_str(&result.output);
        }

        output
    }

    /// Use only the best result
    fn best_only(&self, results: &[&AgentResult]) -> String {
        results
            .iter()
            .max_by(|a, b| a.quality_score().partial_cmp(&b.quality_score()).unwrap())
            .map(|r| r.output.clone())
            .unwrap_or_default()
    }

    /// Smart merge based on agent types
    fn smart_merge(&self, results: &[&AgentResult], metadata: &mut SynthesisMetadata) -> String {
        // Group results by agent type
        let mut by_type: HashMap<&str, Vec<&AgentResult>> = HashMap::new();
        for result in results {
            by_type
                .entry(&result.agent_type)
                .or_default()
                .push(result);
        }

        metadata.processing_steps.push(format!(
            "Grouped {} results into {} agent types",
            results.len(),
            by_type.len()
        ));

        // Process each type differently
        let mut sections = Vec::new();

        // Code agents: take the most detailed output
        if let Some(code_results) = by_type.get("code") {
            let best = code_results
                .iter()
                .max_by(|a, b| a.output.len().cmp(&b.output.len()))
                .map(|r| r.output.clone())
                .unwrap_or_default();
            if !best.is_empty() {
                sections.push(format!("Here's the implementation: {}", best));
            }
        }

        // Research agents: merge unique findings
        if let Some(research_results) = by_type.get("research") {
            let findings = self.merge_unique_findings(research_results);
            if !findings.is_empty() {
                sections.push(format!("Based on my research: {}", findings));
            }
        }

        // Analysis agents: summarize insights
        if let Some(analysis_results) = by_type.get("analysis") {
            let insights = analysis_results
                .iter()
                .map(|r| r.output.clone())
                .collect::<Vec<_>>()
                .join(". ");
            if !insights.is_empty() {
                sections.push(format!("Analysis shows: {}", insights));
            }
        }

        // General agents: use as fallback
        if sections.is_empty() {
            if let Some(general_results) = by_type.get("general") {
                return self.concatenate(general_results);
            }
            // Last resort: concatenate all
            return self.concatenate(results);
        }

        sections.join(". ")
    }

    /// Merge unique findings from research agents
    fn merge_unique_findings(&self, results: &[&AgentResult]) -> String {
        let mut seen = std::collections::HashSet::new();
        let mut findings = Vec::new();

        for result in results {
            let sentences: Vec<&str> = result.output
                .split(|c| c == '.' || c == '!' || c == '?')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty() && s.len() > 10)
                .collect();

            for sentence in sentences {
                let key = sentence.to_lowercase();
                if !seen.contains(&key) {
                    seen.insert(key);
                    findings.push(sentence);
                }
            }
        }

        findings.join(". ")
    }

    /// Optimize text for voice output
    fn optimize_for_voice(&self, text: &str) -> String {
        let mut optimized = text.to_string();

        // Replace abbreviations with spoken forms
        optimized = optimized.replace("e.g.", "for example");
        optimized = optimized.replace("i.e.", "that is");
        optimized = optimized.replace("etc.", "and so on");
        optimized = optimized.replace("vs.", "versus");
        optimized = optimized.replace("Dr.", "Doctor");
        optimized = optimized.replace("Mr.", "Mister");
        optimized = optimized.replace("Mrs.", "Misses");
        optimized = optimized.replace("Ms.", "Miss");

        // Add pauses for long sentences
        optimized = optimized.replace(", ", ", <break time=\"200ms\"/> ");
        optimized = optimized.replace("; ", "; <break time=\"300ms\"/> ");

        // Remove markdown formatting
        optimized = optimized.replace("**", "");
        optimized = optimized.replace("*", "");
        optimized = optimized.replace("`", "");
        optimized = optimized.replace("# ", "");
        optimized = optimized.replace("## ", "");
        optimized = optimized.replace("### ", "");

        // Format numbers for better speech
        optimized = self.spell_out_numbers(&optimized);

        optimized
    }

    /// Spell out certain numbers for better speech
    fn spell_out_numbers(&self, text: &str) -> String {
        // Simple replacement for common patterns
        // In production, use a proper number-to-words library
        text.replace("$1", "one dollar")
            .replace("$2", "two dollars")
            .replace("$3", "three dollars")
            .replace("100%", "one hundred percent")
            .replace("50%", "fifty percent")
            .replace("25%", "twenty-five percent")
    }

    /// Convert AgentMessage::TaskResult to AgentResult
    pub fn from_task_result(
        agent_id: impl Into<String>,
        agent_type: impl Into<String>,
        model: impl Into<String>,
        msg: &AgentMessage,
    ) -> Option<AgentResult> {
        match msg {
            AgentMessage::TaskResult { output, success, .. } => {
                if *success {
                    Some(AgentResult::new(
                        agent_id,
                        agent_type,
                        model,
                        output.clone(),
                    ))
                } else {
                    Some(AgentResult::error(
                        agent_id,
                        agent_type,
                        output.clone(),
                    ))
                }
            }
            _ => None,
        }
    }

    /// Create synthesis engine from orchestration plan
    pub fn from_plan(plan: &OrchestrationPlan) -> Self {
        let strategy = match plan.execution_mode {
            ExecutionMode::Parallel => SynthesisStrategy::SmartMerge,
            ExecutionMode::Sequential => SynthesisStrategy::Concatenate,
        };

        Self::with_config(SynthesisConfig {
            strategy,
            ..Default::default()
        })
    }
}

impl Default for SynthesisEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Voice response builder for multi-turn conversations
pub struct VoiceResponseBuilder {
    config: SynthesisConfig,
    results: Vec<AgentResult>,
    conversation_context: Option<String>,
}

impl VoiceResponseBuilder {
    /// Create a new response builder
    pub fn new() -> Self {
        Self {
            config: SynthesisConfig::default(),
            results: Vec::new(),
            conversation_context: None,
        }
    }

    /// Create with custom config
    pub fn with_config(config: SynthesisConfig) -> Self {
        Self {
            config,
            results: Vec::new(),
            conversation_context: None,
        }
    }

    /// Add an agent result
    pub fn add_result(&mut self, result: AgentResult) -> &mut Self {
        self.results.push(result);
        self
    }

    /// Add multiple results
    pub fn add_results(&mut self, results: Vec<AgentResult>) -> &mut Self {
        self.results.extend(results);
        self
    }

    /// Set conversation context
    pub fn with_context(&mut self, context: impl Into<String>) -> &mut Self {
        self.conversation_context = Some(context.into());
        self
    }

    /// Build the final synthesized response
    pub fn build(&self) -> Result<SynthesizedResult> {
        let engine = SynthesisEngine::with_config(self.config.clone());
        engine.synthesize(self.results.clone())
    }

    /// Build and synthesize speech
    #[cfg(feature = "voice")]
    pub async fn build_and_speak(&self, tts: &TtsEngine) -> Result<Vec<u8>> {
        let result = self.build()?;
        let text = result.voice_text.as_ref().unwrap_or(&result.text);

        let tts_result = tts.synthesize(text)?;

        // Convert samples to bytes
        let bytes: Vec<u8> = tts_result.samples
            .iter()
            .flat_map(|&sample| {
                let clipped = sample.clamp(-1.0, 1.0);
                let int_sample = (clipped * 32767.0) as i16;
                int_sample.to_le_bytes().to_vec()
            })
            .collect();

        Ok(bytes)
    }
}

impl Default for VoiceResponseBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper functions for common synthesis patterns
pub mod helpers {
    use super::*;

    /// Quick synthesis with default settings
    pub fn quick_synthesize(results: Vec<AgentResult>) -> Result<String> {
        let engine = SynthesisEngine::new();
        let result = engine.synthesize(results)?;
        Ok(result.text)
    }

    /// Synthesize for voice output
    pub fn synthesize_for_voice(results: Vec<AgentResult>) -> Result<String> {
        let config = SynthesisConfig {
            voice_optimized: true,
            add_transitions: true,
            ..Default::default()
        };
        let engine = SynthesisEngine::with_config(config);
        let result = engine.synthesize(results)?;
        Ok(result.voice_text.unwrap_or(result.text))
    }

    /// Merge code outputs (takes longest/most detailed)
    pub fn merge_code_outputs(results: Vec<AgentResult>) -> String {
        results
            .into_iter()
            .filter(|r| !r.has_error)
            .max_by(|a, b| a.output.len().cmp(&b.output.len()))
            .map(|r| r.output)
            .unwrap_or_else(|| "No valid code output found.".to_string())
    }

    /// Merge research outputs (unique findings)
    pub fn merge_research_outputs(results: Vec<AgentResult>) -> String {
        let mut seen = std::collections::HashSet::new();
        let mut findings = Vec::new();

        for result in results {
            let sentences: Vec<&str> = result.output
                .split(|c| c == '.' || c == '!' || c == '?')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();

            for sentence in sentences {
                let key = sentence.to_lowercase();
                if !seen.contains(&key) {
                    seen.insert(key);
                    findings.push(sentence.to_string());
                }
            }
        }

        findings.join(". ")
    }

    /// Create a natural transition between outputs
    pub fn add_transition(previous: &str, current: &str) -> String {
        let transitions = vec![
            "Additionally, ",
            "Furthermore, ",
            "Moreover, ",
            "In addition, ",
            "Also, ",
        ];

        // Simple hash to pick consistent transition
        let index = previous.len() % transitions.len();
        format!("{}{}", transitions[index], current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_synthesis_config_default() {
        let config = SynthesisConfig::default();
        assert_eq!(config.strategy, SynthesisStrategy::SmartMerge);
        assert_eq!(config.max_response_length, 2000);
        assert!(config.voice_optimized);
        assert!(config.add_transitions);
    }

    #[test]
    fn test_agent_result_creation() {
        let result = AgentResult::new("agent1", "code", "gpt-4", "Hello world");
        assert_eq!(result.agent_id, "agent1");
        assert_eq!(result.agent_type, "code");
        assert_eq!(result.output, "Hello world");
        assert_eq!(result.confidence, 1.0);
        assert!(!result.has_error);
    }

    #[test]
    fn test_agent_result_error() {
        let result = AgentResult::error("agent1", "code", "Connection failed");
        assert!(result.has_error);
        assert_eq!(result.error_message, Some("Connection failed".to_string()));
        assert_eq!(result.quality_score(), 0.0);
    }

    #[test]
    fn test_quality_score() {
        let mut result = AgentResult::new("agent1", "code", "gpt-4", "Hello");
        result.confidence = 0.8;
        assert!(result.quality_score() > 0.0);
        assert!(result.quality_score() <= 1.0);
    }

    #[test]
    fn test_concatenate_strategy() {
        let engine = SynthesisEngine::with_config(SynthesisConfig {
            strategy: SynthesisStrategy::Concatenate,
            add_transitions: false,
            ..Default::default()
        });

        let results = vec![
            AgentResult::new("a1", "code", "model", "First result"),
            AgentResult::new("a2", "code", "model", "Second result"),
        ];

        let result = engine.synthesize(results).unwrap();
        assert!(result.text.contains("First result"));
        assert!(result.text.contains("Second result"));
    }

    #[test]
    fn test_best_only_strategy() {
        let engine = SynthesisEngine::with_config(SynthesisConfig {
            strategy: SynthesisStrategy::BestOnly,
            ..Default::default()
        });

        let results = vec![
            AgentResult::new("a1", "code", "model", "Short"),
            AgentResult::new("a2", "code", "model", "This is a much longer and more detailed result"),
        ];

        let result = engine.synthesize(results).unwrap();
        // BestOnly picks the one with highest quality (based on length for now)
        assert!(result.text.contains("longer"));
    }

    #[test]
    fn test_voice_optimization() {
        let engine = SynthesisEngine::with_config(SynthesisConfig {
            voice_optimized: true,
            ..Default::default()
        });

        let text = "Use e.g. and i.e. for examples. Visit Dr. Smith at 100% capacity.";
        let optimized = engine.optimize_for_voice(text);

        assert!(optimized.contains("for example"));
        assert!(optimized.contains("that is"));
        assert!(optimized.contains("Doctor"));
        assert!(optimized.contains("one hundred percent"));
        assert!(!optimized.contains("e.g."));
        assert!(!optimized.contains("Dr."));
    }

    #[test]
    fn test_empty_results() {
        let engine = SynthesisEngine::new();
        let result = engine.synthesize(vec![]).unwrap();
        assert!(!result.text.is_empty()); // Should have fallback message
        assert_eq!(result.overall_confidence, 0.0);
    }

    #[test]
    fn test_error_results_filtered() {
        let engine = SynthesisEngine::new();
        let results = vec![
            AgentResult::error("a1", "code", "Failed"),
            AgentResult::new("a2", "code", "model", "Success"),
        ];

        let result = engine.synthesize(results).unwrap();
        assert!(result.text.contains("Success"));
        assert!(!result.text.contains("Failed"));
    }

    #[test]
    fn test_helpers_quick_synthesize() {
        let results = vec![
            AgentResult::new("a1", "code", "model", "Hello"),
            AgentResult::new("a2", "code", "model", "World"),
        ];

        let text = helpers::quick_synthesize(results).unwrap();
        assert!(!text.is_empty());
    }

    #[test]
    fn test_voice_response_builder() {
        let mut builder = VoiceResponseBuilder::new();
        builder
            .add_result(AgentResult::new("a1", "code", "model", "Result 1"))
            .add_result(AgentResult::new("a2", "code", "model", "Result 2"))
            .with_context("User asked about programming");

        let result = builder.build().unwrap();
        assert!(!result.text.is_empty());
    }
}
