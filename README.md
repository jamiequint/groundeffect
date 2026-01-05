# GroundEffect

A hyper-fast IMAP/CalDAV client and local MCP server. Syncs Gmail and Google Calendar locally for instant search via Claude Code.

## Features

- **Hybrid Search**: BM25 full-text + vector semantic search with Reciprocal Rank Fusion
- **Local Embeddings**: Runs nomic-embed-text-v1.5 locally via Candle with Metal acceleration
- **Multi-Account**: Connect unlimited Gmail/GCal accounts with independent sync
- **MCP Integration**: Exposes email and calendar tools directly to Claude Code
- **Real-time Sync**: IMAP IDLE for instant email notifications, CalDAV polling for calendar

## Prerequisites

- macOS or Linux (macOS with Metal acceleration recommended)
- Rust toolchain (`rustup`)
- A Google Cloud project with OAuth 2.0 credentials (see setup below)

## Installation

### 1. Build from source

```bash
git clone https://github.com/yourusername/groundeffect.git
cd groundeffect
cargo build --release
```

Binaries will be at:
- `target/release/groundeffect-daemon` - Background sync daemon
- `target/release/groundeffect-mcp` - MCP server for Claude Code

### 2. Set up Google OAuth credentials

Each user needs their own Google Cloud OAuth credentials:

