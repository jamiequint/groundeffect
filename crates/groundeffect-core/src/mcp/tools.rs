//! MCP tool implementations

use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::db::Database;
use crate::error::{Error, Result};
use crate::keychain::KeychainManager;
use crate::models::{Account, AccountStatus, SendEmailRequest};
use crate::oauth::OAuthManager;
use crate::search::{CalendarSearchOptions, SearchEngine, SearchOptions};
use super::protocol::{ToolDefinition, ToolResult};

/// Get all tool definitions
pub fn get_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        // Account tools
        ToolDefinition {
            name: "list_accounts".to_string(),
            description: "List all connected Gmail/GCal accounts".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: "get_account".to_string(),
            description: "Get details for a specific account".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account email or alias"
                    }
                },
                "required": ["account"]
            }),
        },
        ToolDefinition {
            name: "add_account".to_string(),
            description: "Add a new Google account via OAuth. Opens a browser for authentication. The tool will wait up to 5 minutes for the OAuth callback.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "alias": {
                        "type": "string",
                        "description": "Optional friendly name for the account (e.g., 'work', 'personal')"
                    },
                    "years_to_sync": {
                        "type": "integer",
                        "description": "How many years of email history to sync (default: 1 year). Use 0 to sync only 90 days.",
                        "default": 1,
                        "minimum": 0,
                        "maximum": 10
                    }
                }
            }),
        },
        // Email tools
        ToolDefinition {
            name: "search_emails".to_string(),
            description: "Search emails using hybrid BM25 + vector search across one or more accounts".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (natural language)"
                    },
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Account(s) to search (email addresses or aliases). Omit to search ALL accounts."
                    },
                    "limit": {
                        "type": "integer",
                        "default": 10,
                        "maximum": 100
                    },
                    "folder": {
                        "type": "string",
                        "description": "Filter by folder (e.g., INBOX, Sent)"
                    },
                    "from": {
                        "type": "string",
                        "description": "Filter by sender email/name"
                    },
                    "to": {
                        "type": "string",
                        "description": "Filter by recipient email/name"
                    },
                    "date_from": {
                        "type": "string",
                        "format": "date",
                        "description": "Filter emails after this date"
                    },
                    "date_to": {
                        "type": "string",
                        "format": "date",
                        "description": "Filter emails before this date"
                    },
                    "has_attachment": {
                        "type": "boolean",
                        "description": "Filter emails with attachments"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "get_email".to_string(),
            description: "Fetch single email by ID".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Email ID"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "get_thread".to_string(),
            description: "Fetch all emails in a thread".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "thread_id": {
                        "type": "string",
                        "description": "Gmail thread ID"
                    },
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to specific accounts"
                    }
                },
                "required": ["thread_id"]
            }),
        },
        ToolDefinition {
            name: "list_folders".to_string(),
            description: "List all IMAP folders".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to specific accounts"
                    }
                }
            }),
        },
        // Calendar tools
        ToolDefinition {
            name: "search_calendar".to_string(),
            description: "Search calendar events across one or more accounts".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (natural language)"
                    },
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Account(s) to search. Omit to search ALL accounts."
                    },
                    "limit": {
                        "type": "integer",
                        "default": 10,
                        "maximum": 100
                    },
                    "calendar_id": {
                        "type": "string",
                        "description": "Filter to specific calendar"
                    },
                    "date_from": {
                        "type": "string",
                        "format": "date",
                        "description": "Filter events after this date"
                    },
                    "date_to": {
                        "type": "string",
                        "format": "date",
                        "description": "Filter events before this date"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "get_event".to_string(),
            description: "Fetch single calendar event by ID".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Event ID"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "list_calendars".to_string(),
            description: "List all calendars".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to specific accounts"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "create_event".to_string(),
            description: "Create a new calendar event".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account to create event on (email or alias)"
                    },
                    "summary": {
                        "type": "string",
                        "description": "Event title"
                    },
                    "start": {
                        "type": "string",
                        "format": "date-time",
                        "description": "Start time (ISO 8601)"
                    },
                    "end": {
                        "type": "string",
                        "format": "date-time",
                        "description": "End time (ISO 8601)"
                    },
                    "calendar_id": {
                        "type": "string",
                        "description": "Calendar ID (omit for primary)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Event description"
                    },
                    "location": {
                        "type": "string",
                        "description": "Event location"
                    },
                    "attendees": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Attendee email addresses"
                    }
                },
                "required": ["account", "summary", "start", "end"]
            }),
        },
        // System tools
        ToolDefinition {
            name: "get_sync_status".to_string(),
            description: "Get current sync status and statistics".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to specific accounts"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "reset_sync".to_string(),
            description: "Clear all synced emails and events for an account. The account will remain connected but all local data will be deleted. Run the daemon again to re-sync.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account email or alias to reset"
                    },
                    "confirm": {
                        "type": "boolean",
                        "description": "Must be true to confirm deletion"
                    }
                },
                "required": ["account", "confirm"]
            }),
        },
        ToolDefinition {
            name: "sync_older_emails".to_string(),
            description: "Extend sync to include older emails. Shows current sync status and allows syncing further back. Syncs newest-first to prioritize recent emails within the new range.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account email or alias"
                    },
                    "target_date": {
                        "type": "string",
                        "format": "date",
                        "description": "Date to sync back to (YYYY-MM-DD format). If not provided, returns current sync status."
                    }
                },
                "required": ["account"]
            }),
        },
    ]
}

