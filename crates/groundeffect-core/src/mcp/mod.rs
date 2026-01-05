//! MCP (Model Context Protocol) server implementation
//!
//! Provides stdio JSON-RPC interface for Claude Code integration.

mod protocol;
mod tools;
mod resources;

pub use protocol::*;
pub use tools::*;
pub use resources::*;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::db::Database;
use crate::embedding::EmbeddingEngine;
use crate::error::{Error, Result};
use crate::oauth::OAuthManager;
use crate::search::SearchEngine;

/// MCP Server for GroundEffect
pub struct McpServer {
    db: Arc<Database>,
    config: Arc<Config>,
    search: Arc<SearchEngine>,
    oauth: Arc<OAuthManager>,
}

impl McpServer {
    /// Create a new MCP server
    pub fn new(
        db: Arc<Database>,
        config: Arc<Config>,
        embedding: Arc<EmbeddingEngine>,
        oauth: Arc<OAuthManager>,
    ) -> Self {
        let search = Arc::new(SearchEngine::new(db.clone(), embedding));

        Self {
            db,
            config,
            search,
            oauth,
        }
    }

    /// Run the MCP server on stdio
    pub async fn run(&self) -> Result<()> {
        info!("Starting MCP server on stdio");

        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let mut reader = BufReader::new(stdin);
        let mut stdout = stdout;
        let mut line = String::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;

            if n == 0 {
                // EOF
                debug!("Received EOF, shutting down");
                break;
            }

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            debug!("Received request: {}", line);

            // Parse JSON-RPC request
            let request: JsonRpcRequest = match serde_json::from_str(line) {
                Ok(r) => r,
                Err(e) => {
                    let error_response = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: None,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32700,
                            message: format!("Parse error: {}", e),
                            data: None,
                        }),
                    };
                    let response_json = serde_json::to_string(&error_response)?;
                    stdout.write_all(response_json.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                    continue;
                }
            };

            // Handle request
            let response = self.handle_request(&request).await;

            // Send response
            let response_json = serde_json::to_string(&response)?;
            debug!("Sending response: {}", response_json);
            stdout.write_all(response_json.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }

        Ok(())
    }

    /// Handle a JSON-RPC request
    async fn handle_request(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let start = std::time::Instant::now();
        let method = &request.method;

        // Log incoming request (with tool name if it's a tool call)
        let request_desc = if method == "tools/call" {
            if let Some(params) = &request.params {
                let tool_name = params["name"].as_str().unwrap_or("unknown");
                format!("tools/call:{}", tool_name)
            } else {
                "tools/call".to_string()
            }
        } else {
            method.clone()
        };

        info!("→ {}", request_desc);

        let result = match method.as_str() {
            // MCP protocol methods
            "initialize" => self.handle_initialize(&request.params).await,
            "initialized" => Ok(Value::Null),
            "ping" => Ok(Value::String("pong".to_string())),

            // Tool listing
            "tools/list" => self.handle_tools_list().await,

            // Tool execution
            "tools/call" => self.handle_tools_call(&request.params).await,

            // Resource listing
            "resources/list" => self.handle_resources_list().await,

            // Resource reading
            "resources/read" => self.handle_resources_read(&request.params).await,

            _ => Err(Error::McpProtocol(format!(
                "Unknown method: {}",
                method
            ))),
        };

        let elapsed = start.elapsed();
        let elapsed_ms = elapsed.as_millis();

        match result {
            Ok(value) => {
                // Log slow requests with warning
                if elapsed_ms > 1000 {
                    warn!("← {} OK ({}ms) SLOW", request_desc, elapsed_ms);
                } else {
                    info!("← {} OK ({}ms)", request_desc, elapsed_ms);
                }
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: Some(value),
                    error: None,
                }
            },
            Err(e) => {
                error!("← {} ERROR ({}ms): {}", request_desc, elapsed_ms, e);
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: e.to_string(),
                        data: Some(serde_json::json!({
                            "code": e.mcp_code(),
                            "action": e.action_hint()
                        })),
                    }),
                }
            },
        }
    }

    /// Handle initialize request
    async fn handle_initialize(&self, _params: &Option<Value>) -> Result<Value> {
        Ok(serde_json::json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {
                "name": "groundeffect",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {
                "tools": {},
                "resources": {
                    "subscribe": false,
                    "listChanged": false
                }
            }
        }))
    }

    /// Handle tools/list request
    async fn handle_tools_list(&self) -> Result<Value> {
        Ok(serde_json::json!({
            "tools": get_tool_definitions()
        }))
    }

    /// Handle tools/call request
    async fn handle_tools_call(&self, params: &Option<Value>) -> Result<Value> {
        let params = params
            .as_ref()
            .ok_or_else(|| Error::InvalidRequest("Missing params".to_string()))?;

        let name = params["name"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing tool name".to_string()))?;

        let arguments = params.get("arguments").cloned().unwrap_or(Value::Object(Default::default()));

        let tool_handler = ToolHandler::new(
            self.db.clone(),
            self.config.clone(),
            self.search.clone(),
            self.oauth.clone(),
        );

        tool_handler.execute(name, &arguments).await
    }

    /// Handle resources/list request
    async fn handle_resources_list(&self) -> Result<Value> {
        Ok(serde_json::json!({
            "resources": get_resource_definitions()
        }))
    }

    /// Handle resources/read request
    async fn handle_resources_read(&self, params: &Option<Value>) -> Result<Value> {
        let params = params
            .as_ref()
            .ok_or_else(|| Error::InvalidRequest("Missing params".to_string()))?;

        let uri = params["uri"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing resource URI".to_string()))?;

        let resource_handler = ResourceHandler::new(self.db.clone(), self.config.clone());

        resource_handler.read(uri).await
    }
}
