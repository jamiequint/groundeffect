//! Error types for GroundEffect

use thiserror::Error;

/// Result type alias using GroundEffect's Error type
pub type Result<T> = std::result::Result<T, Error>;

/// Main error type for GroundEffect
#[derive(Error, Debug)]
pub enum Error {
    // Database errors
    #[error("Database error: {0}")]
    Database(#[from] lancedb::Error),

    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("Table not found: {0}")]
    TableNotFound(String),

    // Authentication errors
    #[error("OAuth error: {0}")]
    OAuth(String),

    #[error("Token expired for account {account}")]
    TokenExpired { account: String },

    #[error("Token refresh failed for account {account}: {reason}")]
    TokenRefreshFailed { account: String, reason: String },

    #[error("Token storage error: {0}")]
    Token(String),

    // Sync errors
    #[error("IMAP error: {0}")]
    Imap(String),

    #[error("CalDAV error: {0}")]
    CalDav(String),

    #[error("Sync error: {0}")]
    Sync(String),

    #[error("Connection failed to {host}: {reason}")]
    ConnectionFailed { host: String, reason: String },

    // Embedding errors
    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("Model loading error: {0}")]
    ModelLoading(String),

    // MCP errors
    #[error("MCP protocol error: {0}")]
    McpProtocol(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Resource not found: {0}")]
    ResourceNotFound(String),

    // Account errors
    #[error("Account not found: {0}")]
    AccountNotFound(String),

    #[error("Account already exists: {0}")]
    AccountAlreadyExists(String),

    // Data errors
    #[error("Email not found: {0}")]
    EmailNotFound(String),

    #[error("Event not found: {0}")]
    EventNotFound(String),

    #[error("Thread not found: {0}")]
    ThreadNotFound(String),

    #[error("Invalid email format: {0}")]
    InvalidEmailFormat(String),

    // Configuration errors
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Invalid configuration: {field}: {reason}")]
    InvalidConfig { field: String, reason: String },

    // I/O errors
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    // Serialization errors
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    // Rate limiting
    #[error("Rate limited, retry after {retry_after_secs} seconds")]
    RateLimited { retry_after_secs: u64 },

    // Generic errors
    #[error("{0}")]
    Other(String),

    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

impl Error {
    /// Returns true if this error indicates the user needs to re-authenticate
    pub fn requires_reauth(&self) -> bool {
        matches!(
            self,
            Error::TokenExpired { .. } | Error::TokenRefreshFailed { .. }
        )
    }

    /// Returns an error code suitable for MCP error responses
    pub fn mcp_code(&self) -> &'static str {
        match self {
            Error::TokenExpired { .. } | Error::TokenRefreshFailed { .. } => "AUTH_EXPIRED",
            Error::AccountNotFound(_) => "ACCOUNT_NOT_FOUND",
            Error::EmailNotFound(_) => "EMAIL_NOT_FOUND",
            Error::EventNotFound(_) => "EVENT_NOT_FOUND",
            Error::InvalidRequest(_) => "INVALID_REQUEST",
            Error::ToolNotFound(_) => "TOOL_NOT_FOUND",
            Error::ResourceNotFound(_) => "RESOURCE_NOT_FOUND",
            Error::RateLimited { .. } => "RATE_LIMITED",
            Error::Database(_) | Error::Arrow(_) => "DATABASE_ERROR",
            Error::Imap(_) | Error::CalDav(_) | Error::Sync(_) | Error::ConnectionFailed { .. } => "SYNC_ERROR",
            _ => "INTERNAL_ERROR",
        }
    }

    /// Returns a user-friendly action message for recoverable errors
    pub fn action_hint(&self) -> Option<&'static str> {
        match self {
            Error::TokenExpired { .. } | Error::TokenRefreshFailed { .. } => {
                Some("Please re-authenticate in GroundEffect preferences")
            }
            Error::RateLimited { .. } => Some("Please wait and try again"),
            Error::ConnectionFailed { .. } => Some("Check your network connection"),
            _ => None,
        }
    }
}