/// Tool execution handler
pub struct ToolHandler {
    db: Arc<Database>,
    config: Arc<Config>,
    search: Arc<SearchEngine>,
    oauth: Arc<OAuthManager>,
}

impl ToolHandler {
    /// Create a new tool handler
    pub fn new(
        db: Arc<Database>,
        config: Arc<Config>,
        search: Arc<SearchEngine>,
        oauth: Arc<OAuthManager>,
    ) -> Self {
        Self {
            db,
            config,
            search,
            oauth,
        }
    }

    /// Execute a tool
    pub async fn execute(&self, name: &str, arguments: &Value) -> Result<Value> {
        debug!("Executing tool: {} with args: {:?}", name, arguments);

        let result = match name {
            "list_accounts" => self.list_accounts().await,
            "get_account" => self.get_account(arguments).await,
            "add_account" => self.add_account(arguments).await,
            "search_emails" => self.search_emails(arguments).await,
            "get_email" => self.get_email(arguments).await,
            "get_thread" => self.get_thread(arguments).await,
            "list_folders" => self.list_folders(arguments).await,
            "search_calendar" => self.search_calendar(arguments).await,
            "get_event" => self.get_event(arguments).await,
            "list_calendars" => self.list_calendars(arguments).await,
            "create_event" => self.create_event(arguments).await,
            "get_sync_status" => self.get_sync_status(arguments).await,
            "reset_sync" => self.reset_sync(arguments).await,
            "sync_older_emails" => self.sync_older_emails(arguments).await,
            _ => Err(Error::ToolNotFound(name.to_string())),
        }?;

        Ok(serde_json::json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&result)?
            }]
        }))
    }

    /// List all accounts
    async fn list_accounts(&self) -> Result<Value> {
        let accounts = self.db.list_accounts().await?;
        Ok(serde_json::json!({
            "accounts": accounts.iter().map(|a| serde_json::json!({
                "id": a.id,
                "alias": a.alias,
                "display_name": a.display_name,
                "status": format!("{:?}", a.status).to_lowercase(),
                "added_at": a.added_at.to_rfc3339()
            })).collect::<Vec<_>>()
        }))
    }

    /// Get a specific account
    async fn get_account(&self, args: &Value) -> Result<Value> {
        let account_id = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        // Resolve alias if needed
        let email = self
            .config
            .resolve_account(account_id)
            .ok_or_else(|| Error::AccountNotFound(account_id.to_string()))?;

        let account = self
            .db
            .get_account(&email)
            .await?
            .ok_or_else(|| Error::AccountNotFound(email.clone()))?;

        Ok(serde_json::to_value(&account)?)
    }

    /// Add a new Google account via OAuth
    async fn add_account(&self, args: &Value) -> Result<Value> {
        let alias = args["alias"].as_str().map(|s| s.to_string());
        let years_to_sync = args["years_to_sync"].as_u64().unwrap_or(1) as u32;

        // Generate state for CSRF protection
        let state = format!("groundeffect_{}", uuid::Uuid::new_v4());

        // Generate authorization URL
        let auth_url = self.oauth.authorization_url(&state);

        // Try to bind to the OAuth callback port
        let listener = TcpListener::bind("127.0.0.1:8085").await
            .map_err(|e| Error::Other(format!("Failed to start OAuth callback server: {}. Is another instance running?", e)))?;

        // Open the browser
        if let Err(e) = open::that(&auth_url) {
            warn!("Failed to open browser automatically: {}", e);
        }

        info!("Waiting for OAuth callback on http://localhost:8085 ...");

        // Wait for callback with timeout (5 minutes)
        let callback_result = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            self.wait_for_oauth_callback(&listener, &state)
        ).await;

        let code = match callback_result {
            Ok(Ok(code)) => code,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(Error::Other("OAuth timeout: no callback received within 5 minutes".to_string())),
        };

        // Exchange code for tokens
        let (tokens, user_info) = self.oauth.exchange_code(&code).await?;

        // Store tokens in keychain
        KeychainManager::store_tokens(&user_info.email, &tokens)?;

        // Calculate sync_email_since based on years_to_sync
        use chrono::Duration;
        let sync_since = if years_to_sync == 0 {
            // Default to 90 days
            Some(Utc::now() - Duration::days(90))
        } else {
            Some(Utc::now() - Duration::days(years_to_sync as i64 * 365))
        };

        // Check if account already exists
        if let Some(existing) = self.db.get_account(&user_info.email).await? {
            let mut updated = existing;
            updated.status = AccountStatus::Active;
            updated.alias = alias.or(updated.alias);
            updated.sync_email_since = sync_since;
            self.db.upsert_account(&updated).await?;

            Ok(serde_json::json!({
                "success": true,
                "message": format!("Account {} re-authenticated successfully", user_info.email),
                "account": {
                    "id": updated.id,
                    "alias": updated.alias,
                    "display_name": updated.display_name,
                    "status": "active",
                    "years_to_sync": years_to_sync
                }
            }))
        } else {
            // Create new account
            let account = Account {
                id: user_info.email.clone(),
                alias,
                display_name: user_info.name.unwrap_or_else(|| user_info.email.clone()),
                added_at: Utc::now(),
                last_sync_email: None,
                last_sync_calendar: None,
                status: AccountStatus::Active,
                sync_email_since: sync_since,
                oldest_email_synced: None,
            };
            self.db.upsert_account(&account).await?;

            Ok(serde_json::json!({
                "success": true,
                "message": format!("Account {} added successfully", account.id),
                "account": {
                    "id": account.id,
                    "alias": account.alias,
                    "display_name": account.display_name,
                    "status": "active",
                    "years_to_sync": years_to_sync
                },
                "next_steps": "Run the daemon to start syncing: groundeffect-daemon"
            }))
        }
    }

    /// Wait for OAuth callback and return the authorization code
    async fn wait_for_oauth_callback(&self, listener: &TcpListener, expected_state: &str) -> Result<String> {
        // Accept one connection
        let (mut socket, _) = listener.accept().await
            .map_err(|e| Error::Other(format!("Failed to accept OAuth callback: {}", e)))?;

        // Read the HTTP request
        let mut reader = BufReader::new(&mut socket);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).await
            .map_err(|e| Error::Other(format!("Failed to read OAuth callback: {}", e)))?;

        // Parse the request to extract code and state
        let (code, received_state) = self.parse_oauth_callback(&request_line)?;

        // Verify state
        if received_state != expected_state {
            let response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<h1>Error: Invalid state</h1>";
            let _ = socket.write_all(response.as_bytes()).await;
            return Err(Error::Other("OAuth state mismatch - possible CSRF attack".to_string()));
        }

        // Send success response to browser
        let success_html = r#"HTTP/1.1 200 OK
