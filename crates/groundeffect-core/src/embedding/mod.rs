//! Embedding pipeline using Candle with Metal acceleration
//!
//! Uses bge-base-en-v1.5 (or all-MiniLM-L6-v2) for text embeddings.
//! Supports both local (CPU/GPU) and remote (HTTP service) embedding generation.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use hf_hub::{api::sync::Api, Repo, RepoType};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokenizers::Tokenizer;
use tracing::{debug, info, warn};

use crate::config::{EmbeddingFallback, EmbeddingProvider, SearchConfig};
use crate::error::{Error, Result};
use crate::EMBEDDING_DIMENSION;

/// Supported embedding models
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingModel {
    /// bge-base-en-v1.5 (768 dimensions, high quality, standard BERT)
    BgeBaseEn,
    /// all-MiniLM-L6-v2 (384 dimensions, faster)
    MiniLML6,
}

impl EmbeddingModel {
    /// Get the HuggingFace model ID
    pub fn model_id(&self) -> &'static str {
        match self {
            EmbeddingModel::BgeBaseEn => "BAAI/bge-base-en-v1.5",
            EmbeddingModel::MiniLML6 => "sentence-transformers/all-MiniLM-L6-v2",
        }
    }

    /// Get the embedding dimension
    pub fn dimension(&self) -> usize {
        match self {
            EmbeddingModel::BgeBaseEn => 768,
            EmbeddingModel::MiniLML6 => 384,
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "bge-base-en-v1.5" | "bge-base" | "bge" => Some(EmbeddingModel::BgeBaseEn),
            "all-minilm-l6-v2" | "minilm" => Some(EmbeddingModel::MiniLML6),
            _ => None,
        }
    }
}

/// Embedding engine for generating text embeddings
pub struct EmbeddingEngine {
    model: Arc<RwLock<BertModel>>,
    tokenizer: Arc<Tokenizer>,
    device: Device,
    model_type: EmbeddingModel,
    max_length: usize,
}

impl EmbeddingEngine {
    /// Create a new embedding engine
    ///
    /// The `use_gpu` parameter enables GPU acceleration:
    /// - On macOS with `metal` feature: Uses Metal
    /// - On Linux with `cuda` feature: Uses CUDA
    /// - Otherwise: Falls back to CPU
    pub fn new(model_type: EmbeddingModel, use_gpu: bool) -> Result<Self> {
        info!(
            "Initializing embedding engine with model {:?}, gpu={}",
            model_type, use_gpu
        );

        // Select device - try GPU acceleration if enabled
        let device = if use_gpu {
            Self::select_gpu_device()
        } else {
            info!("GPU disabled, using CPU for embeddings");
            Device::Cpu
        };

        // Download model from HuggingFace
        let api = Api::new().map_err(|e| Error::ModelLoading(e.to_string()))?;
        let repo = api.repo(Repo::new(model_type.model_id().to_string(), RepoType::Model));

        info!("Loading model files from HuggingFace...");

        // Load tokenizer
        let tokenizer_path = repo
            .get("tokenizer.json")
            .map_err(|e| Error::ModelLoading(format!("Failed to get tokenizer: {}", e)))?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| Error::ModelLoading(format!("Failed to load tokenizer: {}", e)))?;

        // Load model config
        let config_path = repo
            .get("config.json")
            .map_err(|e| Error::ModelLoading(format!("Failed to get config: {}", e)))?;
        let config_str = std::fs::read_to_string(&config_path)?;
        let config: BertConfig = serde_json::from_str(&config_str)?;

        // Load model weights
        let weights_path = repo
            .get("model.safetensors")
            .or_else(|_| repo.get("pytorch_model.bin"))
            .map_err(|e| Error::ModelLoading(format!("Failed to get weights: {}", e)))?;

        let vb = if weights_path.extension().map(|e| e == "safetensors").unwrap_or(false) {
            unsafe {
                VarBuilder::from_mmaped_safetensors(&[weights_path], candle_core::DType::F32, &device)
                    .map_err(|e| Error::ModelLoading(format!("Failed to load safetensors: {}", e)))?
            }
        } else {
            VarBuilder::from_pth(weights_path, candle_core::DType::F32, &device)
                .map_err(|e| Error::ModelLoading(format!("Failed to load weights: {}", e)))?
        };

