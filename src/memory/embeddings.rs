//! Real embedding models via API or local inference
//!
//! Supports:
//! - OpenRouter embeddings (uses your existing API key)
//! - OpenAI embeddings directly
//! - Local model via candle (offline, free)

use anyhow::{Result, Context};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Available embedding providers
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingProvider {
    /// Use OpenRouter (same API key as chat, supports many models)
    OpenRouter,
    /// Use OpenAI directly (requires separate key)
    OpenAi,
    /// Use local model (free, offline, requires model download)
    Local,
    /// Use local hash-based fallback (free, no download, lower quality)
    Hash,
}

impl Default for EmbeddingProvider {
    fn default() -> Self {
        Self::OpenRouter // Uses existing OpenRouter API key
    }
}

impl std::fmt::Display for EmbeddingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenRouter => write!(f, "OpenRouter"),
            Self::OpenAi => write!(f, "OpenAI"),
            Self::Local => write!(f, "Local"),
            Self::Hash => write!(f, "Hash"),
        }
    }
}

/// Embedding model configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Model provider
    pub provider: EmbeddingProvider,
    /// Model name (provider-specific)
    pub model_name: String,
    /// API key (for cloud providers) - if None, tries to get from keyring
    pub api_key: Option<String>,
    /// Cache directory for model files (for local models)
    pub cache_dir: Option<PathBuf>,
    /// Maximum sequence length
    pub max_length: usize,
    /// Embedding dimension
    pub embedding_dim: usize,
    /// Batch size for API calls
    pub batch_size: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        let cache_dir = dirs::cache_dir()
            .map(|d| d.join("my-agent").join("models"));

        Self {
            provider: EmbeddingProvider::OpenRouter,
            // OpenRouter supports OpenAI embedding models
            model_name: "openai/text-embedding-3-small".to_string(),
            api_key: None, // Will use keyring key
            cache_dir,
            max_length: 8191,
            embedding_dim: 1536,
            batch_size: 100,
        }
    }
}

impl EmbeddingConfig {
    /// Create config for OpenRouter (uses existing API key)
    pub fn openrouter() -> Self {
        Self::default()
    }

    /// Create config for OpenAI directly
    pub fn openai(api_key: String) -> Self {
        Self {
            provider: EmbeddingProvider::OpenAi,
            model_name: "text-embedding-3-small".to_string(),
            api_key: Some(api_key),
            ..Default::default()
        }
    }

    /// Create config for local model (all-MiniLM-L6-v2)
    pub fn local() -> Self {
        Self {
            provider: EmbeddingProvider::Local,
            model_name: "sentence-transformers/all-MiniLM-L6-v2".to_string(),
            api_key: None,
            embedding_dim: 384, // MiniLM dimension
            max_length: 512,
            ..Default::default()
        }
    }

    /// Create config for hash-based fallback (no dependencies)
    pub fn hash() -> Self {
        Self {
            provider: EmbeddingProvider::Hash,
            model_name: "hash-based".to_string(),
            api_key: None,
            embedding_dim: 384,
            max_length: 512,
            ..Default::default()
        }
    }
}

/// Embedding model wrapper supporting multiple backends
pub struct EmbeddingModel {
    config: EmbeddingConfig,
    client: Client,
    initialized: bool,
    /// Cache for recently computed embeddings
    cache: Arc<RwLock<lru::LruCache<String, Vec<f32>>>>,
}

impl EmbeddingModel {
    /// Create a new embedding model with the given configuration
    pub async fn new(config: EmbeddingConfig) -> Result<Self> {
        info!("Initializing embedding model: {} ({})", config.model_name, config.provider);

        // Ensure cache directory exists
        if let Some(ref cache_dir) = config.cache_dir {
            tokio::fs::create_dir_all(cache_dir).await
                .with_context(|| "Failed to create model cache directory")?;
        }

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        // Create LRU cache for embeddings (1000 entries)
        let cache = Arc::new(RwLock::new(
            lru::LruCache::new(std::num::NonZeroUsize::new(1000).unwrap())
        ));

        Ok(Self {
            config,
            client,
            initialized: true,
            cache,
        })
    }

    /// Create with API key from keyring (uses OpenRouter by default)
    pub async fn with_keyring_key() -> Result<Self> {
        let mut config = EmbeddingConfig::default();

        // Try to get OpenRouter API key from keyring
        if let Ok(key) = crate::security::get_api_key() {
            if !key.is_empty() {
                config.api_key = Some(key);
                info!("Using OpenRouter API key from keyring for embeddings");
            }
        }

        // If no keyring key, try environment
        if config.api_key.is_none() {
            if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
                config.api_key = Some(key);
                info!("Using OPENROUTER_API_KEY env var for embeddings");
            }
        }

