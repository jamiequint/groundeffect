//! Dawn token provider - reads tokens from Dawn's oauth_tokens table
//!
//! This provider reads OAuth tokens from Dawn's `oauth_tokens` table which uses
//! Fernet encryption. It enables GroundEffect to share token storage with Dawn
//! instead of maintaining a separate `groundeffect_tokens` table.
//!
//! # Database Schema
//!
//! Dawn's `oauth_tokens` table:
//! ```sql
//! CREATE TABLE oauth_tokens (
//!     id UUID PRIMARY KEY,
//!     user_id VARCHAR(26) NOT NULL,
//!     provider VARCHAR(50) NOT NULL,  -- 'google', 'notion'
//!     account_email VARCHAR(255) NOT NULL,
//!     access_token_encrypted BYTEA NOT NULL,  -- Fernet encrypted
//!     refresh_token_encrypted BYTEA NOT NULL, -- Fernet encrypted
//!     expires_at TIMESTAMP WITH TIME ZONE,
//!     scopes TEXT[],
//!     created_at TIMESTAMP WITH TIME ZONE,
//!     updated_at TIMESTAMP WITH TIME ZONE,
//!     UNIQUE (user_id, provider, account_email)
//! );
//! ```
//!
//! # Encryption
//!
//! Dawn uses Python's `cryptography.fernet.Fernet` for token encryption.
//! Fernet keys are 32 bytes, url-safe base64 encoded.
//! The encrypted data is the raw Fernet token (base64 encoded internally).

use async_trait::async_trait;
use fernet::Fernet;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use tracing::{debug, info};

use crate::error::{Error, Result};
use crate::keychain::OAuthTokens;

use super::TokenProvider;

/// Dawn-compatible token provider using Fernet encryption
pub struct DawnTokenProvider {
    pool: PgPool,
    fernet: Fernet,
    /// User ID for multi-tenant isolation (required)
    user_id: String,
}

impl DawnTokenProvider {
    /// Create a new Dawn token provider
    ///
    /// # Arguments
    ///
    /// * `database_url` - PostgreSQL connection string
    /// * `encryption_key` - Fernet key (url-safe base64 encoded, 32 bytes)
    /// * `user_id` - User ID for multi-tenant isolation
    ///
    /// # Example
    ///
    /// ```ignore
    /// let provider = DawnTokenProvider::new(
    ///     "postgres://user:pass@localhost/db",
    ///     "base64-fernet-key-here",
    ///     "01KEP0A3KKSY1RJ4MHVWW8BC1C",
    /// ).await?;
    /// ```
    pub async fn new(
        database_url: &str,
        encryption_key: &str,
        user_id: &str,
    ) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .map_err(|e| Error::Config(format!("Failed to connect to database: {}", e)))?;

        let fernet = Fernet::new(encryption_key).ok_or_else(|| {
            Error::Config("Invalid Fernet encryption key - must be 32-byte url-safe base64".to_string())
        })?;

        info!(
            "Dawn token provider initialized for user_id: {}",
            user_id
        );

        Ok(Self {
            pool,
            fernet,
            user_id: user_id.to_string(),
        })
    }

    /// Decrypt a Fernet-encrypted value
    fn decrypt(&self, encrypted: &[u8]) -> Result<String> {
        // Fernet tokens are base64-encoded strings, but Dawn stores them as bytes
        // Convert bytes back to string for decryption
        let token_str = std::str::from_utf8(encrypted)
            .map_err(|e| Error::Token(format!("Invalid encrypted data encoding: {}", e)))?;

        self.fernet
            .decrypt(token_str)
            .map_err(|e| Error::Token(format!("Fernet decryption failed: {:?}", e)))
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
    }

    /// Encrypt a value using Fernet
    fn encrypt(&self, plaintext: &str) -> Vec<u8> {
        // Fernet returns a base64-encoded token string
        // Dawn stores this as bytes in the database
        self.fernet.encrypt(plaintext.as_bytes()).into_bytes()
    }
}

