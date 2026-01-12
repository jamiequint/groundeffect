//! GroundEffect MCP Server
//!
//! CLI spawned by Claude Code for read-only access to emails and calendar events.
//! Communicates via stdio JSON-RPC.

use std::sync::Arc;

use anyhow::Result;
use tracing::{error, info};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

use groundeffect_core::config::{Config, EmbeddingFallback};
use groundeffect_core::db::Database;
use groundeffect_core::embedding::{EmbeddingEngine, EmbeddingModel, HybridEmbeddingProvider};
use groundeffect_core::mcp::McpServer;
use groundeffect_core::oauth::OAuthManager;
use groundeffect_core::token_provider::create_token_provider;

#[tokio::main]
async fn main() -> Result<()> {
    // Load config first to get log path
    let config = Arc::new(Config::load().unwrap_or_else(|_| Config::default()));

    // Check if logging is enabled via environment variable
    let enable_logging = std::env::var("GROUNDEFFECT_MCP_LOGGING")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    if enable_logging {
        // Set up file logging to macOS standard location
        let log_dir = config.general.data_dir.join("logs");
        std::fs::create_dir_all(&log_dir)?;

        let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "mcp.log");

        // Create a file layer with timestamps
        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(file_appender)
            .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S%.3f".to_string()))
            .with_ansi(false)
            .with_target(true)
            .with_thread_ids(true);

        // Filter out noisy LanceDB internal logs
        let filter = tracing_subscriber::filter::EnvFilter::new(
            "info,lance=warn,lancedb=warn,lance_core=warn,lance_index=warn,lance_table=warn,lance_file=warn,lance_encoding=warn"
        );

        // Initialize the subscriber with file logging
        tracing_subscriber::registry()
            .with(file_layer.with_filter(filter))
            .init();

        info!("GroundEffect MCP server starting with file logging enabled");
    }

    // Check if database exists
    let db_path = config.lancedb_dir();
    if !db_path.exists() {
        error!("Database not found at {:?}", db_path);
        error!("Please run the GroundEffect daemon first to initialize the database.");
        std::process::exit(1);
    }

    // Open database in read-only mode
    // Note: LanceDB supports concurrent readers, so this is safe
    let db = Arc::new(Database::open(&db_path).await?);

    // Initialize embedding engine with hybrid remote/local support
    // Skip loading local model if using remote with BM25 fallback (saves CPU/memory)
    let local_embedding = if config.search.embedding_url.is_some()
        && config.search.embedding_fallback == EmbeddingFallback::Bm25
    {
        info!("Skipping local embedding model (using remote with BM25 fallback)");
        None
    } else {
        info!("Loading embedding model...");
        let model_type = EmbeddingModel::from_str(&config.search.embedding_model)
            .unwrap_or(EmbeddingModel::BgeBaseEn);
        Some(Arc::new(
            EmbeddingEngine::from_cache(config.models_dir(), model_type, config.search.use_gpu)
                .map_err(|e| {
                    error!("Failed to load embedding model: {}", e);
                    e
                })?,
        ))
    };
    let embedding = Arc::new(HybridEmbeddingProvider::new(
        local_embedding,
        config.search.embedding_url.clone(),
        config.search.embedding_timeout_ms,
        config.search.embedding_fallback,
    )?);

    // Initialize token provider and OAuth manager (for mutations that go directly to IMAP/CalDAV)
    let token_provider = create_token_provider(&config).await?;
    let oauth = Arc::new(OAuthManager::new(token_provider));

    // Create and run MCP server
    let server = McpServer::new(db, config.clone(), embedding, oauth);

    info!("Starting MCP server on stdio");
    server.run().await?;

    Ok(())
}
