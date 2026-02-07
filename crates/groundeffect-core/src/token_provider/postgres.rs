//! PostgreSQL-based token provider
//!
//! Stores OAuth tokens in PostgreSQL with AES-256-GCM encryption.
//! Requires the "postgres" feature to be enabled.
//!
//! # Security
//!
//! - Tokens are encrypted at rest using AES-256-GCM
//! - Encryption key is derived from user-provided key using HKDF-SHA256
//! - Each token gets a unique 96-bit nonce
//!
//! # Database Schema
//!
//! Single-tenant mode (default):
//! ```sql
//! CREATE TABLE IF NOT EXISTS groundeffect_tokens (
//!     email VARCHAR(255) PRIMARY KEY,
//!     encrypted_tokens BYTEA NOT NULL,
//!     created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
//!     updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
//! );
//! ```
//!
//! Multi-tenant mode (when user_id is configured):
//! ```sql
//! CREATE TABLE IF NOT EXISTS groundeffect_tokens (
//!     user_id VARCHAR(255) NOT NULL,
//!     email VARCHAR(255) NOT NULL,
//!     encrypted_tokens BYTEA NOT NULL,
//!     created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
//!     updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
//!     PRIMARY KEY (user_id, email)
//! );
//! ```

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use async_trait::async_trait;
use hkdf::Hkdf;
use sha2::Sha256;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tracing::{debug, info};

use crate::error::{Error, Result};
use crate::keychain::OAuthTokens;

use super::TokenProvider;

/// Nonce size for AES-256-GCM (96 bits = 12 bytes)
const NONCE_SIZE: usize = 12;

/// PostgreSQL-based token provider with encryption
pub struct PostgresTokenProvider {
    pool: PgPool,
    cipher: Aes256Gcm,
    table_name: String,
    /// Optional user_id for multi-tenant deployments
    /// When set, all queries filter by this user_id
    user_id: Option<String>,
}

impl PostgresTokenProvider {
    /// Create a new PostgreSQL token provider
    ///
    /// # Arguments
    ///
    /// * `database_url` - PostgreSQL connection string
    /// * `encryption_key` - User-provided encryption key (will be derived via HKDF)
    /// * `table_name` - Optional custom table name (defaults to "groundeffect_tokens")
    /// * `user_id` - Optional user_id for multi-tenant deployments
    ///
    /// # Example
    ///
    /// Single-tenant mode:
    /// ```ignore
    /// let provider = PostgresTokenProvider::new(
    ///     "postgres://user:pass@localhost/db",
    ///     "my-secret-key",
    ///     Some("my_custom_table"),
    ///     None,
    /// ).await?;
    /// ```
    ///
    /// Multi-tenant mode:
    /// ```ignore
    /// let provider = PostgresTokenProvider::new(
    ///     "postgres://user:pass@localhost/db",
    ///     "my-secret-key",
    ///     Some("groundeffect_tokens"),
    ///     Some("user_123"),
    /// ).await?;
    /// ```
    pub async fn new(
        database_url: &str,
        encryption_key: &str,
        table_name: Option<&str>,
        user_id: Option<&str>,
    ) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .map_err(|e| Error::Config(format!("Failed to connect to database: {}", e)))?;

        let derived_key = Self::derive_key(encryption_key)?;
        let cipher = Aes256Gcm::new(&derived_key.into());

        let table = table_name
            .map(|s| s.to_string())
            .unwrap_or_else(|| "groundeffect_tokens".to_string());

        let provider = Self {
            pool,
            cipher,
            table_name: table,
            user_id: user_id.map(|s| s.to_string()),
        };

        // Ensure table exists with the correct schema
        // Note: For multi-tenant mode, the table must be created externally with
        // the user_id column and composite primary key (user_id, email)
        provider.ensure_table().await?;

