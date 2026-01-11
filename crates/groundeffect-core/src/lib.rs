//! GroundEffect Core Library
//!
//! High-performance email and calendar sync with LanceDB storage
//! and MCP server for Claude Code integration.

pub mod config;
pub mod db;
pub mod embedding;
pub mod error;
pub mod keychain;
pub mod mcp;
pub mod models;
pub mod oauth;
pub mod search;
pub mod sync;

pub use config::Config;
pub use error::{Error, Result};
pub use models::*;

/// Vector dimension for embeddings (768 for bge-base-en-v1.5)
pub const EMBEDDING_DIMENSION: usize = 768;

/// Application name for config paths
pub const APP_NAME: &str = "groundeffect";
