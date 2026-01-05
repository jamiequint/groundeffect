//! Embedding pipeline using Candle with Metal acceleration
//!
//! Uses nomic-embed-text-v1.5 (or all-MiniLM-L6-v2) for text embeddings.

use std::path::Path;
use std::sync::Arc;

use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use hf_hub::{api::sync::Api, Repo, RepoType};
use parking_lot::RwLock;
use tokenizers::Tokenizer;
use tracing::{debug, info, warn};

use crate::error::{Error, Result};
use crate::EMBEDDING_DIMENSION;

/// Supported embedding models
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingModel {
    /// nomic-embed-text-v1.5 (768 dimensions)
    NomicEmbedText,
    /// all-MiniLM-L6-v2 (384 dimensions, faster)
    MiniLML6,
}

impl EmbeddingModel {
    /// Get the HuggingFace model ID
    pub fn model_id(&self) -> &'static str {
        match self {
            EmbeddingModel::NomicEmbedText => "nomic-ai/nomic-embed-text-v1.5",
            EmbeddingModel::MiniLML6 => "sentence-transformers/all-MiniLM-L6-v2",
        }
    }

    /// Get the embedding dimension
    pub fn dimension(&self) -> usize {
        match self {
            EmbeddingModel::NomicEmbedText => 768,
            EmbeddingModel::MiniLML6 => 384,
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "nomic-embed-text-v1.5" | "nomic" => Some(EmbeddingModel::NomicEmbedText),
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
    pub fn new(model_type: EmbeddingModel, use_metal: bool) -> Result<Self> {
        info!(
            "Initializing embedding engine with model {:?}, metal={}",
            model_type, use_metal
        );

        // Select device
        let device = if use_metal {
            match Device::new_metal(0) {
                Ok(d) => {
                    info!("Using Metal device for embeddings");
                    d
                }
                Err(e) => {
                    warn!("Metal not available, falling back to CPU: {}", e);
                    Device::Cpu
                }
            }
        } else {
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
    pub fn from_cache(cache_dir: impl AsRef<Path>, model_type: EmbeddingModel, use_metal: bool) -> Result<Self> {
        let cache_dir = cache_dir.as_ref();
        let model_dir = cache_dir.join(model_type.model_id().replace("/", "--"));

        if model_dir.exists() {
            info!("Loading model from cache: {:?}", model_dir);
            // If cached, set HF_HOME and load
            std::env::set_var("HF_HOME", cache_dir);
        }

        Self::new(model_type, use_metal)
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

        // Tokenize all texts
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

        // Convert to tensors
        let batch_size = texts.len();
        let input_ids_flat: Vec<u32> = all_input_ids.into_iter().flatten().collect();
        let attention_mask_flat: Vec<u32> = all_attention_masks.into_iter().flatten().collect();

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

        // Convert to Vec<Vec<f32>>
        let embeddings_vec: Vec<f32> = embeddings
            .to_vec2::<f32>()
            .map_err(|e| Error::Embedding(format!("Failed to convert embeddings: {}", e)))?
            .into_iter()
            .flatten()
            .collect();

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
}
