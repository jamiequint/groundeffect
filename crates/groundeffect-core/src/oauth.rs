//! OAuth 2.0 flow for Google authentication

use std::sync::Arc;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

use crate::error::{Error, Result};
use crate::keychain::OAuthTokens;
use crate::token_provider::TokenProvider;

/// Google OAuth configuration
pub struct GoogleOAuthConfig {
    /// OAuth client ID
    pub client_id: String,

    /// OAuth client secret
    pub client_secret: String,

    /// Redirect URI for OAuth callback
    pub redirect_uri: String,
}

impl Default for GoogleOAuthConfig {
    fn default() -> Self {
        // Try env vars first, then fall back to ~/.secrets file
        let (client_id, client_secret) = Self::load_credentials();
        Self {
            client_id,
            client_secret,
            redirect_uri: "http://localhost:8085/oauth/callback".to_string(),
        }
    }
}

impl GoogleOAuthConfig {
    /// Load OAuth credentials from env vars or ~/.secrets file
    fn load_credentials() -> (String, String) {
        // Try env vars first (multiple naming conventions)
        let client_id = std::env::var("GROUNDEFFECT_CLIENT_ID")
            .or_else(|_| std::env::var("GROUNDEFFECT_GOOGLE_CLIENT_ID"))
            .or_else(|_| std::env::var("GOOGLE_CLIENT_ID"))
            .ok();
        let client_secret = std::env::var("GROUNDEFFECT_CLIENT_SECRET")
            .or_else(|_| std::env::var("GROUNDEFFECT_GOOGLE_CLIENT_SECRET"))
            .or_else(|_| std::env::var("GOOGLE_CLIENT_SECRET"))
            .ok();

        if let (Some(id), Some(secret)) = (client_id, client_secret) {
            return (id, secret);
        }

        // Fall back to ~/.secrets file
        if let Some(home) = dirs::home_dir() {
            let secrets_path = home.join(".secrets");
            if let Ok(contents) = std::fs::read_to_string(&secrets_path) {
                let parsed = Self::parse_secrets_file(&contents);
                if let (Some(id), Some(secret)) = (parsed.0, parsed.1) {
                    return (id, secret);
                }
            }
        }

        // Return placeholders if nothing found
        ("YOUR_CLIENT_ID".to_string(), "YOUR_CLIENT_SECRET".to_string())
    }

    /// Parse shell-style exports from secrets file
    fn parse_secrets_file(contents: &str) -> (Option<String>, Option<String>) {
        let mut client_id = None;
        let mut client_secret = None;

        for line in contents.lines() {
            let line = line.trim();
            // Parse: export VAR_NAME="value" or export VAR_NAME='value'
            if let Some(rest) = line.strip_prefix("export ") {
                if let Some((key, value)) = rest.split_once('=') {
                    let key = key.trim();
                    let value = value.trim().trim_matches('"').trim_matches('\'');

                    match key {
                        "GROUNDEFFECT_CLIENT_ID" | "GROUNDEFFECT_GOOGLE_CLIENT_ID" => {
                            client_id = Some(value.to_string());
                        }
                        "GROUNDEFFECT_CLIENT_SECRET" | "GROUNDEFFECT_GOOGLE_CLIENT_SECRET" => {
                            client_secret = Some(value.to_string());
                        }
                        _ => {}
                    }
                }
            }
        }

        (client_id, client_secret)
    }
}

/// Required OAuth scopes for GroundEffect
pub const OAUTH_SCOPES: &[&str] = &[
    "https://mail.google.com/",                     // Full Gmail access (IMAP)
    "https://www.googleapis.com/auth/gmail.send",   // Send emails
    "https://www.googleapis.com/auth/calendar",     // Full Calendar access
    "https://www.googleapis.com/auth/userinfo.email", // Get email address
    "https://www.googleapis.com/auth/userinfo.profile", // Get display name
];

/// Google token endpoint
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Google userinfo endpoint
const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";

/// Response from Google token endpoint
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: i64,
    pub token_type: String,
    pub scope: Option<String>,
}

/// User info from Google
#[derive(Debug, Deserialize)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub picture: Option<String>,
}

/// OAuth manager for handling Google authentication
pub struct OAuthManager {
    config: GoogleOAuthConfig,
    client: Client,
    token_provider: Arc<dyn TokenProvider>,
}

impl OAuthManager {
    /// Create a new OAuth manager with token provider
    pub fn new(token_provider: Arc<dyn TokenProvider>) -> Self {
        Self::with_config(GoogleOAuthConfig::default(), token_provider)
    }

    /// Create with custom config
    pub fn with_config(config: GoogleOAuthConfig, token_provider: Arc<dyn TokenProvider>) -> Self {
        Self {
            config,
            client: Client::new(),
            token_provider,
        }
    }

    /// Generate the OAuth authorization URL
    pub fn authorization_url(&self, state: &str) -> String {
        let scopes = OAUTH_SCOPES.join(" ");
        format!(
            "https://accounts.google.com/o/oauth2/v2/auth?\
             client_id={}&\
             redirect_uri={}&\
             response_type=code&\
             scope={}&\
             access_type=offline&\
             prompt=consent&\
             state={}",
            urlencoding::encode(&self.config.client_id),
            urlencoding::encode(&self.config.redirect_uri),
            urlencoding::encode(&scopes),
            urlencoding::encode(state)
        )
    }

