//! Pluggable OAuth token storage providers
//!
//! This module provides a trait-based abstraction for token storage,
//! allowing tokens to be stored in files (default) or external databases.
//!
//! # Configuration
//!
//! In `config.toml`:
//!
//! ```toml
//! # File-based storage (default)
//! [tokens]
//! provider = "file"
//!
//! # PostgreSQL storage (requires "postgres" feature)
//! [tokens]
//! provider = "postgres"
//! database_url_env = "DATABASE_URL"
//! encryption_key_env = "GE_TOKEN_ENCRYPTION_KEY"
//! table_name = "groundeffect_tokens"  # optional
//! ```

mod file;
#[cfg(feature = "postgres")]
mod postgres;

pub use file::FileTokenProvider;
#[cfg(feature = "postgres")]
pub use postgres::PostgresTokenProvider;

use async_trait::async_trait;
use std::sync::Arc;

use crate::config::{Config, TokenProviderConfig};
use crate::error::Result;
use crate::keychain::OAuthTokens;

/// Trait for OAuth token storage backends
///
/// Implementations must be thread-safe (`Send + Sync`) for use across
/// async tasks and the daemon's background sync loops.
#[async_trait]
pub trait TokenProvider: Send + Sync {
    /// Get tokens for an account
    async fn get_tokens(&self, account_id: &str) -> Result<Option<OAuthTokens>>;

    /// Store tokens for an account
    async fn store_tokens(&self, account_id: &str, tokens: &OAuthTokens) -> Result<()>;

    /// Delete tokens for an account
    async fn delete_tokens(&self, account_id: &str) -> Result<()>;

    /// Update just the access token (after refresh)
    async fn update_access_token(
        &self,
        account_id: &str,
        access_token: &str,
        expires_at: i64,
    ) -> Result<()> {
        let mut tokens = self
            .get_tokens(account_id)
            .await?
            .ok_or_else(|| crate::error::Error::Token("No existing tokens to update".to_string()))?;

        tokens.access_token = access_token.to_string();
        tokens.expires_at = expires_at;

        self.store_tokens(account_id, &tokens).await
    }

    /// Check if tokens exist for an account
    async fn has_tokens(&self, account_id: &str) -> Result<bool> {
        Ok(self.get_tokens(account_id).await?.is_some())
    }

    /// List all accounts with stored tokens
    async fn list_accounts(&self) -> Result<Vec<String>>;
}

/// Create a token provider based on configuration
pub async fn create_token_provider(config: &Config) -> Result<Arc<dyn TokenProvider>> {
    match &config.tokens {
        TokenProviderConfig::File => {
            Ok(Arc::new(FileTokenProvider::new()))
        }
        #[cfg(feature = "postgres")]
        TokenProviderConfig::Postgres {
            database_url,
            database_url_env,
            encryption_key_env,
            table_name,
        } => {
            use crate::error::Error;

            // Resolve database URL
            let url = database_url
                .clone()
                .or_else(|| {
                    database_url_env
                        .as_ref()
                        .and_then(|env| std::env::var(env).ok())
                })
                .ok_or_else(|| {
                    Error::Config("database_url or database_url_env required for postgres provider".to_string())
                })?;

            // Get encryption key from environment
            let key = std::env::var(encryption_key_env).map_err(|_| {
                Error::Config(format!(
                    "encryption key env var {} not set",
                    encryption_key_env
                ))
            })?;

            let mut provider = PostgresTokenProvider::new(&url, &key).await?;

            if let Some(name) = table_name {
                provider.set_table_name(name.clone());
            }

            Ok(Arc::new(provider))
        }
        #[cfg(not(feature = "postgres"))]
        TokenProviderConfig::Postgres { .. } => {
            Err(crate::error::Error::Config(
                "PostgreSQL token provider requires the 'postgres' feature".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_file_provider_creation() {
        let config = Config::default();
        let provider = create_token_provider(&config).await.unwrap();

        // Should be able to query without error
        let result = provider.list_accounts().await;
        assert!(result.is_ok());
    }
}