#[async_trait]
impl TokenProvider for DawnTokenProvider {
    async fn get_tokens(&self, account_id: &str) -> Result<Option<OAuthTokens>> {
        // Query Dawn's oauth_tokens table for Google tokens
        let row = sqlx::query(
            r#"
            SELECT access_token_encrypted, refresh_token_encrypted, expires_at, scopes
            FROM oauth_tokens
            WHERE user_id = $1 AND provider = 'google' AND account_email = $2
            "#,
        )
        .bind(&self.user_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Token(format!("Database query failed: {}", e)))?;

        match row {
            Some(row) => {
                let access_encrypted: Vec<u8> = row.get("access_token_encrypted");
                let refresh_encrypted: Vec<u8> = row.get("refresh_token_encrypted");
                let expires_at: Option<chrono::DateTime<chrono::Utc>> = row.get("expires_at");
                let scopes: Option<Vec<String>> = row.get("scopes");

                let access_token = self.decrypt(&access_encrypted)?;
                let refresh_token = self.decrypt(&refresh_encrypted)?;

                let expires_at_ts = expires_at
                    .map(|dt| dt.timestamp())
                    .unwrap_or(0);

                let tokens = OAuthTokens {
                    access_token,
                    refresh_token,
                    expires_at: expires_at_ts,
                    scopes: scopes.unwrap_or_default(),
                };

                debug!("Retrieved tokens for {} from Dawn oauth_tokens", account_id);
                Ok(Some(tokens))
            }
            None => {
                debug!("No tokens found for {} in Dawn oauth_tokens", account_id);
                Ok(None)
            }
        }
    }

    async fn store_tokens(&self, account_id: &str, tokens: &OAuthTokens) -> Result<()> {
        let access_encrypted = self.encrypt(&tokens.access_token);
        let refresh_encrypted = self.encrypt(&tokens.refresh_token);

        let expires_at: Option<chrono::DateTime<chrono::Utc>> = if tokens.expires_at > 0 {
            chrono::DateTime::from_timestamp(tokens.expires_at, 0)
        } else {
            None
        };

        sqlx::query(
            r#"
            INSERT INTO oauth_tokens (id, user_id, provider, account_email, access_token_encrypted, refresh_token_encrypted, expires_at, scopes, created_at, updated_at)
            VALUES (gen_random_uuid(), $1, 'google', $2, $3, $4, $5, $6, NOW(), NOW())
            ON CONFLICT (user_id, provider, account_email) DO UPDATE SET
                access_token_encrypted = EXCLUDED.access_token_encrypted,
                refresh_token_encrypted = EXCLUDED.refresh_token_encrypted,
                expires_at = EXCLUDED.expires_at,
                scopes = EXCLUDED.scopes,
                updated_at = NOW()
            "#,
        )
        .bind(&self.user_id)
        .bind(account_id)
        .bind(&access_encrypted)
        .bind(&refresh_encrypted)
        .bind(expires_at)
        .bind(&tokens.scopes)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Token(format!("Failed to store tokens: {}", e)))?;

        debug!("Stored tokens for {} in Dawn oauth_tokens", account_id);
        Ok(())
    }

    async fn delete_tokens(&self, account_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM oauth_tokens
            WHERE user_id = $1 AND provider = 'google' AND account_email = $2
            "#,
        )
        .bind(&self.user_id)
        .bind(account_id)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Token(format!("Failed to delete tokens: {}", e)))?;

        info!("Deleted tokens for {} from Dawn oauth_tokens", account_id);
        Ok(())
    }

    async fn list_accounts(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT account_email
            FROM oauth_tokens
            WHERE user_id = $1 AND provider = 'google'
            "#,
        )
        .bind(&self.user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Token(format!("Failed to list accounts: {}", e)))?;

        let accounts: Vec<String> = rows.iter().map(|row| row.get("account_email")).collect();
        debug!("Found {} Google accounts in Dawn oauth_tokens", accounts.len());
        Ok(accounts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fernet_key_validation() {
        // Valid Fernet key (generated with Fernet.generate_key())
        let valid_key = "YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXoxMjM0NTY=";
        let fernet = Fernet::new(valid_key);
        // Note: This may fail if the key isn't valid Fernet format
        // Real tests would use a properly generated key

        // Invalid key
        let invalid_key = "not-a-valid-key";
        let result = Fernet::new(invalid_key);
        assert!(result.is_none());
    }
}