    /// Exchange authorization code for tokens
    pub async fn exchange_code(&self, code: &str) -> Result<(OAuthTokens, UserInfo)> {
        info!("Exchanging authorization code for tokens");

        let params = [
            ("client_id", self.config.client_id.as_str()),
            ("client_secret", self.config.client_secret.as_str()),
            ("code", code),
            ("grant_type", "authorization_code"),
            ("redirect_uri", self.config.redirect_uri.as_str()),
        ];

        let response = self
            .client
            .post(TOKEN_URL)
            .form(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Token exchange failed: {} - {}", status, body);
            return Err(Error::OAuth(format!(
                "Token exchange failed: {} - {}",
                status, body
            )));
        }

        let token_response: TokenResponse = response.json().await?;
        debug!("Token exchange successful");

        let expires_at = chrono::Utc::now().timestamp() + token_response.expires_in;

        let tokens = OAuthTokens {
            access_token: token_response.access_token.clone(),
            refresh_token: token_response
                .refresh_token
                .ok_or_else(|| Error::OAuth("No refresh token in response".to_string()))?,
            expires_at,
            scopes: token_response
                .scope
                .map(|s| s.split_whitespace().map(String::from).collect())
                .unwrap_or_else(|| OAUTH_SCOPES.iter().map(|s| s.to_string()).collect()),
        };

        // Get user info
        let user_info = self.get_user_info(&token_response.access_token).await?;
        info!("Authenticated as {}", user_info.email);

        Ok((tokens, user_info))
    }

    /// Refresh an access token
    pub async fn refresh_token(&self, account_id: &str) -> Result<OAuthTokens> {
        let current_tokens = self.token_provider
            .get_tokens(account_id)
            .await?
            .ok_or_else(|| Error::TokenExpired {
                account: account_id.to_string(),
            })?;

        debug!("Refreshing access token for {}", account_id);

        let params = [
            ("client_id", self.config.client_id.as_str()),
            ("client_secret", self.config.client_secret.as_str()),
            ("refresh_token", current_tokens.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];

        let response = self
            .client
            .post(TOKEN_URL)
            .form(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Token refresh failed for {}: {} - {}", account_id, status, body);
            return Err(Error::TokenRefreshFailed {
                account: account_id.to_string(),
                reason: format!("{} - {}", status, body),
            });
        }

        let token_response: TokenResponse = response.json().await?;
        let expires_at = chrono::Utc::now().timestamp() + token_response.expires_in;

        let new_tokens = OAuthTokens {
            access_token: token_response.access_token,
            // Keep the old refresh token if not provided
            refresh_token: token_response
                .refresh_token
                .unwrap_or(current_tokens.refresh_token),
            expires_at,
            scopes: current_tokens.scopes,
        };

        // Store updated tokens
        self.token_provider.store_tokens(account_id, &new_tokens).await?;
        info!("Refreshed access token for {}", account_id);

        Ok(new_tokens)
    }

    /// Get user info from Google
    pub async fn get_user_info(&self, access_token: &str) -> Result<UserInfo> {
        let response = self
            .client
            .get(USERINFO_URL)
            .bearer_auth(access_token)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::OAuth(format!(
                "Failed to get user info: {} - {}",
                status, body
            )));
        }

        let user_info: UserInfo = response.json().await?;
        Ok(user_info)
    }

    /// Get a valid access token, refreshing if necessary
    pub async fn get_valid_token(&self, account_id: &str) -> Result<String> {
        debug!("Getting valid token for {}", account_id);
        let tokens = self.token_provider
            .get_tokens(account_id)
            .await?
            .ok_or_else(|| Error::TokenExpired {
                account: account_id.to_string(),
            })?;

        if tokens.is_expired() {
            info!("Token expired for {}, refreshing...", account_id);
            let new_tokens = self.refresh_token(account_id).await?;
            info!("Token refreshed for {}", account_id);
            Ok(new_tokens.access_token)
        } else {
            debug!("Token still valid for {}", account_id);
            Ok(tokens.access_token)
        }
    }

    /// Get the token provider
    pub fn token_provider(&self) -> &Arc<dyn TokenProvider> {
        &self.token_provider
    }

    /// Generate XOAUTH2 string for IMAP authentication
    /// Note: async_imap base64-encodes the response, so we return raw bytes
    pub fn generate_xoauth2(email: &str, access_token: &str) -> String {
        let auth_string = format!("user={}\x01auth=Bearer {}\x01\x01", email, access_token);
        debug!("XOAUTH2 raw length: {}", auth_string.len());
        auth_string
    }
}

// Note: OAuthManager no longer implements Default since it requires a TokenProvider.
// Use OAuthManager::new(token_provider) instead.

// URL encoding helper
mod urlencoding {
    pub fn encode(s: &str) -> String {
        url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
    }
}
