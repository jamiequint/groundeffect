//! File-based token provider
//!
//! Wraps the existing KeychainManager for backwards compatibility.
//! Stores tokens in ~/.config/groundeffect/tokens/<account>.json

use async_trait::async_trait;
use std::fs;
use std::path::PathBuf;
use tracing::debug;

use crate::error::Result;
use crate::keychain::{KeychainManager, OAuthTokens};

use super::TokenProvider;

/// File-based token provider using the existing KeychainManager
///
/// This is the default provider and maintains backwards compatibility
/// with existing token storage.
pub struct FileTokenProvider {
    _private: (), // Prevent direct construction
}

impl FileTokenProvider {
    /// Create a new file-based token provider
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Get the tokens directory path
    fn tokens_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("groundeffect")
            .join("tokens")
    }
}

impl Default for FileTokenProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TokenProvider for FileTokenProvider {
    async fn get_tokens(&self, account_id: &str) -> Result<Option<OAuthTokens>> {
        // KeychainManager is synchronous, but that's fine for file I/O
        KeychainManager::get_tokens(account_id)
    }

    async fn store_tokens(&self, account_id: &str, tokens: &OAuthTokens) -> Result<()> {
        KeychainManager::store_tokens(account_id, tokens)
    }

    async fn delete_tokens(&self, account_id: &str) -> Result<()> {
        KeychainManager::delete_tokens(account_id)
    }

    async fn update_access_token(
        &self,
        account_id: &str,
        access_token: &str,
        expires_at: i64,
    ) -> Result<()> {
        KeychainManager::update_access_token(account_id, access_token, expires_at)
    }

    async fn has_tokens(&self, account_id: &str) -> Result<bool> {
        Ok(KeychainManager::has_tokens(account_id))
    }

    async fn list_accounts(&self) -> Result<Vec<String>> {
        let tokens_dir = Self::tokens_dir();

        if !tokens_dir.exists() {
            debug!("Tokens directory does not exist: {:?}", tokens_dir);
            return Ok(vec![]);
        }

        let mut accounts = Vec::new();

        let entries = fs::read_dir(&tokens_dir).map_err(|e| {
            crate::error::Error::Token(format!("Failed to read tokens directory: {}", e))
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(stem) = path.file_stem() {
                    let filename = stem.to_string_lossy();
                    // Reverse the sanitization: _at_ -> @, _ -> .
                    // But be careful: underscores in domain names should stay
                    // The pattern is: name_at_domain_tld.json
                    if let Some(at_pos) = filename.find("_at_") {
                        let local = &filename[..at_pos];
                        let domain = &filename[at_pos + 4..];
                        // Replace remaining underscores with dots in domain
                        let domain = domain.replace('_', ".");
                        let email = format!("{}@{}", local, domain);
                        accounts.push(email);
                    }
                }
            }
        }

        debug!("Found {} accounts with stored tokens", accounts.len());
        Ok(accounts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_file_provider_list_empty() {
        let provider = FileTokenProvider::new();
        // Should not error even if directory doesn't exist
        let result = provider.list_accounts().await;
        assert!(result.is_ok());
    }
}