        let model = BertModel::load(vb, &config)
            .map_err(|e| Error::ModelLoading(format!("Failed to load model: {}", e)))?;

        info!("Embedding model loaded successfully");

        Ok(Self {
            model: Arc::new(RwLock::new(model)),
            tokenizer: Arc::new(tokenizer),
            device,
            model_type,
            max_length: 512,
        })
    }

    /// Load from a local cache directory
    pub fn from_cache(cache_dir: impl AsRef<Path>, model_type: EmbeddingModel, use_gpu: bool) -> Result<Self> {
        let cache_dir = cache_dir.as_ref();
        let model_dir = cache_dir.join(model_type.model_id().replace("/", "--"));

        if model_dir.exists() {
            info!("Loading model from cache: {:?}", model_dir);
            // If cached, set HF_HOME and load
            std::env::set_var("HF_HOME", cache_dir);
        }

        Self::new(model_type, use_gpu)
    }

    /// Select the best available GPU device based on compiled features
    fn select_gpu_device() -> Device {
        // Try Metal first (macOS)
        #[cfg(feature = "metal")]
        {
            match Device::new_metal(0) {
                Ok(d) => {
                    info!("Using Metal GPU for embeddings");
                    return d;
                }
                Err(e) => {
                    warn!("Metal not available: {}", e);
                }
            }
        }

        // Try CUDA (Linux with NVIDIA GPU)
        #[cfg(feature = "cuda")]
        {
            match Device::new_cuda(0) {
                Ok(d) => {
                    info!("Using CUDA GPU for embeddings");
                    return d;
                }
                Err(e) => {
                    warn!("CUDA not available: {}", e);
                }
            }
        }

        // Fallback to CPU
        #[cfg(not(any(feature = "metal", feature = "cuda")))]
        {
            info!("No GPU features enabled, using CPU for embeddings");
        }

        #[cfg(any(feature = "metal", feature = "cuda"))]
        {
            warn!("GPU requested but not available, falling back to CPU");
        }

        Device::Cpu
    }

    /// Generate embedding for a single text
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.embed_batch(&[text.to_string()])?;
        Ok(embeddings.into_iter().next().unwrap_or_default())
    }

    /// Generate embeddings for a batch of texts
    pub fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        debug!("Generating embeddings for {} texts", texts.len());

        // Tokenize all texts (CPU work)
        let mut all_input_ids = Vec::new();
        let mut all_attention_masks = Vec::new();
        let mut max_len = 0;

        for text in texts {
            let encoding = self
                .tokenizer
                .encode(text.as_str(), true)
                .map_err(|e| Error::Embedding(format!("Tokenization failed: {}", e)))?;

            let mut ids: Vec<u32> = encoding.get_ids().to_vec();
            let mut mask: Vec<u32> = encoding.get_attention_mask().iter().map(|&x| x as u32).collect();

            // Truncate if necessary
            if ids.len() > self.max_length {
                ids.truncate(self.max_length);
                mask.truncate(self.max_length);
            }

            max_len = max_len.max(ids.len());
            all_input_ids.push(ids);
            all_attention_masks.push(mask);
        }

        // Pad all sequences to max_len
        for (ids, mask) in all_input_ids.iter_mut().zip(all_attention_masks.iter_mut()) {
            while ids.len() < max_len {
                ids.push(0);
                mask.push(0);
            }
        }

        let batch_size = texts.len();
        let input_ids_flat: Vec<u32> = all_input_ids.into_iter().flatten().collect();
        let attention_mask_flat: Vec<u32> = all_attention_masks.into_iter().flatten().collect();

        // GPU work in isolated scope - all tensors dropped before sync
        let embeddings_vec: Vec<f32> = {
            let input_ids = Tensor::from_vec(input_ids_flat, (batch_size, max_len), &self.device)
                .map_err(|e| Error::Embedding(format!("Failed to create input tensor: {}", e)))?;

            let attention_mask = Tensor::from_vec(attention_mask_flat, (batch_size, max_len), &self.device)
                .map_err(|e| Error::Embedding(format!("Failed to create attention tensor: {}", e)))?;

            let token_type_ids = Tensor::zeros((batch_size, max_len), candle_core::DType::U32, &self.device)
                .map_err(|e| Error::Embedding(format!("Failed to create token type tensor: {}", e)))?;

            // Run model
            let model = self.model.read();
            let output = model
                .forward(&input_ids, &token_type_ids, Some(&attention_mask))
                .map_err(|e| Error::Embedding(format!("Model forward pass failed: {}", e)))?;

            // Mean pooling over sequence dimension
            let embeddings = self.mean_pooling(&output, &attention_mask)?;

            // Normalize embeddings
            let embeddings = self.normalize(&embeddings)?;

            // Convert to CPU Vec<f32> - last GPU operation
            embeddings
                .to_vec2::<f32>()
                .map_err(|e| Error::Embedding(format!("Failed to convert embeddings: {}", e)))?
                .into_iter()
                .flatten()
                .collect()
        }; // All GPU tensors dropped here

        // Sync GPU and release buffers AFTER tensors are dropped
        self.sync();

        // CPU-only work from here
        let dim = self.model_type.dimension();
        let result: Vec<Vec<f32>> = embeddings_vec
            .chunks(dim)
            .map(|chunk| {
                let mut v = chunk.to_vec();
                // Pad to EMBEDDING_DIMENSION if needed
                while v.len() < EMBEDDING_DIMENSION {
                    v.push(0.0);
                }
                v.truncate(EMBEDDING_DIMENSION);
                v
            })
            .collect();

        debug!("Generated {} embeddings", result.len());
        Ok(result)
    }

    /// Mean pooling over sequence tokens
    fn mean_pooling(&self, hidden_states: &Tensor, attention_mask: &Tensor) -> Result<Tensor> {
        // Expand attention mask to match hidden states shape
        let mask = attention_mask
            .unsqueeze(2)
            .map_err(|e| Error::Embedding(format!("Failed to unsqueeze mask: {}", e)))?
            .to_dtype(candle_core::DType::F32)
            .map_err(|e| Error::Embedding(format!("Failed to convert mask dtype: {}", e)))?;

        // Multiply hidden states by mask
        let masked = hidden_states
            .broadcast_mul(&mask)
            .map_err(|e| Error::Embedding(format!("Failed to apply mask: {}", e)))?;

        // Sum over sequence dimension
        let sum = masked
            .sum(1)
            .map_err(|e| Error::Embedding(format!("Failed to sum: {}", e)))?;

        // Get count of non-masked tokens
        let count = mask
            .sum(1)
            .map_err(|e| Error::Embedding(format!("Failed to count: {}", e)))?
            .clamp(1.0, f64::MAX)
            .map_err(|e| Error::Embedding(format!("Failed to clamp: {}", e)))?;

        // Divide to get mean
        let mean = sum
            .broadcast_div(&count)
            .map_err(|e| Error::Embedding(format!("Failed to divide: {}", e)))?;

        Ok(mean)
    }

    /// L2 normalize embeddings
    fn normalize(&self, embeddings: &Tensor) -> Result<Tensor> {
        let norm = embeddings
            .sqr()
            .map_err(|e| Error::Embedding(format!("Failed to square: {}", e)))?
            .sum_keepdim(1)
            .map_err(|e| Error::Embedding(format!("Failed to sum: {}", e)))?
            .sqrt()
            .map_err(|e| Error::Embedding(format!("Failed to sqrt: {}", e)))?
            .clamp(1e-12, f64::MAX)
            .map_err(|e| Error::Embedding(format!("Failed to clamp: {}", e)))?;

        embeddings
            .broadcast_div(&norm)
            .map_err(|e| Error::Embedding(format!("Failed to normalize: {}", e)))
    }

    /// Get the embedding dimension
    pub fn dimension(&self) -> usize {
        self.model_type.dimension()
    }

    /// Get the device being used
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Synchronize the GPU and release unused buffers
    ///
    /// This is important for Metal GPU to prevent memory accumulation.
    /// Should be called after processing batches to force buffer cleanup.
    pub fn sync(&self) {
        #[cfg(feature = "metal")]
        {
            if let Device::Metal(metal_device) = &self.device {
                if let Err(e) = metal_device.wait_until_completed() {
                    warn!("Failed to sync Metal device: {}", e);
                }
            }
        }
    }
}