        // Still no key? Fall back to hash-based
        if config.api_key.is_none() {
            warn!("No API key found for embeddings, using hash-based fallback");
            config = EmbeddingConfig::hash();
        }

        Self::new(config).await
    }

    /// Generate an embedding for the given text
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        if !self.initialized {
            return Err(anyhow::anyhow!("Embedding model not initialized"));
        }

        // Check cache first
        let cache_key = self.cache_key(text);
        {
            let mut cache = self.cache.write().await;
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached.clone());
            }
        }

        // Generate embedding based on provider
        let embedding = match self.config.provider {
            EmbeddingProvider::OpenRouter | EmbeddingProvider::OpenAi => {
                self.embed_via_api(text).await?
            }
            EmbeddingProvider::Local => self.embed_local(text)?,
            EmbeddingProvider::Hash => self.embed_hash(text)?,
        };

        // Cache the result
        {
            let mut cache = self.cache.write().await;
            cache.put(cache_key, embedding.clone());
        }

        Ok(embedding)
    }

    /// Generate embeddings for multiple texts (batched)
    pub async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());

        // Process in batches
        for chunk in texts.chunks(self.config.batch_size) {
            let batch_embeddings = self.embed_batch_api(chunk).await?;
            results.extend(batch_embeddings);
        }

        Ok(results)
    }

    /// Generate embedding via OpenAI/OpenRouter API
    async fn embed_via_api(&self, text: &str) -> Result<Vec<f32>> {
        let api_key = self.config.api_key.as_ref()
            .ok_or_else(|| anyhow::anyhow!("API key not configured for embeddings"))?;

        // Truncate text if too long
        let text = self.truncate_text(text);

        let request = EmbeddingRequest {
            model: self.config.model_name.clone(),
            input: vec![text.to_string()],
        };

        let (url, mut headers) = match self.config.provider {
            EmbeddingProvider::OpenRouter => {
                let url = "https://openrouter.ai/api/v1/embeddings";
                let headers = vec![
                    ("Authorization", format!("Bearer {}", api_key)),
                    ("Content-Type", "application/json".to_string()),
                    ("HTTP-Referer", "https://github.com/my-agent".to_string()),
                    ("X-Title", "my-agent".to_string()),
                ];
                (url, headers)
            }
            EmbeddingProvider::OpenAi => {
                let url = "https://api.openai.com/v1/embeddings";
                let headers = vec![
                    ("Authorization", format!("Bearer {}", api_key)),
                    ("Content-Type", "application/json".to_string()),
                ];
                (url, headers)
            }
            _ => return Err(anyhow::anyhow!("Invalid provider for API embeddings")),
        };

        let mut req = self.client.post(url);
        for (key, value) in headers {
            req = req.header(key, value);
        }

        let response = req
            .json(&request)
            .send()
            .await
            .context("Failed to send embedding request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            warn!("Embedding API error: {}", error_text);
            return Err(anyhow::anyhow!("Embedding API error: {}", error_text));
        }

        let result: EmbeddingResponse = response.json().await
            .context("Failed to parse embedding response")?;

        let embedding = result.data.first()
            .map(|d| d.embedding.clone())
            .ok_or_else(|| anyhow::anyhow!("No embedding in response"))?;

        Ok(embedding)
    }

    /// Generate embeddings for a batch via API
    async fn embed_batch_api(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let api_key = self.config.api_key.as_ref()
            .ok_or_else(|| anyhow::anyhow!("API key not configured for embeddings"))?;

        let request = EmbeddingRequest {
            model: self.config.model_name.clone(),
            input: texts.to_vec(),
        };

        let (url, mut headers) = match self.config.provider {
            EmbeddingProvider::OpenRouter => {
                let url = "https://openrouter.ai/api/v1/embeddings";
                let headers = vec![
                    ("Authorization", format!("Bearer {}", api_key)),
                    ("Content-Type", "application/json".to_string()),
                    ("HTTP-Referer", "https://github.com/my-agent".to_string()),
                    ("X-Title", "my-agent".to_string()),
                ];
                (url, headers)
            }
            EmbeddingProvider::OpenAi => {
                let url = "https://api.openai.com/v1/embeddings";
                let headers = vec![
                    ("Authorization", format!("Bearer {}", api_key)),
                    ("Content-Type", "application/json".to_string()),
                ];
                (url, headers)
            }
            _ => return Err(anyhow::anyhow!("Invalid provider for API embeddings")),
        };

        let mut req = self.client.post(url);
        for (key, value) in headers {
            req = req.header(key, value);
        }

        let response = req
            .json(&request)
            .send()
            .await
            .context("Failed to send batch embedding request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Batch embedding API error: {}", error_text));
        }

        let result: EmbeddingResponse = response.json().await
            .context("Failed to parse batch embedding response")?;

        // Sort by index to maintain order
        let mut data = result.data;
        data.sort_by_key(|d| d.index);

        Ok(data.into_iter().map(|d| d.embedding).collect())
    }

    /// Local embedding using candle (requires model download)
    /// Falls back to hash-based if model not available
    fn embed_local(&self, text: &str) -> Result<Vec<f32>> {
        // For now, fall back to hash-based
        // In a full implementation, this would:
        // 1. Load the model from HuggingFace Hub if not cached
        // 2. Tokenize the text
        // 3. Run inference through the model
        // 4. Pool and normalize the embeddings
        info!("Local model not yet implemented, using hash-based embedding");
        self.embed_hash(text)
    }

    /// Hash-based embedding (deterministic, no model needed)
    fn embed_hash(&self, text: &str) -> Result<Vec<f32>> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let dim = self.config.embedding_dim;
        let mut embedding = vec![0.0f32; dim];

        // Generate embeddings from n-grams
        let tokens: Vec<&str> = text.split_whitespace().collect();

        for (i, token) in tokens.iter().enumerate() {
            let mut hasher = DefaultHasher::new();
            token.hash(&mut hasher);
            token.to_lowercase().hash(&mut hasher);
            (i as u64).hash(&mut hasher);
            let hash = hasher.finish();

            for j in 0..dim {
                let mut hasher = DefaultHasher::new();
                hash.hash(&mut hasher);
                (j as u64).hash(&mut hasher);
                let val = hasher.finish();
                let normalized = (val as f64 / u64::MAX as f64) * 2.0 - 1.0;
                embedding[j] += normalized as f32;
            }
        }

        // Normalize the embedding
        let mag: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if mag > 0.0 {
            for val in embedding.iter_mut() {
                *val /= mag;
            }
        }

        Ok(embedding)
    }

    /// Truncate text to maximum length
    fn truncate_text<'a>(&self, text: &'a str) -> &'a str {
        // Rough estimate: 4 chars per token
        let max_chars = self.config.max_length * 4;
        if text.len() > max_chars {
            &text[..max_chars]
        } else {
            text
        }
    }

    /// Generate cache key for text
    fn cache_key(&self, text: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// Check if the model is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Get the embedding dimension
    pub fn dimension(&self) -> usize {
        self.config.embedding_dim
    }

    /// Get the model name
    pub fn model_name(&self) -> &str {
        &self.config.model_name
    }

    /// Check if using real embeddings (not local fallback)
    pub fn uses_real_embeddings(&self) -> bool {
        matches!(self.config.provider, EmbeddingProvider::OpenRouter | EmbeddingProvider::OpenAi)
            && self.config.api_key.is_some()
    }
}

