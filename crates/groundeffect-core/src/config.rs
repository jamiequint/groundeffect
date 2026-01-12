//! Configuration management for GroundEffect

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::info;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// General settings
    #[serde(default)]
    pub general: GeneralConfig,

    /// Sync settings
    #[serde(default)]
    pub sync: SyncConfig,

    /// Search settings
    #[serde(default)]
    pub search: SearchConfig,

    /// UI settings
    #[serde(default)]
    pub ui: UiConfig,

    /// Account aliases
    #[serde(default)]
    pub accounts: AccountsConfig,

    /// Token storage provider configuration
    #[serde(default)]
    pub tokens: TokenProviderConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            sync: SyncConfig::default(),
            search: SearchConfig::default(),
            ui: UiConfig::default(),
            accounts: AccountsConfig::default(),
            tokens: TokenProviderConfig::default(),
        }
    }
}

/// Token storage provider configuration
///
/// Controls where OAuth tokens are stored. Default is file-based storage.
///
/// # Examples
///
/// File-based (default):
/// ```toml
/// [tokens]
/// provider = "file"
/// ```
///
/// PostgreSQL (requires "postgres" feature):
/// ```toml
/// [tokens]
/// provider = "postgres"
/// database_url_env = "DATABASE_URL"
/// encryption_key_env = "GE_TOKEN_ENCRYPTION_KEY"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum TokenProviderConfig {
    /// File-based token storage (default)
    /// Stores tokens in ~/.config/groundeffect/tokens/<account>.json
    File,

    /// PostgreSQL-based token storage
    /// Requires the "postgres" feature to be enabled
    Postgres {
        /// Direct database URL (optional if database_url_env is set)
        #[serde(skip_serializing_if = "Option::is_none")]
        database_url: Option<String>,

        /// Environment variable containing the database URL
        #[serde(skip_serializing_if = "Option::is_none")]
        database_url_env: Option<String>,

        /// Environment variable containing the encryption key (required)
        encryption_key_env: String,

        /// Custom table name (default: "groundeffect_tokens")
        #[serde(skip_serializing_if = "Option::is_none")]
        table_name: Option<String>,

        /// Static user_id for multi-tenant deployments (optional)
        /// When set, queries filter by user_id column for tenant isolation
        #[serde(skip_serializing_if = "Option::is_none")]
        user_id: Option<String>,

        /// Environment variable containing the user_id (optional)
        /// Alternative to setting user_id directly in config
        #[serde(skip_serializing_if = "Option::is_none")]
        user_id_env: Option<String>,
    },
}

impl Default for TokenProviderConfig {
    fn default() -> Self {
        Self::File
    }
}

/// General application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    /// Log level (debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Log file path
    #[serde(default = "default_log_file")]
    pub log_file: PathBuf,

    /// Data directory path
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    /// User's timezone for date parsing and display (e.g., "America/Los_Angeles", "UTC")
    /// Used when parsing relative dates like "today" or date ranges in search queries
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

fn default_timezone() -> String {
    // Try to detect system timezone, fallback to UTC
    std::env::var("TZ").unwrap_or_else(|_| "UTC".to_string())
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            log_file: default_log_file(),
            data_dir: default_data_dir(),
            timezone: default_timezone(),
        }
    }
}

/// Sync settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Use IMAP IDLE for real-time push
    #[serde(default = "default_true")]
    pub email_idle_enabled: bool,

    /// Fallback poll interval (seconds)
    #[serde(default = "default_poll_interval")]
    pub email_poll_interval_secs: u64,

    /// CalDAV poll interval (seconds)
    #[serde(default = "default_poll_interval")]
    pub calendar_poll_interval_secs: u64,

    /// Max parallel email fetches per account
    #[serde(default = "default_concurrent_fetches")]
    pub max_concurrent_fetches: usize,

    /// Skip attachments larger than this (MB)
    #[serde(default = "default_max_attachment_size")]
    pub attachment_max_size_mb: u64,

    /// Global rate limit (requests per second)
    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_second: u32,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            email_idle_enabled: true,
            email_poll_interval_secs: 300,
            calendar_poll_interval_secs: 300,
            max_concurrent_fetches: 10,
            attachment_max_size_mb: 100,
            rate_limit_per_second: 10,
        }
    }
}

/// Fallback behavior when remote embedding service is unavailable
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingFallback {
    /// Fall back to BM25-only search (no vector search, fast, lower quality)
    Bm25,
    /// Fall back to local CPU embedding (preserves quality, slower)
    Local,
    /// Return an error if remote service is unavailable
    Error,
}

impl Default for EmbeddingFallback {
    fn default() -> Self {
        Self::Bm25
    }
}

