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

/// Vector dimension for embeddings (384 for all-MiniLM-L6-v2)
pub const EMBEDDING_DIMENSION: usize = 384;

/// Application name for Keychain and config paths
pub const APP_NAME: &str = "groundeffect";

/// Keychain service name prefix
pub const KEYCHAIN_SERVICE: &str = "com.groundeffect.oauth";