// ============================================================================
// Remote Embedding Client
// ============================================================================

/// Request body for the custom remote /embed endpoint
#[derive(Debug, Serialize)]
struct RemoteEmbedRequest {
    texts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
}

/// Response body from the custom remote /embed endpoint
#[derive(Debug, Deserialize)]
struct RemoteEmbedResponse {
    embeddings: Vec<Vec<f32>>,
    model: String,
    dimension: usize,
    count: usize,
}

/// Request body for OpenRouter /embeddings endpoint
#[derive(Debug, Serialize)]
struct OpenRouterEmbedRequest {
    model: String,
    input: Vec<String>,
    encoding_format: String,
}

/// Response body from OpenRouter /embeddings endpoint
#[derive(Debug, Deserialize)]
struct OpenRouterEmbedResponse {
    data: Vec<OpenRouterEmbedItem>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterEmbedItem {
    embedding: Vec<f32>,
    index: usize,
}

#[derive(Debug)]
enum RemoteEmbeddingKind {
    DawnCompatible {
        model: String,
    },
    OpenRouter {
        api_key: String,
        model: String,
    },
}

/// Client for remote embedding service (custom /embed or OpenRouter)
pub struct RemoteEmbeddingClient {
    client: reqwest::blocking::Client,
    url: String,
    kind: RemoteEmbeddingKind,
}

impl RemoteEmbeddingClient {
    /// Create a new remote embedding client for custom /embed APIs.
    pub fn new(url: String, timeout_ms: u64) -> Result<Self> {
        let timeout = Duration::from_millis(timeout_ms);
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| Error::Embedding(format!("Failed to create HTTP client: {}", e)))?;