        if let Some(ref uid) = provider.user_id {
            info!(
                "PostgreSQL token provider initialized with table: {}, user_id: {}",
                provider.table_name, uid
            );
        } else {
            info!(
                "PostgreSQL token provider initialized with table: {}",
                provider.table_name
            );
        }
        Ok(provider)
    }

    /// Derive a 256-bit key from the user-provided key using HKDF
    fn derive_key(key: &str) -> Result<[u8; 32]> {
        let hkdf = Hkdf::<Sha256>::new(Some(b"groundeffect-tokens"), key.as_bytes());
        let mut okm = [0u8; 32];
        hkdf.expand(b"aes-256-gcm", &mut okm)
            .map_err(|_| Error::Token("Failed to derive encryption key".to_string()))?;
        Ok(okm)
    }

    /// Ensure the tokens table exists
    async fn ensure_table(&self) -> Result<()> {
        let query = format!(
            r#"
            CREATE TABLE IF NOT EXISTS {} (
                email VARCHAR(255) PRIMARY KEY,
                encrypted_tokens BYTEA NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
            self.table_name
        );

        sqlx::query(&query)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Config(format!("Failed to create tokens table: {}", e)))?;

        debug!("Ensured tokens table exists: {}", self.table_name);
        Ok(())
    }

    /// Encrypt tokens using AES-256-GCM
    fn encrypt(&self, tokens: &OAuthTokens) -> Result<Vec<u8>> {
        use aes_gcm::aead::rand_core::RngCore;

        let plaintext = serde_json::to_vec(tokens)
            .map_err(|e| Error::Token(format!("Failed to serialize tokens: {}", e)))?;

        // Generate random nonce
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext.as_ref())
            .map_err(|e| Error::Token(format!("Encryption failed: {}", e)))?;

        // Prepend nonce to ciphertext
        let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);

        Ok(result)
    }

    /// Decrypt tokens
    fn decrypt(&self, data: &[u8]) -> Result<OAuthTokens> {
        if data.len() < NONCE_SIZE {
            return Err(Error::Token(
                "Invalid encrypted data: too short".to_string(),
            ));
        }

        let (nonce_bytes, ciphertext) = data.split_at(NONCE_SIZE);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| Error::Token(format!("Decryption failed: {}", e)))?;

        serde_json::from_slice(&plaintext)
            .map_err(|e| Error::Token(format!("Failed to deserialize tokens: {}", e)))
    }
}

#[async_trait]
impl TokenProvider for PostgresTokenProvider {
    async fn get_tokens(&self, account_id: &str) -> Result<Option<OAuthTokens>> {
        let row: Option<(Vec<u8>,)> = if let Some(ref user_id) = self.user_id {
            // Multi-tenant mode: filter by user_id
            let query = format!(
                "SELECT encrypted_tokens FROM {} WHERE user_id = $1 AND email = $2",
                self.table_name
            );
            sqlx::query_as(&query)
                .bind(user_id)
                .bind(account_id)
                .fetch_optional(&self.pool)
                .await
        } else {
            // Single-tenant mode: email is primary key
            let query = format!(
                "SELECT encrypted_tokens FROM {} WHERE email = $1",
                self.table_name
            );
            sqlx::query_as(&query)
                .bind(account_id)
                .fetch_optional(&self.pool)
                .await
        }
        .map_err(|e| Error::Token(format!("Database query failed: {}", e)))?;

        match row {
            Some((encrypted,)) => {
                let tokens = self.decrypt(&encrypted)?;
                debug!("Retrieved tokens for {} from PostgreSQL", account_id);
                Ok(Some(tokens))
            }
            None => {
                debug!("No tokens found for {} in PostgreSQL", account_id);
                Ok(None)
            }
        }
    }

    async fn store_tokens(&self, account_id: &str, tokens: &OAuthTokens) -> Result<()> {
        let encrypted = self.encrypt(tokens)?;

        if let Some(ref user_id) = self.user_id {
            // Multi-tenant mode: include user_id in insert/update
            let query = format!(
                r#"
                INSERT INTO {} (user_id, email, encrypted_tokens, created_at, updated_at)
                VALUES ($1, $2, $3, NOW(), NOW())
                ON CONFLICT (user_id, email) DO UPDATE SET
                    encrypted_tokens = EXCLUDED.encrypted_tokens,
                    updated_at = NOW()
                "#,
                self.table_name
            );
            sqlx::query(&query)
                .bind(user_id)
                .bind(account_id)
                .bind(&encrypted)
                .execute(&self.pool)
                .await
        } else {
            // Single-tenant mode
            let query = format!(
                r#"
                INSERT INTO {} (email, encrypted_tokens, created_at, updated_at)
                VALUES ($1, $2, NOW(), NOW())
                ON CONFLICT (email) DO UPDATE SET
                    encrypted_tokens = EXCLUDED.encrypted_tokens,
                    updated_at = NOW()
                "#,
                self.table_name
            );
            sqlx::query(&query)
                .bind(account_id)
                .bind(&encrypted)
                .execute(&self.pool)
                .await
        }
        .map_err(|e| Error::Token(format!("Failed to store tokens: {}", e)))?;

        debug!("Stored tokens for {} in PostgreSQL", account_id);
        Ok(())
    }

