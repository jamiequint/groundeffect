//! MCP tool implementations

use std::sync::Arc;
use std::process::Command;

use chrono::{DateTime, Local, Utc};
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
            description: "Add a new Google account via OAuth. IMPORTANT: Before calling this tool, ask the user how many years of email/calendar history they want to sync (1-20 years, or 'all' for everything). Opens a browser for authentication.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "alias": {
                        "type": "string",
                        "description": "Optional friendly name for the account (e.g., 'work', 'personal')"
                    },
                    "years_to_sync": {
                        "type": "string",
                        "description": "IMPORTANT: Ask the user before proceeding. How many years of email/calendar history to sync: '1'-'20' for specific years, or 'all' for entire history. More years = longer initial sync time.",
                        "default": "1"
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
            name: "list_recent_emails".to_string(),
            description: "List recent emails sorted by date (newest first). Much faster than search_emails for just getting recent messages.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account email or alias. Omit to list from ALL accounts."
                    },
                    "limit": {
                        "type": "integer",
                        "default": 10,
                        "maximum": 100,
                        "description": "Number of emails to return"
                    }
                }
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
            name: "extend_sync_range".to_string(),
            description: "Extend sync to include older data (both emails and calendar events). Shows current sync status and allows syncing further back. Syncs newest-first to prioritize recent data within the new range.".to_string(),
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
        // Daemon management tools
        ToolDefinition {
            name: "start_daemon".to_string(),
            description: "Start the GroundEffect sync daemon. The daemon syncs emails and calendar events in the background.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "logging": {
                        "type": "boolean",
                        "description": "Enable file logging to ~/.groundeffect/logs/daemon.log. Useful for debugging sync issues."
                    }
                }
            }),
        },
        ToolDefinition {
            name: "stop_daemon".to_string(),
            description: "Stop the running GroundEffect sync daemon.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: "get_daemon_status".to_string(),
            description: "Check if the GroundEffect sync daemon is running.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
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
            "list_recent_emails" => self.list_recent_emails(arguments).await,
            "get_email" => self.get_email(arguments).await,
            "get_thread" => self.get_thread(arguments).await,
            "list_folders" => self.list_folders(arguments).await,
            "search_calendar" => self.search_calendar(arguments).await,
            "get_event" => self.get_event(arguments).await,
            "list_calendars" => self.list_calendars(arguments).await,
            "create_event" => self.create_event(arguments).await,
            "get_sync_status" => self.get_sync_status(arguments).await,
            "reset_sync" => self.reset_sync(arguments).await,
            "extend_sync_range" => self.extend_sync_range(arguments).await,
            "start_daemon" => self.start_daemon(arguments).await,
            "stop_daemon" => self.stop_daemon().await,
            "get_daemon_status" => self.get_daemon_status().await,
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

        // Parse years_to_sync: "1"-"20" for specific years, "all" for no limit
        let years_to_sync_str = args["years_to_sync"].as_str().unwrap_or("1");
        let years_to_sync: Option<u32> = if years_to_sync_str.eq_ignore_ascii_case("all") {
            None // No limit
        } else {
            let years = years_to_sync_str.parse::<u32>()
                .map_err(|_| Error::InvalidRequest(format!(
                    "Invalid years_to_sync value '{}'. Use '1'-'20' or 'all'", years_to_sync_str
                )))?;
            if years < 1 || years > 20 {
                return Err(Error::InvalidRequest(
                    "years_to_sync must be between 1 and 20, or 'all'".to_string()
                ));
            }
            Some(years)
        };

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
        let sync_since = years_to_sync.map(|years| {
            Utc::now() - Duration::days(years as i64 * 365)
        });

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
                    "years_to_sync": years_to_sync_str
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
                    "years_to_sync": years_to_sync_str
                },
                "next_steps": "Use start_daemon tool to begin syncing"
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

        // For wildcard/empty queries, use fast path (no semantic search needed)
        let query_trimmed = query.trim();
        if query_trimmed.is_empty() || query_trimmed == "*" {
            info!("Wildcard query detected, using fast list path");
            return self.list_recent_emails(args).await;
        }

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

    /// List recent emails (fast, no search)
    async fn list_recent_emails(&self, args: &Value) -> Result<Value> {
        let limit = args["limit"].as_u64().unwrap_or(10) as usize;
        let limit = limit.min(100); // Cap at 100

        // Resolve account if provided (handle both "account" and "accounts" params)
        let account_id = args["account"]
            .as_str()
            .and_then(|id| self.config.resolve_account(id))
            .or_else(|| {
                // Also check "accounts" array (for search_emails redirect)
                args["accounts"]
                    .as_array()
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.as_str())
                    .and_then(|id| self.config.resolve_account(id))
            });

        info!("Listing recent emails: account={:?}, limit={}", account_id, limit);

        let start = std::time::Instant::now();
        let emails = self.db.list_recent_emails(account_id.as_deref(), limit).await?;
        let query_time = start.elapsed().as_millis();

        // Convert to summaries
        let results: Vec<_> = emails.iter().map(|e| {
            serde_json::json!({
                "id": e.id,
                "subject": e.subject,
                "from": e.from.to_string_full(),
                "date": e.date.to_rfc3339(),
                "snippet": e.snippet,
                "folder": e.folder,
                "is_read": e.is_read(),
                "has_attachments": e.has_attachments()
            })
        }).collect();

        Ok(serde_json::json!({
            "emails": results,
            "count": results.len(),
            "query_time_ms": query_time
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
    async fn get_sync_status(&self, _args: &Value) -> Result<Value> {
        // Refresh table handles to see latest data from daemon
        self.db.refresh_tables().await?;

        let accounts = self.db.list_accounts().await?;

        // Try to read daemon's progress file for live sync progress
        let progress_file = self.config.sync_progress_file();
        let sync_progress: Option<Vec<crate::sync::AccountSyncState>> = std::fs::read_to_string(&progress_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());

        let mut account_stats = Vec::new();
        let mut total_emails = 0u64;
        let mut total_events = 0u64;

        for account in &accounts {
            let email_count = self.db.count_emails(Some(&account.id)).await?;
            let event_count = self.db.count_events(Some(&account.id)).await?;

            total_emails += email_count;
            total_events += event_count;

            // Check for live progress from daemon
            let live_progress = sync_progress
                .as_ref()
                .and_then(|states| states.iter().find(|s| s.account_id == account.id));

            // Build sync progress if available
            let (is_syncing, sync_progress_json) = if let Some(progress_state) = live_progress {
                let progress_json = progress_state.initial_sync_progress.as_ref().map(|progress| {
                    serde_json::json!({
                        "phase": format!("{:?}", progress.phase),
                        "emails_synced": progress.emails_synced,
                        "total_emails_estimated": progress.total_emails_estimated,
                        "events_synced": progress.events_synced,
                        "total_events_estimated": progress.total_events_estimated,
                        "percentage_complete": progress.percentage_complete(),
                        "emails_per_second": progress.emails_per_second,
                        "estimated_seconds_remaining": progress.estimated_seconds_remaining()
                    })
                });
                (progress_state.is_syncing, progress_json)
            } else {
                (false, None)
            };

            // Helper to format UTC time as local time string
            fn format_local_time(dt: DateTime<Utc>) -> String {
                let local: DateTime<Local> = dt.into();
                local.format("%Y-%m-%d %H:%M:%S %Z").to_string()
            }

            // Always include all fields for consistent output
            let stat = serde_json::json!({
                "id": account.id,
                "alias": account.alias,
                "status": format!("{:?}", account.status).to_lowercase(),
                "last_email_sync": account.last_sync_email.map(format_local_time),
                "last_calendar_sync": account.last_sync_calendar.map(format_local_time),
                "email_count": email_count,
                "event_count": event_count,
                "attachment_count": 0, // TODO
                "is_syncing": is_syncing,
                "sync_progress": sync_progress_json
            });

            account_stats.push(stat);
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
            "next_steps": "Use start_daemon tool to re-sync"
        }))
    }

    /// Extend sync range to include older emails and calendar events
    async fn extend_sync_range(&self, args: &Value) -> Result<Value> {
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

        let event_count = self.db.count_events(Some(&email)).await?;

        // If no target_date provided, just return current status
        if target_date.is_none() {
            return Ok(serde_json::json!({
                "account": email,
                "current_sync_status": {
                    "configured_sync_from": current_sync_from.format("%Y-%m-%d").to_string(),
                    "oldest_synced": oldest_synced.format("%Y-%m-%d").to_string(),
                    "email_count": email_count,
                    "event_count": event_count,
                    "message": format!(
                        "Currently synced back to {}. {} emails and {} calendar events in database.",
                        oldest_synced.format("%Y-%m-%d"),
                        email_count,
                        event_count
                    )
                },
                "usage": "To sync older data, call again with target_date parameter (YYYY-MM-DD format)"
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
                "Extended sync range from {} to {}. The daemon will sync {} additional days of emails and calendar events (newest first).",
                current_sync_from.format("%Y-%m-%d"),
                target_date_str,
                (current_sync_from - target_datetime).num_days()
            ),
            "note": "Data is synced newest-first within the new range. Stable IDs prevent duplicates.",
            "next_steps": "Use start_daemon tool to begin syncing older data"
        }))
    }

    /// Get the path to the daemon binary (sibling of current executable)
    fn get_daemon_binary_path(&self) -> Result<std::path::PathBuf> {
        let current_exe = std::env::current_exe()
            .map_err(|e| Error::Other(format!("Failed to get current executable path: {}", e)))?;

        let exe_dir = current_exe.parent()
            .ok_or_else(|| Error::Other("Failed to get executable directory".to_string()))?;

        let daemon_path = exe_dir.join("groundeffect-daemon");

        if daemon_path.exists() {
            Ok(daemon_path)
        } else {
            Err(Error::Other(format!(
                "Daemon binary not found at {:?}. Make sure groundeffect-daemon is built.",
                daemon_path
            )))
        }
    }

    /// Check if daemon is running by reading PID file or using pgrep
    fn is_daemon_running(&self) -> Option<u32> {
        // First, try PID file (for daemons started via MCP tool)
        let pid_file = self.config.daemon_pid_file();

        if pid_file.exists() {
            if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
                if let Ok(pid) = pid_str.trim().parse::<u32>() {
                    // Verify the process is actually running
                    #[cfg(unix)]
                    {
                        use std::process::Command;
                        let output = Command::new("kill")
                            .args(["-0", &pid.to_string()])
                            .output()
                            .ok();

                        if let Some(out) = output {
                            if out.status.success() {
                                return Some(pid);
                            }
                        }
                        // Process not running, clean up stale PID file
                        let _ = std::fs::remove_file(&pid_file);
                    }

                    #[cfg(not(unix))]
                    {
                        return Some(pid);
                    }
                }
            }
        }

        // Fallback: use pgrep to find daemon (for manually started daemons)
        #[cfg(unix)]
        {
            use std::process::Command;
            let output = Command::new("pgrep")
                .args(["-f", "groundeffect-daemon"])
                .output()
                .ok()?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // pgrep returns one PID per line, take the first one
                if let Some(first_line) = stdout.lines().next() {
                    if let Ok(pid) = first_line.trim().parse::<u32>() {
                        return Some(pid);
                    }
                }
            }
        }

        None
    }

    /// Start the daemon
    async fn start_daemon(&self, arguments: &Value) -> Result<Value> {
        // Parse logging option - check argument first, then environment variable
        let enable_logging = arguments.get("logging")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| {
                std::env::var("GROUNDEFFECT_DAEMON_LOGGING")
                    .map(|v| v == "true" || v == "1")
                    .unwrap_or(false)
            });

        // Check if already running
        if let Some(pid) = self.is_daemon_running() {
            return Ok(serde_json::json!({
                "success": false,
                "message": format!("Daemon is already running (PID {})", pid),
                "status": "running",
                "pid": pid
            }));
        }

        // Get daemon binary path
        let daemon_path = self.get_daemon_binary_path()?;

        // Get credentials from environment (they should be set in the MCP wrapper script)
        let client_id = std::env::var("GROUNDEFFECT_GOOGLE_CLIENT_ID")
            .or_else(|_| std::env::var("GROUNDEFFECT_CLIENT_ID"))
            .ok();
        let client_secret = std::env::var("GROUNDEFFECT_GOOGLE_CLIENT_SECRET")
            .or_else(|_| std::env::var("GROUNDEFFECT_CLIENT_SECRET"))
            .ok();

        // Start daemon as background process
        let mut cmd = Command::new(&daemon_path);

        // Add logging flag if requested
        if enable_logging {
            cmd.arg("--log");
        }

        // Pass through OAuth credentials if available
        if let Some(id) = &client_id {
            cmd.env("GROUNDEFFECT_GOOGLE_CLIENT_ID", id);
        }
        if let Some(secret) = &client_secret {
            cmd.env("GROUNDEFFECT_GOOGLE_CLIENT_SECRET", secret);
        }

        // Spawn the daemon
        let child = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| Error::Other(format!("Failed to start daemon: {}", e)))?;

        let pid = child.id();

        // Write PID file
        let pid_file = self.config.daemon_pid_file();
        if let Some(parent) = pid_file.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&pid_file, pid.to_string())
            .map_err(|e| Error::Other(format!("Failed to write PID file: {}", e)))?;

        // Wait a moment for daemon to start
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Verify it's running
        if self.is_daemon_running().is_some() {
            let message = if enable_logging {
                "Daemon started successfully with file logging enabled"
            } else {
                "Daemon started successfully"
            };
            Ok(serde_json::json!({
                "success": true,
                "message": message,
                "status": "running",
                "pid": pid,
                "logging_enabled": enable_logging,
                "log_file": if enable_logging { Some("~/.groundeffect/logs/daemon.log") } else { None }
            }))
        } else {
            // Clean up PID file if daemon didn't start
            let _ = std::fs::remove_file(&pid_file);
            Err(Error::Other("Daemon started but exited immediately. Check logs for errors.".to_string()))
        }
    }

    /// Stop the daemon
    async fn stop_daemon(&self) -> Result<Value> {
        let pid = match self.is_daemon_running() {
            Some(pid) => pid,
            None => {
                return Ok(serde_json::json!({
                    "success": true,
                    "message": "Daemon is not running",
                    "status": "stopped"
                }));
            }
        };

        // Send SIGTERM to gracefully stop the daemon
        #[cfg(unix)]
        {
            let output = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .output()
                .map_err(|e| Error::Other(format!("Failed to send stop signal: {}", e)))?;

            if !output.status.success() {
                return Err(Error::Other(format!(
                    "Failed to stop daemon: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
        }

        // Wait for daemon to stop
        for _ in 0..10 {
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            if self.is_daemon_running().is_none() {
                break;
            }
        }

        // Clean up PID file
        let pid_file = self.config.daemon_pid_file();
        let _ = std::fs::remove_file(&pid_file);

        if self.is_daemon_running().is_none() {
            Ok(serde_json::json!({
                "success": true,
                "message": "Daemon stopped successfully",
                "status": "stopped"
            }))
        } else {
            // Force kill if still running
            #[cfg(unix)]
            {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .output();
            }

            Ok(serde_json::json!({
                "success": true,
                "message": "Daemon force stopped",
                "status": "stopped"
            }))
        }
    }

    /// Get daemon status
    async fn get_daemon_status(&self) -> Result<Value> {
        match self.is_daemon_running() {
            Some(pid) => {
                // Get additional info about the daemon process
                let mut process_info = serde_json::json!({
                    "running": true,
                    "pid": pid,
                    "status": "running"
                });

                // Try to get process uptime on Unix
                #[cfg(unix)]
                {
                    if let Ok(output) = Command::new("ps")
                        .args(["-o", "etime=", "-p", &pid.to_string()])
                        .output()
                    {
                        if output.status.success() {
                            let uptime = String::from_utf8_lossy(&output.stdout).trim().to_string();
                            if !uptime.is_empty() {
                                process_info["uptime"] = serde_json::json!(uptime);
                            }
                        }
                    }
                }

                Ok(process_info)
            }
            None => {
                Ok(serde_json::json!({
                    "running": false,
                    "status": "stopped",
                    "message": "Daemon is not running. Use start_daemon to start it."
                }))
            }
        }
    }
}