/// OpenAI embedding request
#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
}

/// OpenAI embedding response
#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
    #[allow(dead_code)]
    model: String,
    #[allow(dead_code)]
    usage: EmbeddingUsage,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    #[allow(dead_code)]
    index: i32,
}

#[derive(Debug, Deserialize)]
struct EmbeddingUsage {
    #[allow(dead_code)]
    prompt_tokens: i32,
    #[allow(dead_code)]
    total_tokens: i32,
}

/// Calculate cosine similarity between two vectors
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }

    dot / (mag_a * mag_b)
}

/// Calculate euclidean distance between two vectors
pub fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return f32::MAX;
    }

    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_embedding() {
        let config = EmbeddingConfig {
            provider: EmbeddingProvider::Local,
            ..Default::default()
        };
        let model = EmbeddingModel::new(config).await.unwrap();

        let emb1 = model.embed("hello world").await.unwrap();
        let emb2 = model.embed("hello world").await.unwrap();
        let emb3 = model.embed("goodbye moon").await.unwrap();

        // Same text should produce same embedding
        assert_eq!(emb1, emb2);

        // Different text should produce different embedding
        assert_ne!(emb1, emb3);

        // Embedding should be normalized
        let mag: f32 = emb1.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((mag - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let c = vec![0.0, 1.0, 0.0];

        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);
        assert!((cosine_similarity(&a, &c) - 0.0).abs() < 0.001);
    }
}