        info!("Created remote embedding client for {}", url);
        Ok(Self {
            client,
            url,
            kind: RemoteEmbeddingKind::DawnCompatible {
                model: "bge-base-en-v1.5".to_string(),
            },
        })
    }

    /// Create a new remote embedding client for OpenRouter.
    pub fn new_openrouter(url: String, api_key: String, model: String, timeout_ms: u64) -> Result<Self> {
        let timeout = Duration::from_millis(timeout_ms);
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| Error::Embedding(format!("Failed to create HTTP client: {}", e)))?;

        info!("Created OpenRouter embedding client for {}", url);
        Ok(Self {
            client,
            url,
            kind: RemoteEmbeddingKind::OpenRouter { api_key, model },
        })
    }

    /// Generate embeddings for a batch of texts using the remote service
    pub fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        debug!(
            "Requesting embeddings for {} texts from {}",
            texts.len(),
            self.url
        );

        match &self.kind {
            RemoteEmbeddingKind::DawnCompatible { model } => {
                let request = RemoteEmbedRequest {
                    texts: texts.to_vec(),
                    model: Some(model.clone()),
                };

                let response = self
                    .client
                    .post(format!("{}/embed", self.url.trim_end_matches('/')))
                    .json(&request)
                    .send()
                    .map_err(|e| Error::Embedding(format!("Remote embedding request failed: {}", e)))?;

                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().unwrap_or_default();
                    return Err(Error::Embedding(format!(
                        "Remote embedding service returned {}: {}",
                        status, body
                    )));
                }

                let result: RemoteEmbedResponse = response
                    .json()
                    .map_err(|e| Error::Embedding(format!("Failed to parse embedding response: {}", e)))?;

                debug!(
                    "Received {} embeddings (dimension: {}, model: {}) from remote service",
                    result.count, result.dimension, result.model
                );

                Ok(Self::fit_embeddings(result.embeddings))
            }
            RemoteEmbeddingKind::OpenRouter { api_key, model } => {
                let request = OpenRouterEmbedRequest {
                    model: model.clone(),
                    input: texts.to_vec(),
                    encoding_format: "float".to_string(),
                };

                let response = self
                    .client
                    .post(format!("{}/embeddings", self.url.trim_end_matches('/')))
                    .bearer_auth(api_key)
                    .json(&request)
                    .send()
                    .map_err(|e| Error::Embedding(format!("OpenRouter embedding request failed: {}", e)))?;

                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().unwrap_or_default();
                    return Err(Error::Embedding(format!(
                        "OpenRouter returned {}: {}",
                        status, body
                    )));
                }

                let mut result: OpenRouterEmbedResponse = response
                    .json()
                    .map_err(|e| Error::Embedding(format!("Failed to parse OpenRouter response: {}", e)))?;

                // Keep the same order as input.
                result.data.sort_by_key(|item| item.index);
                let embeddings = result
                    .data
                    .into_iter()
                    .map(|item| item.embedding)
                    .collect::<Vec<_>>();

                debug!(
                    "Received {} embeddings from OpenRouter model {}",
                    embeddings.len(),
                    model
                );

                Ok(Self::fit_embeddings(embeddings))
            }
        }
    }

    /// Generate embedding for a single text
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.embed_batch(&[text.to_string()])?;
        Ok(embeddings.into_iter().next().unwrap_or_default())
    }

    /// Check if the remote service is available
    pub fn is_available(&self) -> bool {
        match &self.kind {
            RemoteEmbeddingKind::DawnCompatible { .. } => {
                match self.client.get(format!("{}/health", self.url.trim_end_matches('/'))).send() {
                    Ok(resp) => resp.status().is_success(),
                    Err(_) => false,
                }
            }
            // OpenRouter has no lightweight unauthenticated health check.
            RemoteEmbeddingKind::OpenRouter { .. } => true,
        }
    }

    fn fit_embeddings(embeddings: Vec<Vec<f32>>) -> Vec<Vec<f32>> {
        embeddings
            .into_iter()
            .map(|mut e| {
                // Pad or truncate to EMBEDDING_DIMENSION
                while e.len() < EMBEDDING_DIMENSION {
                    e.push(0.0);
                }
                e.truncate(EMBEDDING_DIMENSION);
                e
            })
            .collect()
    }
}