/// Search settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Embedding model name
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,

    /// Use GPU acceleration (Metal on macOS, CUDA on Linux)
    #[serde(default = "default_true", alias = "use_metal")]
    pub use_gpu: bool,

    /// BM25 weight in hybrid search (0.0-1.0)
    #[serde(default = "default_search_weight")]
    pub bm25_weight: f32,

    /// Vector weight in hybrid search (0.0-1.0)
    #[serde(default = "default_search_weight")]
    pub vector_weight: f32,

    /// Batch size for local embedding generation
    /// Small values (1-2) for memory-constrained environments, larger (32+) for powerful GPUs
    #[serde(default = "default_embedding_batch_size")]
    pub embedding_batch_size: usize,

    /// Minimum texts to use remote GPU embedding service (if configured)
    /// Below this threshold, use local embedding
    #[serde(default = "default_embedding_gpu_threshold")]
    pub embedding_gpu_threshold: usize,

    /// URL of remote embedding service (e.g., "http://dawn-embeddings.internal:8000")
    /// If set, embeddings will be requested from this service instead of generated locally.
    #[serde(default)]
    pub embedding_url: Option<String>,

    /// What to do when remote embedding service is unavailable
    #[serde(default)]
    pub embedding_fallback: EmbeddingFallback,

    /// Timeout for remote embedding requests in milliseconds
    #[serde(default = "default_embedding_timeout_ms")]
    pub embedding_timeout_ms: u64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            embedding_model: default_embedding_model(),
            use_gpu: true,
            bm25_weight: 0.5,
            vector_weight: 0.5,
            embedding_batch_size: default_embedding_batch_size(),
            embedding_gpu_threshold: default_embedding_gpu_threshold(),
            embedding_url: None,
            embedding_fallback: EmbeddingFallback::default(),
            embedding_timeout_ms: default_embedding_timeout_ms(),
        }
    }
}

/// UI settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    /// Show menu bar icon
    #[serde(default = "default_true")]
    pub show_menu_bar_icon: bool,

    /// Number of recent items to show
    #[serde(default = "default_recent_items")]
    pub show_recent_items: usize,

    /// Launch at login
    #[serde(default)]
    pub launch_at_login: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_menu_bar_icon: true,
            show_recent_items: 5,
            launch_at_login: false,
        }
    }
}

/// Account-related configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountsConfig {
    /// Account aliases (alias -> email address)
    #[serde(default)]
    pub aliases: HashMap<String, String>,

    /// Per-account settings
    #[serde(flatten)]
    pub accounts: HashMap<String, AccountConfig>,
}

/// Per-account configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    /// Whether syncing is enabled for this account
    #[serde(default = "default_true")]
    pub sync_enabled: bool,

    /// Whether to sync emails for this account
    #[serde(default = "default_true")]
    pub sync_email: bool,

    /// Whether to sync calendar for this account
    #[serde(default = "default_true")]
    pub sync_calendar: bool,

    /// Folders to sync (empty = all folders)
    #[serde(default)]
    pub folders: Vec<String>,
}

impl Default for AccountConfig {
    fn default() -> Self {
        Self {
            sync_enabled: true,
            sync_email: true,
            sync_calendar: true,
            folders: vec![],
        }
    }
}

// Default value functions
fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_file() -> PathBuf {
    get_data_dir().join("logs").join("groundeffect.log")
}

fn default_data_dir() -> PathBuf {
    get_data_dir()
}

fn default_true() -> bool {
    true
}

fn default_poll_interval() -> u64 {
    300
}

fn default_concurrent_fetches() -> usize {
    10
}

fn default_max_attachment_size() -> u64 {
    100
}

fn default_rate_limit() -> u32 {
    10
}

fn default_embedding_model() -> String {
    "bge-base-en-v1.5".to_string()
}

fn default_search_weight() -> f32 {
    0.5
}

fn default_embedding_batch_size() -> usize {
    1  // Minimal memory footprint, safe for 1GB containers
}

fn default_embedding_gpu_threshold() -> usize {
    10  // Route most bulk work to GPU service
}

fn default_embedding_timeout_ms() -> u64 {
    30000  // 30 seconds - embedding can be slow on first request (model loading)
}

fn default_recent_items() -> usize {
    5
}

/// Get the data directory (XDG: ~/.local/share/groundeffect)
fn get_data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("groundeffect")
}

/// Get the config directory (XDG: ~/.config/groundeffect)
fn get_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("groundeffect")
}

impl Config {
    /// Load configuration from the default path
    pub fn load() -> Result<Self> {
        let config_path = get_config_dir().join("config.toml");
        Self::load_from(&config_path)
    }

