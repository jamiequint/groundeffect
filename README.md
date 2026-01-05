# GroundEffect

A hyper-fast IMAP/CalDAV client and local MCP server for macOS. Syncs Gmail and Google Calendar locally for instant search via Claude Code.

## Features

- **Hybrid Search**: BM25 full-text + vector semantic search with Reciprocal Rank Fusion
- **Local Embeddings**: Runs nomic-embed-text-v1.5 locally via Candle with Metal acceleration
- **Multi-Account**: Connect unlimited Gmail/GCal accounts with independent sync
- **MCP Integration**: Exposes email and calendar tools directly to Claude Code
- **Real-time Sync**: IMAP IDLE for instant email notifications, CalDAV polling for calendar

## Prerequisites

- macOS (Apple Silicon or Intel with Metal support)
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

## macOS Keychain Setup

GroundEffect stores OAuth tokens in the macOS Keychain. On first use, you'll be prompted to allow access.

### Reducing Keychain Prompts

To avoid repeated password prompts:

1. When the Keychain prompt appears, click **Always Allow**
2. If you still get prompts, open **Keychain Access** app:
   - Find items starting with `groundeffect.oauth.`
   - Double-click each item
   - Go to **Access Control** tab
   - Add `groundeffect` to the list of allowed applications
   - Or select **Allow all applications to access this item**

### Keychain Locked?

If you see errors about keychain access, your keychain may be locked:

```bash
security unlock-keychain ~/Library/Keychains/login.keychain-db
```

### Token Storage

Tokens are stored per-account:
- Keychain item: `groundeffect.oauth.{email-address}`
- Contains: access token, refresh token, expiry

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

Once connected, Claude Code has access to these tools:

| Tool | Description |
|------|-------------|
| `list_accounts` | List all connected Gmail/GCal accounts |
| `get_account` | Get details for a specific account |
| `search_emails` | Hybrid BM25 + vector search across emails |
| `get_email` | Fetch single email by ID |
| `get_thread` | Fetch all emails in a thread |
| `list_folders` | List all IMAP folders |
| `search_calendar` | Search calendar events |
| `get_event` | Fetch single calendar event by ID |
| `list_calendars` | List all calendars |
| `create_event` | Create a new calendar event |
| `get_sync_status` | Get current sync status and statistics |

### Example queries in Claude Code

```
Search my emails for "quarterly report"
→ Uses search_emails with hybrid BM25 + vector search

What meetings do I have about the product launch?
→ Uses search_calendar

Show me the sync status
→ Uses get_sync_status
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

Both the daemon and MCP server can optionally write logs for debugging. Logging is **disabled by default** and must be explicitly enabled.

### Log Files

Logs are written to the macOS standard location:

| Component | Log File | Purpose |
|-----------|----------|---------|
| Daemon | `~/Library/Application Support/com.groundeffect.groundeffect/logs/daemon.log` | Sync operations, IMAP/CalDAV activity |
| MCP Server | `~/Library/Application Support/com.groundeffect.groundeffect/logs/mcp.log` | MCP tool calls, search queries |

Logs use daily rotation and include timestamps, thread IDs, and target module information.

### Enable Daemon Logging

#### Via CLI

```bash
groundeffect-daemon --log
```

#### Via MCP Tool

When using the `start_daemon` MCP tool, pass `logging: true`:

```json
{
  "logging": true
}
```

#### Via Environment Variable

Set `GROUNDEFFECT_DAEMON_LOGGING=true` before starting the daemon.

### Enable MCP Server Logging

Set `GROUNDEFFECT_MCP_LOGGING=true` in your environment.

### Enable Both in Claude Code MCP Config

Add environment variables to your Claude Code MCP config in `~/.claude.json`:

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

This enables logging for both:
- **MCP Server**: Logs immediately when Claude Code connects
- **Daemon**: Logs when started via the `start_daemon` MCP tool

## Data Storage

```
~/.local/share/groundeffect/
├── data/
│   ├── lancedb/           # LanceDB database (emails, events, accounts)
│   ├── attachments/       # Downloaded attachments
│   └── models/            # Embedding model files
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