Content-Type: text/html

<!DOCTYPE html>
<html>
<head><title>GroundEffect - Success</title></head>
<body style="font-family: -apple-system, BlinkMacSystemFont, sans-serif; padding: 40px; text-align: center;">
    <h1>✅ Authentication Successful!</h1>
    <p>You can close this window and return to Claude Code.</p>
</body>
</html>"#;
        let _ = socket.write_all(success_html.as_bytes()).await;

        Ok(code)
    }

    /// Parse OAuth callback URL to extract code and state
    fn parse_oauth_callback(&self, request_line: &str) -> Result<(String, String)> {
        // Request line looks like: GET /oauth/callback?code=xxx&state=yyy HTTP/1.1
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(Error::Other("Invalid HTTP request".to_string()));
        }

        let path = parts[1];
        if !path.starts_with("/oauth/callback") {
            return Err(Error::Other(format!("Unexpected callback path: {}", path)));
        }

        // Parse query string
        let query_start = path.find('?')
            .ok_or_else(|| Error::Other("No query string in callback".to_string()))?;
        let query = &path[query_start + 1..];

        let mut code = None;
        let mut state = None;

        for param in query.split('&') {
            let mut kv = param.splitn(2, '=');
            let key = kv.next().unwrap_or("");
            let value = kv.next().unwrap_or("");
            match key {
                "code" => code = Some(urlencoding::decode(value)
                    .map_err(|e| Error::Other(format!("Failed to decode code: {}", e)))?
                    .into_owned()),
                "state" => state = Some(urlencoding::decode(value)
                    .map_err(|e| Error::Other(format!("Failed to decode state: {}", e)))?
                    .into_owned()),
                _ => {}
            }
        }

        let code = code.ok_or_else(|| Error::Other("No authorization code in callback".to_string()))?;
        let state = state.ok_or_else(|| Error::Other("No state in callback".to_string()))?;

        Ok((code, state))
    }

    /// Search emails
    async fn search_emails(&self, args: &Value) -> Result<Value> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing query".to_string()))?;

        let limit = args["limit"].as_u64().unwrap_or(10) as usize;

        // Resolve account aliases
        let accounts = args["accounts"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|id| self.config.resolve_account(id))
                    .collect::<Vec<_>>()
            });

        let options = SearchOptions {
            accounts,
            limit,
            folder: args["folder"].as_str().map(|s| s.to_string()),
            from: args["from"].as_str().map(|s| s.to_string()),
            ..Default::default()
        };

        let start = std::time::Instant::now();
        let results = self.search.search_emails(query, &options).await?;
        let search_time = start.elapsed().as_millis();

        Ok(serde_json::json!({
            "results": results,
            "accounts_searched": options.accounts.unwrap_or_default(),
            "total_count": results.len(),
            "search_time_ms": search_time
        }))
    }

    /// Get a single email
    async fn get_email(&self, args: &Value) -> Result<Value> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing id".to_string()))?;

        let email = self
            .db
            .get_email(id)
            .await?
            .ok_or_else(|| Error::EmailNotFound(id.to_string()))?;

        Ok(serde_json::to_value(&email)?)
    }

    /// Get all emails in a thread
    async fn get_thread(&self, args: &Value) -> Result<Value> {
        let thread_id = args["thread_id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing thread_id".to_string()))?;

        // TODO: Implement thread fetching from LanceDB
        // For now, return an error indicating the feature is pending
        Err(Error::Other("Thread fetching not yet implemented".to_string()))
    }

    /// List folders
    async fn list_folders(&self, _args: &Value) -> Result<Value> {
        // Return common Gmail folders
        Ok(serde_json::json!({
            "folders": [
                "INBOX",
                "[Gmail]/All Mail",
                "[Gmail]/Drafts",
                "[Gmail]/Important",
                "[Gmail]/Sent Mail",
                "[Gmail]/Spam",
                "[Gmail]/Starred",
                "[Gmail]/Trash"
            ]
        }))
    }

    /// Search calendar events
    async fn search_calendar(&self, args: &Value) -> Result<Value> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing query".to_string()))?;

        let limit = args["limit"].as_u64().unwrap_or(10) as usize;

        // Resolve account aliases
        let accounts = args["accounts"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|id| self.config.resolve_account(id))
                    .collect::<Vec<_>>()
            });

        let options = CalendarSearchOptions {
            accounts,
            limit,
            calendar_id: args["calendar_id"].as_str().map(|s| s.to_string()),
            ..Default::default()
        };

        let start = std::time::Instant::now();
        let results = self.search.search_calendar(query, &options).await?;
        let search_time = start.elapsed().as_millis();

        Ok(serde_json::json!({
            "results": results,
            "accounts_searched": options.accounts.unwrap_or_default(),
            "total_count": results.len(),
            "search_time_ms": search_time
        }))
    }

    /// Get a single event
    async fn get_event(&self, args: &Value) -> Result<Value> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing id".to_string()))?;

        let event = self
            .db
            .get_event(id)
            .await?
            .ok_or_else(|| Error::Other(format!("Event not found: {}", id)))?;

        Ok(serde_json::to_value(&event)?)
    }

    /// List calendars
    async fn list_calendars(&self, _args: &Value) -> Result<Value> {
        // TODO: Implement calendar listing
        Ok(serde_json::json!({
            "calendars": []
        }))
    }

    /// Create a calendar event
    async fn create_event(&self, args: &Value) -> Result<Value> {
        let account = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        let account_email = self
            .config
            .resolve_account(account)
            .ok_or_else(|| Error::AccountNotFound(account.to_string()))?;

        // TODO: Implement event creation via CalDAV
        Err(Error::Other("Event creation not yet implemented".to_string()))
    }

    /// Get sync status
    async fn get_sync_status(&self, args: &Value) -> Result<Value> {
        let accounts = self.db.list_accounts().await?;

        let mut account_stats = Vec::new();
        let mut total_emails = 0u64;
        let mut total_events = 0u64;

        for account in &accounts {
            let email_count = self.db.count_emails(Some(&account.id)).await?;
            let event_count = self.db.count_events(Some(&account.id)).await?;

            total_emails += email_count;
            total_events += event_count;

            account_stats.push(serde_json::json!({
                "id": account.id,
                "alias": account.alias,
                "status": format!("{:?}", account.status).to_lowercase(),
                "last_email_sync": account.last_sync_email.map(|d| d.to_rfc3339()),
                "last_calendar_sync": account.last_sync_calendar.map(|d| d.to_rfc3339()),
                "email_count": email_count,
                "event_count": event_count,
                "attachment_count": 0 // TODO
            }));
        }

        Ok(serde_json::json!({
            "accounts": account_stats,
            "totals": {
                "email_count": total_emails,
                "event_count": total_events,
                "attachment_count": 0,
                "index_size_mb": 0.0,
                "attachment_storage_mb": 0.0
            }
        }))
    }

    /// Reset sync data for an account
    async fn reset_sync(&self, args: &Value) -> Result<Value> {
        let account_id = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        let confirm = args["confirm"].as_bool().unwrap_or(false);
        if !confirm {
            return Err(Error::InvalidRequest(
                "Must set confirm: true to reset sync data".to_string(),
            ));
        }

        // Resolve alias if needed
        let email = self
            .config
            .resolve_account(account_id)
            .ok_or_else(|| Error::AccountNotFound(account_id.to_string()))?;

        // Clear sync data
        let (email_count, event_count) = self.db.clear_account_sync_data(&email).await?;

        // Reset account sync timestamps
        if let Some(mut account) = self.db.get_account(&email).await? {
            account.last_sync_email = None;
            account.last_sync_calendar = None;
            account.oldest_email_synced = None;
            self.db.upsert_account(&account).await?;
        }

        Ok(serde_json::json!({
            "success": true,
            "message": format!("Reset sync data for {}", email),
            "deleted": {
                "emails": email_count,
                "events": event_count
            },
            "next_steps": "Run the daemon to re-sync: groundeffect-daemon"
        }))
    }

    /// Sync older emails beyond the initial sync period
    async fn sync_older_emails(&self, args: &Value) -> Result<Value> {
        let account_id = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        let target_date = args["target_date"].as_str();

        // Resolve alias if needed
        let email = self
            .config
            .resolve_account(account_id)
            .ok_or_else(|| Error::AccountNotFound(account_id.to_string()))?;

        // Get the account
        let account = self
            .db
            .get_account(&email)
            .await?
            .ok_or_else(|| Error::AccountNotFound(email.clone()))?;

        // Get current sync boundaries
        use chrono::Duration;
        let current_sync_from = account.sync_email_since
            .unwrap_or_else(|| Utc::now() - Duration::days(90));
        let oldest_synced = account.oldest_email_synced
            .unwrap_or(current_sync_from);
        let email_count = self.db.count_emails(Some(&email)).await?;

        // If no target_date provided, just return current status
        if target_date.is_none() {
            return Ok(serde_json::json!({
                "account": email,
                "current_sync_status": {
                    "configured_sync_from": current_sync_from.format("%Y-%m-%d").to_string(),
                    "oldest_email_synced": oldest_synced.format("%Y-%m-%d").to_string(),
                    "email_count": email_count,
                    "message": format!(
                        "Currently synced back to {}. {} emails in database.",
                        oldest_synced.format("%Y-%m-%d"),
                        email_count
                    )
                },
                "usage": "To sync older emails, call again with target_date parameter (YYYY-MM-DD format)"
            }));
        }

        // Parse target date
        let target_date_str = target_date.unwrap();
        let parsed_date = chrono::NaiveDate::parse_from_str(target_date_str, "%Y-%m-%d")
            .map_err(|e| Error::InvalidRequest(format!("Invalid date format: {}. Use YYYY-MM-DD", e)))?;

        // Convert to DateTime<Utc>
        let target_datetime = parsed_date
            .and_hms_opt(0, 0, 0)
            .and_then(|dt| dt.and_local_timezone(chrono::Utc).single())
            .ok_or_else(|| Error::InvalidRequest("Failed to parse date".to_string()))?;

        // Validate the target date
        if target_datetime >= current_sync_from {
            return Err(Error::InvalidRequest(format!(
                "Target date {} is already within current sync range (back to {}). \
                 Choose an earlier date.",
                target_date_str,
                current_sync_from.format("%Y-%m-%d")
            )));
        }

        // Update the account with new sync range
        let mut updated_account = account.clone();
        updated_account.sync_email_since = Some(target_datetime);
        self.db.upsert_account(&updated_account).await?;

        Ok(serde_json::json!({
            "success": true,
            "account": email,
            "sync_range": {
                "previous_sync_from": current_sync_from.format("%Y-%m-%d").to_string(),
                "new_sync_from": target_date_str,
                "additional_days": (current_sync_from - target_datetime).num_days()
            },
            "message": format!(
                "Extended sync range from {} to {}. The daemon will sync {} additional days of email (newest first).",
                current_sync_from.format("%Y-%m-%d"),
                target_date_str,
                (current_sync_from - target_datetime).num_days()
            ),
            "note": "Emails are synced newest-first, so recent emails in the new range will be available first. Email IDs are stable, so no duplicates will be created.",
            "next_steps": "Run the daemon to sync older emails: groundeffect-daemon"
        }))
    }
}