    async fn delete_tokens(&self, account_id: &str) -> Result<()> {
        if let Some(ref user_id) = self.user_id {
            // Multi-tenant mode: filter by user_id
            let query = format!(
                "DELETE FROM {} WHERE user_id = $1 AND email = $2",
                self.table_name
            );
            sqlx::query(&query)
                .bind(user_id)
                .bind(account_id)
                .execute(&self.pool)
                .await
        } else {
            // Single-tenant mode
            let query = format!("DELETE FROM {} WHERE email = $1", self.table_name);
            sqlx::query(&query)
                .bind(account_id)
                .execute(&self.pool)
                .await
        }
        .map_err(|e| Error::Token(format!("Failed to delete tokens: {}", e)))?;

        info!("Deleted tokens for {} from PostgreSQL", account_id);
        Ok(())
    }

    async fn list_accounts(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = if let Some(ref user_id) = self.user_id {
            // Multi-tenant mode: filter by user_id
            let query = format!("SELECT email FROM {} WHERE user_id = $1", self.table_name);
            sqlx::query_as(&query)
                .bind(user_id)
                .fetch_all(&self.pool)
                .await
        } else {
            // Single-tenant mode
            let query = format!("SELECT email FROM {}", self.table_name);
            sqlx::query_as(&query).fetch_all(&self.pool).await
        }
        .map_err(|e| Error::Token(format!("Failed to list accounts: {}", e)))?;

        let accounts: Vec<String> = rows.into_iter().map(|(email,)| email).collect();
        debug!("Found {} accounts in PostgreSQL", accounts.len());
        Ok(accounts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_derivation() {
        let key1 = PostgresTokenProvider::derive_key("test-key").unwrap();
        let key2 = PostgresTokenProvider::derive_key("test-key").unwrap();
        let key3 = PostgresTokenProvider::derive_key("different-key").unwrap();

        // Same input should produce same output
        assert_eq!(key1, key2);
        // Different input should produce different output
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_encryption_roundtrip() {
        use aes_gcm::KeyInit;

        let key = PostgresTokenProvider::derive_key("test-key").unwrap();
        let cipher = Aes256Gcm::new(&key.into());

        let tokens = OAuthTokens {
            access_token: "access123".to_string(),
            refresh_token: "refresh456".to_string(),
            expires_at: 1234567890,
            scopes: vec!["email".to_string(), "calendar".to_string()],
        };

        // Create a mock provider just for encryption testing
        let provider = PostgresTokenProviderMock { cipher };
        let encrypted = provider.encrypt(&tokens).unwrap();
        let decrypted = provider.decrypt(&encrypted).unwrap();

        assert_eq!(decrypted.access_token, tokens.access_token);
        assert_eq!(decrypted.refresh_token, tokens.refresh_token);
        assert_eq!(decrypted.expires_at, tokens.expires_at);
        assert_eq!(decrypted.scopes, tokens.scopes);
    }

    // Mock for testing encryption without database
    struct PostgresTokenProviderMock {
        cipher: Aes256Gcm,
    }

    impl PostgresTokenProviderMock {
        fn encrypt(&self, tokens: &OAuthTokens) -> Result<Vec<u8>> {
            use aes_gcm::aead::rand_core::RngCore;

            let plaintext = serde_json::to_vec(tokens).unwrap();
            let mut nonce_bytes = [0u8; NONCE_SIZE];
            OsRng.fill_bytes(&mut nonce_bytes);
            let nonce = Nonce::from_slice(&nonce_bytes);

            let ciphertext = self.cipher.encrypt(nonce, plaintext.as_ref()).unwrap();

            let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
            result.extend_from_slice(&nonce_bytes);
            result.extend_from_slice(&ciphertext);
            Ok(result)
        }

        fn decrypt(&self, data: &[u8]) -> Result<OAuthTokens> {
            let (nonce_bytes, ciphertext) = data.split_at(NONCE_SIZE);
            let nonce = Nonce::from_slice(nonce_bytes);
            let plaintext = self.cipher.decrypt(nonce, ciphertext).unwrap();
            Ok(serde_json::from_slice(&plaintext).unwrap())
        }
    }
}
