//! macOS Keychain integration for secure OAuth token storage

use parking_lot::RwLock;
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{debug, error, info};

use crate::error::{Error, Result};
use crate::KEYCHAIN_SERVICE;

/// In-memory token cache to avoid repeated keychain access prompts
static TOKEN_CACHE: LazyLock<RwLock<HashMap<String, OAuthTokens>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// OAuth tokens stored in keychain
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

/// Keychain manager for storing OAuth tokens
pub struct KeychainManager;

impl KeychainManager {
    /// Get the keychain service name for an account
    fn service_name(account_id: &str) -> String {
        format!("{}.{}", KEYCHAIN_SERVICE, account_id)
    }

    /// Store OAuth tokens for an account
    /// Only writes to keychain if refresh_token changed (to minimize password prompts)
    pub fn store_tokens(account_id: &str, tokens: &OAuthTokens) -> Result<()> {
        // Check if we need to write to keychain at all
        // Only write if refresh_token changed (access_token changes are cached in memory)
        let needs_keychain_write = {
            let cache = TOKEN_CACHE.read();
            match cache.get(account_id) {
                Some(cached) => cached.refresh_token != tokens.refresh_token,
                None => true, // Not in cache, need to write
            }
        };

        // Always update the in-memory cache (this is fast and prompt-free)
        TOKEN_CACHE
            .write()
            .insert(account_id.to_string(), tokens.clone());

        if needs_keychain_write {
            let service = Self::service_name(account_id);
            let data = serde_json::to_string(tokens)?;

            // Try to delete existing first (ignore errors)
            let _ = delete_generic_password(&service, account_id);

            set_generic_password(&service, account_id, data.as_bytes()).map_err(|e| {
                error!("Failed to store tokens in keychain: {}", e);
                Error::Keychain(format!("Failed to store tokens: {}", e))
            })?;

            debug!("Stored OAuth tokens for {} (keychain updated)", account_id);
        } else {
            debug!("Stored OAuth tokens for {} (cache only, refresh_token unchanged)", account_id);
        }

        Ok(())
    }

    /// Retrieve OAuth tokens for an account
    /// Uses in-memory cache to avoid repeated keychain access prompts
    pub fn get_tokens(account_id: &str) -> Result<Option<OAuthTokens>> {
        // Check the in-memory cache first
        if let Some(tokens) = TOKEN_CACHE.read().get(account_id) {
            debug!("Retrieved OAuth tokens for {} from cache", account_id);
            return Ok(Some(tokens.clone()));
        }

        // Not in cache, read from keychain
        let service = Self::service_name(account_id);

        match get_generic_password(&service, account_id) {
            Ok(data) => {
                let json = String::from_utf8(data).map_err(|e| {
                    Error::Keychain(format!("Invalid token data encoding: {}", e))
                })?;
                let tokens: OAuthTokens = serde_json::from_str(&json)?;

                // Cache the tokens for future calls
                TOKEN_CACHE
                    .write()
                    .insert(account_id.to_string(), tokens.clone());

                debug!("Retrieved OAuth tokens for {} from keychain", account_id);
                Ok(Some(tokens))
            }
            Err(e) => {
                // Check if it's a "not found" error
                let error_str = e.to_string();
                if error_str.contains("not found") || error_str.contains("-25300") {
                    debug!("No tokens found for {}", account_id);
                    Ok(None)
                } else {
                    error!("Failed to get tokens from keychain: {}", e);
                    Err(Error::Keychain(format!("Failed to get tokens: {}", e)))
                }
            }
        }
    }

    /// Delete OAuth tokens for an account
    pub fn delete_tokens(account_id: &str) -> Result<()> {
        let service = Self::service_name(account_id);

        // Clear from cache first
        TOKEN_CACHE.write().remove(account_id);

        match delete_generic_password(&service, account_id) {
            Ok(_) => {
                info!("Deleted OAuth tokens for {}", account_id);
                Ok(())
            }
            Err(e) => {
                let error_str = e.to_string();
                if error_str.contains("not found") || error_str.contains("-25300") {
                    debug!("No tokens to delete for {}", account_id);
                    Ok(())
                } else {
                    error!("Failed to delete tokens from keychain: {}", e);
                    Err(Error::Keychain(format!("Failed to delete tokens: {}", e)))
                }
            }
        }
    }

    /// Update just the access token (after refresh)
    pub fn update_access_token(
        account_id: &str,
        access_token: &str,
        expires_at: i64,
    ) -> Result<()> {
        let mut tokens = Self::get_tokens(account_id)?
            .ok_or_else(|| Error::Keychain("No existing tokens to update".to_string()))?;

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
}
