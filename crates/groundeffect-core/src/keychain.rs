//! File-based OAuth token storage
//!
//! Stores tokens in ~/.config/groundeffect/tokens/<account>.json
//! with 600 permissions (owner read/write only).

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::LazyLock;
use tracing::{debug, error, info};

use crate::error::{Error, Result};

/// In-memory token cache to avoid repeated file reads
static TOKEN_CACHE: LazyLock<RwLock<HashMap<String, OAuthTokens>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// OAuth tokens stored on disk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    /// Access token for API calls
    pub access_token: String,

    /// Refresh token for obtaining new access tokens
    pub refresh_token: String,

    /// Token expiry timestamp (Unix seconds)
    pub expires_at: i64,

    /// Token scopes
    pub scopes: Vec<String>,
}

impl OAuthTokens {
    /// Check if the access token is expired or will expire soon
    pub fn is_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp();
        // Consider expired if less than 5 minutes remaining
        self.expires_at < now + 300
    }

    /// Check if the access token is definitely expired (no grace period)
    pub fn is_definitely_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp();
        self.expires_at < now
    }
}

/// Get the tokens directory path (XDG: ~/.config/groundeffect/tokens)
fn tokens_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("groundeffect")
        .join("tokens")
}

/// Get the token file path for an account
fn token_file_path(account_id: &str) -> PathBuf {
    // Sanitize account_id for use as filename (replace @ and . with _)
    let safe_name = account_id.replace('@', "_at_").replace('.', "_");
    tokens_dir().join(format!("{}.json", safe_name))
}

/// Ensure the tokens directory exists with proper permissions
fn ensure_tokens_dir() -> Result<()> {
    let dir = tokens_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
        // Set directory permissions to 700 (owner rwx only)
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Token manager for storing OAuth tokens in files
pub struct KeychainManager;

impl KeychainManager {
    /// Store OAuth tokens for an account
    /// Only writes to disk if refresh_token changed (to minimize disk I/O)
    pub fn store_tokens(account_id: &str, tokens: &OAuthTokens) -> Result<()> {
        // Check if we need to write to disk at all
        // Only write if refresh_token changed (access_token changes are cached in memory)
        let needs_disk_write = {
            let cache = TOKEN_CACHE.read();
            match cache.get(account_id) {
                Some(cached) => cached.refresh_token != tokens.refresh_token,
                None => true, // Not in cache, need to write
            }
        };

        // Always update the in-memory cache
        TOKEN_CACHE
            .write()
            .insert(account_id.to_string(), tokens.clone());

        if needs_disk_write {
            ensure_tokens_dir()?;
            let path = token_file_path(account_id);
            let data = serde_json::to_string_pretty(tokens)?;

            fs::write(&path, &data).map_err(|e| {
                error!("Failed to write token file: {}", e);
                Error::Token(format!("Failed to store tokens: {}", e))
            })?;

            // Set file permissions to 600 (owner rw only)
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|e| {
                error!("Failed to set token file permissions: {}", e);
                Error::Token(format!("Failed to set permissions: {}", e))
            })?;

            debug!("Stored OAuth tokens for {} (disk updated)", account_id);
        } else {
            debug!(
                "Stored OAuth tokens for {} (cache only, refresh_token unchanged)",
                account_id
            );
        }

        Ok(())
    }

    /// Retrieve OAuth tokens for an account
    /// Uses in-memory cache to avoid repeated file reads
    pub fn get_tokens(account_id: &str) -> Result<Option<OAuthTokens>> {
        // Check the in-memory cache first
        if let Some(tokens) = TOKEN_CACHE.read().get(account_id) {
            debug!("Retrieved OAuth tokens for {} from cache", account_id);
            return Ok(Some(tokens.clone()));
        }

        // Not in cache, read from disk
        let path = token_file_path(account_id);

        if !path.exists() {
            debug!("No tokens found for {}", account_id);
            return Ok(None);
        }

        match fs::read_to_string(&path) {
            Ok(data) => {
                let tokens: OAuthTokens = serde_json::from_str(&data)
                    .map_err(|e| Error::Token(format!("Invalid token data: {}", e)))?;

                // Cache the tokens for future calls
                TOKEN_CACHE
                    .write()
                    .insert(account_id.to_string(), tokens.clone());

                debug!("Retrieved OAuth tokens for {} from disk", account_id);
                Ok(Some(tokens))
            }
            Err(e) => {
                error!("Failed to read token file: {}", e);
                Err(Error::Token(format!("Failed to read tokens: {}", e)))
            }
        }
    }

    /// Delete OAuth tokens for an account
    pub fn delete_tokens(account_id: &str) -> Result<()> {
        // Clear from cache first
        TOKEN_CACHE.write().remove(account_id);

        let path = token_file_path(account_id);
        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                error!("Failed to delete token file: {}", e);
                Error::Token(format!("Failed to delete tokens: {}", e))
            })?;
            info!("Deleted OAuth tokens for {}", account_id);
        } else {
            debug!("No tokens to delete for {}", account_id);
        }

        Ok(())
    }

    /// Update just the access token (after refresh)
    pub fn update_access_token(
        account_id: &str,
        access_token: &str,
        expires_at: i64,
    ) -> Result<()> {
        let mut tokens = Self::get_tokens(account_id)?
            .ok_or_else(|| Error::Token("No existing tokens to update".to_string()))?;

        tokens.access_token = access_token.to_string();
        tokens.expires_at = expires_at;

        Self::store_tokens(account_id, &tokens)
    }

    /// Check if tokens exist for an account
    pub fn has_tokens(account_id: &str) -> bool {
        Self::get_tokens(account_id)
            .map(|t| t.is_some())
            .unwrap_or(false)
    }

    /// List all accounts with stored tokens
    /// Note: This is a simplified implementation that checks known accounts
    pub fn list_accounts_with_tokens(known_accounts: &[String]) -> Vec<String> {
        known_accounts
            .iter()
            .filter(|id| Self::has_tokens(id))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_tokens_expiry() {
        let tokens = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: "test".to_string(),
            expires_at: chrono::Utc::now().timestamp() + 3600, // 1 hour from now
            scopes: vec![],
        };
        assert!(!tokens.is_expired());
        assert!(!tokens.is_definitely_expired());

        let expired_tokens = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: "test".to_string(),
            expires_at: chrono::Utc::now().timestamp() - 100, // 100 seconds ago
            scopes: vec![],
        };
        assert!(expired_tokens.is_expired());
        assert!(expired_tokens.is_definitely_expired());

        let soon_expired_tokens = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: "test".to_string(),
            expires_at: chrono::Utc::now().timestamp() + 60, // 1 minute from now
            scopes: vec![],
        };
        assert!(soon_expired_tokens.is_expired()); // Within 5-minute grace period
        assert!(!soon_expired_tokens.is_definitely_expired());
    }

    #[test]
    fn test_token_file_path() {
        let path = token_file_path("test@example.com");
        assert!(path.to_string_lossy().contains("test_at_example_com.json"));
    }
}