1. Go to [Google Cloud Console](https://console.cloud.google.com/)
2. Create a new project (or use existing)
3. Enable the **Gmail API** and **Google Calendar API**
4. Go to **APIs & Services > Credentials**
5. Click **Create Credentials > OAuth client ID**
6. Select **Desktop app** as application type
7. Download the JSON credentials file

### 3. Configure credentials

Create `~/.secrets` with your OAuth credentials:

```bash
# ~/.secrets
export GROUNDEFFECT_CLIENT_ID="your-client-id.apps.googleusercontent.com"
export GROUNDEFFECT_CLIENT_SECRET="your-client-secret"
```

Secure the file:

```bash
chmod 600 ~/.secrets
```

### 4. Set up MCP for Claude Code

Create a wrapper script that sources credentials:

```bash
# /path/to/groundeffect/groundeffect-mcp.sh
#!/bin/bash
source ~/.secrets
exec /path/to/groundeffect/target/release/groundeffect-mcp
```

```bash
chmod +x groundeffect-mcp.sh
```

Add to your Claude Code config (`~/.claude.json`):

```json
{
  "mcpServers": {
    "groundeffect": {
      "type": "stdio",
      "command": "/path/to/groundeffect/groundeffect-mcp.sh",
      "args": []
    }
  }
}
```

## Usage

### Add an account

```bash
groundeffect-daemon add-account
```

This opens a browser for Google OAuth. After authentication, the account syncs automatically.

### Run the daemon (syncs in background)

```bash
groundeffect-daemon
```

For production, consider setting up a launchd agent to run at login.

### Run MCP server (for Claude Code)

```bash
groundeffect-mcp
```

This is typically invoked automatically by Claude Code via the MCP config.

## MCP Tools

Once connected, Claude Code has access to 18 tools organized into these categories:

### Account Management

| Tool | Description |
|------|-------------|
| `list_accounts` | List all connected Gmail/GCal accounts |
| `get_account` | Get details for a specific account |
| `add_account` | Add a new Google account via OAuth (opens browser for authentication) |

### Email Tools

| Tool | Description |
|------|-------------|
| `search_emails` | Hybrid BM25 + vector search across emails with filters (folder, from, to, date range, attachments) |
| `list_recent_emails` | List recent emails sorted by date (fast, no search overhead) |
| `get_email` | Fetch single email by ID with full content. Uses plain text when available, extracts text from HTML otherwise. Truncates with `truncated: true` flag if body exceeds 50K chars |
| `get_thread` | Fetch all emails in a Gmail thread |
| `list_folders` | List all IMAP folders for accounts |

### Calendar Tools

| Tool | Description |
|------|-------------|
| `search_calendar` | Search calendar events with filters (date range, calendar) |
| `get_event` | Fetch single calendar event by ID |
| `list_calendars` | List all calendars for accounts |
| `create_event` | Create a new calendar event with attendees, location, etc. |

### Sync Management

| Tool | Description |
|------|-------------|
| `get_sync_status` | Get current sync status, statistics, and live progress |
| `reset_sync` | Clear all synced data for an account (keeps account connected) |
| `extend_sync_range` | Extend sync to include older emails and events |

### Daemon Management

| Tool | Description |
|------|-------------|
| `start_daemon` | Start the background sync daemon (with optional logging) |
| `stop_daemon` | Stop the running sync daemon |
| `get_daemon_status` | Check if the sync daemon is running |

### Example queries in Claude Code

```
Search my emails for "quarterly report"
→ Uses search_emails with hybrid BM25 + vector search

Show me my recent emails
→ Uses list_recent_emails (faster than search for just listing)

What meetings do I have about the product launch?
→ Uses search_calendar

Add my work Gmail account
→ Uses add_account to start OAuth flow

Show me the sync status
→ Uses get_sync_status (shows live progress during sync)

Start syncing my email
→ Uses start_daemon to begin background sync
```

## Configuration

Config file location: `~/.config/groundeffect/config.toml`

```toml
[general]
log_level = "info"
data_dir = "~/.local/share/groundeffect/data"

[sync]
email_poll_interval_secs = 300
calendar_poll_interval_secs = 300

[search]
bm25_weight = 0.5
vector_weight = 0.5

[accounts.aliases]
work = "work@gmail.com"
personal = "personal@gmail.com"
```

## Logging

GroundEffect runs as **two separate processes**, each with its own log file:

| Process | Binary | Log File | What it does |
|---------|--------|----------|--------------|
| **Sync Daemon** | `groundeffect-daemon` | `daemon.log` | Background sync, IMAP/CalDAV, embeddings |
| **MCP Server** | `groundeffect-mcp` | `mcp.log` | Handles Claude Code tool calls, searches |

These processes run independently—the MCP server can start/stop the daemon, but they don't share a process. Logging is **disabled by default** for both.

### Log File Location

```
~/.local/share/groundeffect/logs/
├── daemon.log    # Sync daemon logs
└── mcp.log       # MCP server logs
```

### Enable Logging

**Sync Daemon** (any of these methods):
```bash
groundeffect-daemon --log                    # CLI flag
GROUNDEFFECT_DAEMON_LOGGING=true groundeffect-daemon  # Environment variable
```
Or via MCP tool: `start_daemon` with `logging: true`

**MCP Server**:
```bash
GROUNDEFFECT_MCP_LOGGING=true groundeffect-mcp
```

### Enable Both via Claude Code Config

To enable logging for both processes when using Claude Code, add environment variables to `~/.claude.json`:

```json
{
  "mcpServers": {
    "groundeffect": {
      "type": "stdio",
      "command": "/path/to/groundeffect/groundeffect-mcp.sh",
      "args": [],
      "env": {
        "GROUNDEFFECT_DAEMON_LOGGING": "true",
        "GROUNDEFFECT_MCP_LOGGING": "true"
      }
    }
  }
}
```

- `GROUNDEFFECT_MCP_LOGGING` enables MCP server logging immediately when Claude Code connects
- `GROUNDEFFECT_DAEMON_LOGGING` enables daemon logging when started via the `start_daemon` tool

## Data Storage

```
~/.config/groundeffect/
├── config.toml            # Configuration file
└── tokens/                # OAuth tokens (chmod 600)
    └── user_at_gmail_com.json

~/.local/share/groundeffect/
├── lancedb/               # LanceDB database (emails, events, accounts)
├── attachments/           # Downloaded attachments
├── models/                # Embedding model files
├── logs/                  # Log files (if enabled)
└── cache/
    └── sync_state/        # Per-account sync state
```

## Troubleshooting

### "OAuth token expired and refresh failed"

Re-authenticate the account:
```bash
groundeffect-daemon add-account
```

### "Table not found" errors

The database may need initialization. Run:
```bash
groundeffect-daemon
```
Wait for initial sync to complete.

### Slow initial sync

Initial sync downloads recent emails first (last 90 days + unread/flagged), then backfills older emails in the background. The MCP server is usable immediately after the first batch syncs.

### High memory usage

Embedding model runs locally. Expect ~500MB-1GB memory usage during active embedding. Memory usage drops when idle.

## Architecture

```
┌──────────────┐      ┌──────────────────┐
│ Claude Code  │─────►│ groundeffect mcp │──read──► LanceDB
│ (MCP Host)   │stdio │ (read-only DB)   │
└──────────────┘      └──────────────────┘
                                                    ▲
                      ┌──────────────────┐          │ write
                      │ groundeffect     │──────────┘
                      │ daemon (writer)  │◄──── IMAP/CalDAV
                      └──────────────────┘
```

- **Daemon**: Long-running process that syncs emails/calendar, writes to LanceDB
- **MCP Server**: Spawned by Claude Code, reads from LanceDB, mutations go directly to IMAP/CalDAV

## License

MIT