    /// Load configuration from a specific path
    pub fn load_from(path: &PathBuf) -> Result<Self> {
        if path.exists() {
            let contents = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&contents)?;
            info!("Loaded configuration from {:?}", path);
            Ok(config)
        } else {
            info!("No config file found at {:?}, using defaults", path);
            Ok(Config::default())
        }
    }

    /// Save configuration to the default path
    pub fn save(&self) -> Result<()> {
        let config_path = get_config_dir().join("config.toml");
        self.save_to(&config_path)
    }

    /// Save configuration to a specific path
    pub fn save_to(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents =
            toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        std::fs::write(path, contents)?;
        info!("Saved configuration to {:?}", path);
        Ok(())
    }

    /// Get the LanceDB directory
    pub fn lancedb_dir(&self) -> PathBuf {
        self.general.data_dir.join("lancedb")
    }

    /// Get the attachments directory
    pub fn attachments_dir(&self) -> PathBuf {
        self.general.data_dir.join("attachments")
    }

    /// Get the models directory
    pub fn models_dir(&self) -> PathBuf {
        self.general.data_dir.join("models")
    }

    /// Get the sync state directory
    pub fn sync_state_dir(&self) -> PathBuf {
        self.general.data_dir.join("cache").join("sync_state")
    }

    /// Get the daemon PID file path
    pub fn daemon_pid_file(&self) -> PathBuf {
        self.general.data_dir.join("daemon.pid")
    }

    /// Get the sync progress file path (for MCP to read daemon progress)
    pub fn sync_progress_file(&self) -> PathBuf {
        self.general.data_dir.join("sync_progress.json")
    }

    /// Resolve an account identifier (email or alias) to an email address
    pub fn resolve_account(&self, identifier: &str) -> Option<String> {
        // Check if it's an alias first
        if let Some(email) = self.accounts.aliases.get(identifier) {
            return Some(email.clone());
        }

        // Check if it's already an email in the aliases values
        if self.accounts.aliases.values().any(|v| v == identifier) {
            return Some(identifier.to_string());
        }

        // Assume it's an email address
        if identifier.contains('@') {
            return Some(identifier.to_string());
        }

        None
    }

    /// Get the alias for an email address (if configured)
    pub fn get_alias(&self, email: &str) -> Option<&str> {
        self.accounts
            .aliases
            .iter()
            .find(|(_, v)| v.as_str() == email)
            .map(|(k, _)| k.as_str())
    }
}

/// Daemon-specific configuration for launchd/setup
/// Stored separately at ~/.config/groundeffect/daemon.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Enable logging to ~/.local/share/groundeffect/logs/
    #[serde(default)]
    pub logging_enabled: bool,

    /// Email poll interval in seconds (default: 300)
    #[serde(default = "default_poll_interval")]
    pub email_poll_interval_secs: u64,

    /// Calendar poll interval in seconds (default: 300)
    #[serde(default = "default_poll_interval")]
    pub calendar_poll_interval_secs: u64,

    /// Max concurrent email fetches (default: 10)
    #[serde(default = "default_concurrent_fetches")]
    pub max_concurrent_fetches: usize,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            logging_enabled: false,
            email_poll_interval_secs: 300,
            calendar_poll_interval_secs: 300,
            max_concurrent_fetches: 10,
        }
    }
}

impl DaemonConfig {
    /// Get the config file path
    pub fn config_path() -> PathBuf {
        get_config_dir().join("daemon.toml")
    }

    /// Load daemon config from disk (returns defaults if not found)
    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            let config: DaemonConfig = toml::from_str(&contents)?;
            info!("Loaded daemon config from {:?}", path);
            Ok(config)
        } else {
            info!("No daemon config found at {:?}, using defaults", path);
            Ok(DaemonConfig::default())
        }
    }

    /// Save daemon config to disk
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)
            .map_err(|e| Error::Config(e.to_string()))?;
        std::fs::write(&path, contents)?;
        info!("Saved daemon config to {:?}", path);
        Ok(())
    }

    /// Get the launchd plist path
    pub fn launchd_plist_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library")
            .join("LaunchAgents")
            .join("com.groundeffect.daemon.plist")
    }

    /// Check if launchd agent is installed
    pub fn is_launchd_installed() -> bool {
        Self::launchd_plist_path().exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.general.log_level, "info");
        assert_eq!(config.sync.email_poll_interval_secs, 300);
        assert!(config.sync.email_idle_enabled);
    }

    #[test]
    fn test_resolve_account() {
        let mut config = Config::default();
        config
            .accounts
            .aliases
            .insert("work".to_string(), "jamie@company.com".to_string());

        assert_eq!(
            config.resolve_account("work"),
            Some("jamie@company.com".to_string())
        );
        assert_eq!(
            config.resolve_account("jamie@company.com"),
            Some("jamie@company.com".to_string())
        );
        assert_eq!(
            config.resolve_account("other@example.com"),
            Some("other@example.com".to_string())
        );
        assert_eq!(config.resolve_account("nonexistent"), None);
    }
}