// ============================================================================
// Hybrid Embedding Provider
// ============================================================================

/// Hybrid embedding provider that uses remote service with local fallback
pub struct HybridEmbeddingProvider {
    remote: Option<RemoteEmbeddingClient>,
    local: Option<Arc<EmbeddingEngine>>,
    fallback: EmbeddingFallback,
}

impl HybridEmbeddingProvider {
    /// Create a new hybrid embedding provider
    ///
    /// If `local` is None and fallback is `Local`, it will be changed to `Bm25`.
    pub fn new(
        local: Option<Arc<EmbeddingEngine>>,
        remote_url: Option<String>,
        timeout_ms: u64,
        fallback: EmbeddingFallback,
    ) -> Result<Self> {
        let remote = if let Some(url) = remote_url {
            match RemoteEmbeddingClient::new(url.clone(), timeout_ms) {
                Ok(client) => {
                    info!("Remote embedding service configured at {}", url);
                    Some(client)
                }
                Err(e) => {
                    warn!("Failed to create remote embedding client: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // If no local engine and fallback is Local, change to Bm25
        let actual_fallback = if local.is_none() && fallback == EmbeddingFallback::Local {
            warn!("No local embedding engine provided, changing fallback from Local to Bm25");
            EmbeddingFallback::Bm25
        } else {
            fallback
        };

        Ok(Self {
            remote,
            local,
            fallback: actual_fallback,
        })
    }

    /// Create from search config. Supports local, remote, and OpenRouter providers.
    pub fn from_search_config(local: Option<Arc<EmbeddingEngine>>, search: &SearchConfig) -> Result<Self> {
        let remote = match search.effective_embedding_provider() {
            EmbeddingProvider::Local => None,
            EmbeddingProvider::Remote => {
                if let Some(url) = search.embedding_url.clone() {
                    match RemoteEmbeddingClient::new(url.clone(), search.embedding_timeout_ms) {
                        Ok(client) => {
                            info!("Remote embedding service configured at {}", url);
                            Some(client)
                        }
                        Err(e) => {
                            warn!("Failed to create remote embedding client: {}", e);
                            None
                        }
                    }
                } else {
                    warn!("embedding_provider is 'remote' but search.embedding_url is unset; using fallback only");
                    None
                }
            }
            EmbeddingProvider::OpenRouter => {
                let env_name = search.openrouter_api_key_env.trim();
                if env_name.is_empty() {
                    warn!("search.openrouter_api_key_env is empty; using fallback only");
                    None
                } else {
                    match std::env::var(env_name) {
                        Ok(api_key) if !api_key.trim().is_empty() => {
                            match RemoteEmbeddingClient::new_openrouter(
                                search.openrouter_base_url.clone(),
                                api_key,
                                search.openrouter_model.clone(),
                                search.embedding_timeout_ms,
                            ) {
                                Ok(client) => {
                                    info!("OpenRouter embedding configured with model {}", search.openrouter_model);
                                    Some(client)
                                }
                                Err(e) => {
                                    warn!("Failed to create OpenRouter embedding client: {}", e);
                                    None
                                }
                            }
                        }
                        _ => {
                            warn!(
                                "OpenRouter provider selected but API key env '{}' is missing/empty; using fallback only",
                                env_name
                            );
                            None
                        }
                    }
                }
            }
        };

        // If no local engine and fallback is Local, change to Bm25
        let actual_fallback = if local.is_none() && search.embedding_fallback == EmbeddingFallback::Local {
            warn!("No local embedding engine provided, changing fallback from Local to Bm25");
            EmbeddingFallback::Bm25
        } else {
            search.embedding_fallback
        };

        Ok(Self {
            remote,
            local,
            fallback: actual_fallback,
        })
    }

    /// Generate embeddings for a batch of texts
    ///
    /// If remote service is configured and available, uses remote.
    /// Otherwise falls back based on configuration.
    pub fn embed_batch(&self, texts: &[String]) -> Result<Option<Vec<Vec<f32>>>> {
        if texts.is_empty() {
            return Ok(Some(vec![]));
        }

        // Try remote first if configured
        if let Some(ref remote) = self.remote {
            match remote.embed_batch(texts) {
                Ok(embeddings) => {
                    debug!("Used remote embedding service for {} texts", texts.len());
                    return Ok(Some(embeddings));
                }
                Err(e) => {
                    warn!("Remote embedding failed: {}, using fallback", e);
                }
            }
        }

        // Handle fallback
        match self.fallback {
            EmbeddingFallback::Local => {
                if let Some(ref local) = self.local {
                    debug!("Falling back to local embedding for {} texts", texts.len());
                    let embeddings = local.embed_batch(texts)?;
                    Ok(Some(embeddings))
                } else {
                    debug!("No local engine, falling back to BM25-only");
                    Ok(None)
                }
            }
            EmbeddingFallback::Bm25 => {
                debug!("Falling back to BM25-only (no embeddings) for {} texts", texts.len());
                Ok(None) // Signal to skip vector search
            }
            EmbeddingFallback::Error => {
                Err(Error::Embedding(
                    "Remote embedding service unavailable and fallback is 'error'".to_string(),
                ))
            }
        }
    }

    /// Generate embedding for a single text
    pub fn embed(&self, text: &str) -> Result<Option<Vec<f32>>> {
        let result = self.embed_batch(&[text.to_string()])?;
        Ok(result.map(|mut v| v.pop().unwrap_or_default()))
    }

    /// Check if remote service is available
    pub fn is_remote_available(&self) -> bool {
        self.remote.as_ref().map(|r| r.is_available()).unwrap_or(false)
    }

    /// Get the embedding dimension (768 for bge-base-en-v1.5)
    pub fn dimension(&self) -> usize {
        // If we have a local engine, use its dimension
        // Otherwise return the standard dimension for bge-base-en-v1.5
        self.local.as_ref().map(|l| l.dimension()).unwrap_or(768)
    }

    /// Get a reference to the local embedding engine if available
    pub fn local_engine(&self) -> Option<&Arc<EmbeddingEngine>> {
        self.local.as_ref()
    }
}
