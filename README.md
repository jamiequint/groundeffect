# GroundEffect

Hyper-fast, local Gmail and Google Calendar indexing for Claude Code. 

GroundEffect is a local headless IMAP/CalDav client and MCP Server for Claude Code built in Rust with LanceDB.

## Features

- **Hybrid Search**: BM25 full-text + vector semantic search with Reciprocal Rank Fusion
- **Local Embeddings**: Runs nomic-embed-text-v1.5 locally via Candle with Metal acceleration
- **Multi-Account**: Connect unlimited Gmail/GCal accounts with independent sync
- **MCP Integration**: Exposes email and calendar tools directly to Claude Code
- **Real-time Sync**: IMAP IDLE for instant email notifications, CalDAV polling for calendar
- **HTML Text Extraction**: Automatically converts HTML emails to clean plain text using `html2text`, preserving readability while stripping markup

## Prerequisites

- macOS or Linux (macOS with Metal acceleration recommended)
- Rust toolchain (`rustup`) - only needed if building from source
- A Google Cloud project with OAuth 2.0 credentials (see setup below)

## Installation

### Option A: Install via Homebrew (Recommended)

```bash
brew tap jamiequint/groundeffect
brew install groundeffect
```

After installation, run the setup wizard:

```bash
groundeffect-daemon setup --install
```

This will:
1. Configure daemon settings interactively
2. Install a launchd agent for auto-start at login

Then allow Claude Code to run groundeffect commands without permission prompts:

```bash
groundeffect config add-permissions
```

This adds `Bash(groundeffect:*)` to `~/.claude/settings.json`. To remove later: `groundeffect config remove-permissions`.

### Option B: Build from source

```bash
git clone https://github.com/jamiequint/groundeffect.git
cd groundeffect
cargo build --release
```

Binaries will be at:
- `target/release/groundeffect` - CLI for status and management
- `target/release/groundeffect-daemon` - Background sync daemon
- `target/release/groundeffect-mcp` - MCP server for Claude Code

### Set up Google OAuth credentials

Each user needs their own Google Cloud OAuth credentials:

1. Go to [Google Cloud Console](https://console.cloud.google.com/)
2. Create a new project (or use existing)
3. Enable the **Gmail API** and **Google Calendar API**
4. Go to **APIs & Services > Credentials**
5. Click **Create Credentials > OAuth client ID**
6. Select **Desktop app** as application type
7. Download the JSON credentials file

### 3. Set up Claude Code Integration

You can use GroundEffect with Claude Code via either a **Skill** (recommended) or **MCP Server**:

| Method | Startup | Per-Request Overhead | Best For |
|--------|---------|---------------------|----------|
| **Skill** | Instant | ~10-30ms | Claude Code users (recommended) |
| **MCP** | ~2-3s | ~100-300ms | Other MCP clients, or if you prefer MCP |

The skill is faster because it invokes the `groundeffect` CLI directly via Bash, avoiding MCP's JSON-RPC protocol overhead (serialization, stdio buffering, message framing). Each MCP tool call requires multiple round-trips through the protocol layer, while the skill makes a single CLI invocation with JSON output.

Use MCP if you want to use GroundEffect with other MCP-compatible clients (Cursor, Zed, custom integrations), or if you already have an MCP-based workflow you prefer.

#### Option A: Install Skill (Recommended)

The skill teaches Claude Code how to use the `groundeffect` CLI directly.

```bash
# Download the skill (no need to clone the repo)
mkdir -p ~/.claude/skills/groundeffect/{references,examples}
cd ~/.claude/skills/groundeffect
curl -sLO https://raw.githubusercontent.com/jamiequint/groundeffect/main/skill/SKILL.md
curl -sL https://raw.githubusercontent.com/jamiequint/groundeffect/main/skill/references/email-commands.md -o references/email-commands.md
curl -sL https://raw.githubusercontent.com/jamiequint/groundeffect/main/skill/references/calendar-commands.md -o references/calendar-commands.md
curl -sL https://raw.githubusercontent.com/jamiequint/groundeffect/main/skill/references/account-commands.md -o references/account-commands.md
curl -sL https://raw.githubusercontent.com/jamiequint/groundeffect/main/skill/references/sync-commands.md -o references/sync-commands.md
curl -sL https://raw.githubusercontent.com/jamiequint/groundeffect/main/skill/references/daemon-commands.md -o references/daemon-commands.md
curl -sL https://raw.githubusercontent.com/jamiequint/groundeffect/main/skill/examples/common-workflows.md -o examples/common-workflows.md
```

Or clone just the skill folder:
```bash
git clone --depth 1 --filter=blob:none --sparse https://github.com/jamiequint/groundeffect.git /tmp/groundeffect
cd /tmp/groundeffect && git sparse-checkout set skill
cp -r /tmp/groundeffect/skill ~/.claude/skills/groundeffect
rm -rf /tmp/groundeffect
```

The skill is now active. Claude Code will automatically use `groundeffect` CLI commands for email and calendar tasks.

#### Option B: Set up MCP Server

Add to your Claude Code config (`~/.claude.json`):

**Direct credentials:**
```json
{
  "mcpServers": {
    "groundeffect": {
      "type": "stdio",
      "command": "groundeffect-mcp",
      "env": {
        "GROUNDEFFECT_GOOGLE_CLIENT_ID": "your-client-id.apps.googleusercontent.com",
        "GROUNDEFFECT_GOOGLE_CLIENT_SECRET": "your-client-secret"
      }
    }
  }
}
```

**Reference from ~/.secrets (recommended for MCP):**

Create `~/.secrets`:
```bash
export GROUNDEFFECT_GOOGLE_CLIENT_ID="your-client-id.apps.googleusercontent.com"
export GROUNDEFFECT_GOOGLE_CLIENT_SECRET="your-client-secret"
```

Add to your shell profile (`~/.zshrc` or `~/.bashrc`):
```bash
source ~/.secrets
```

Then in `~/.claude.json`:
```json
{
  "mcpServers": {
    "groundeffect": {
      "type": "stdio",
      "command": "groundeffect-mcp",
      "env": {
        "GROUNDEFFECT_GOOGLE_CLIENT_ID": "${GROUNDEFFECT_GOOGLE_CLIENT_ID}",
        "GROUNDEFFECT_GOOGLE_CLIENT_SECRET": "${GROUNDEFFECT_GOOGLE_CLIENT_SECRET}"
      }
    }
  }
}
```

## Usage

### Initial Setup (Homebrew install)

If you installed via Homebrew, run the setup wizard:

```bash
groundeffect-daemon setup --install
```

This configures daemon settings (which can be changed later with `groundeffect-daemon configure`) and installs a launchd agent for auto-start.

### Add an account

Add a Google account by asking Claude Code:
```
"Add my Gmail account to groundeffect"
```

This opens a browser for Google OAuth. After authentication, the account syncs automatically.

### Run the daemon (syncs in background)

```bash
groundeffect-daemon
```

If you used `setup --install`, the daemon starts automatically at login via launchd.

### Change settings

```bash
groundeffect-daemon configure
```

Interactively change settings. Restarts the daemon if running via launchd.

**Available settings:**

| Setting | Default | Description |
|---------|---------|-------------|
| Logging | Off | Write logs to `~/.local/share/groundeffect/logs/daemon.log` |
| Email poll interval | 300s | How often to check for new emails (IMAP IDLE provides instant notifications regardless) |
| Calendar poll interval | 300s | How often to sync calendar events |
| Max concurrent fetches | 10 | Concurrent IMAP connections for parallel downloads (Gmail limit: 15) |

### Check sync status

```bash
groundeffect sync status
```

Shows sync status for all accounts including email/event counts, date ranges, and last sync times.

### Uninstall launchd agent

```bash
groundeffect-daemon setup --uninstall
```

### Run MCP server (for Claude Code)

```bash
groundeffect-mcp
```

This is typically invoked automatically by Claude Code via the MCP config.

## MCP Tools

Once connected, Claude Code has access to these tools. All tools support account aliases where applicable.

### Account Management

#### `manage_accounts`

Manage Gmail/GCal accounts with these actions:

| Action | Description | Required Parameters |
|--------|-------------|---------------------|
| `list` | List all connected accounts | - |
| `get` | Get details for one account | `account` |
| `add` | Start OAuth flow to add account | `years_to_sync` (will prompt if omitted) |
| `delete` | Remove account and all synced data | `account`, `confirm: true` |
| `configure` | Update account settings | `account` + settings to change |

**Parameters for `add`:**
- `years_to_sync`: How much email history to sync - `"1"` to `"20"` or `"all"` (required, will prompt if not provided)
- `alias`: Friendly name for the account (optional)

**Parameters for `configure`:**
- `alias`: Set or change alias (or `null` to remove)
- `sync_email`: Enable/disable email sync (boolean)
- `sync_calendar`: Enable/disable calendar sync (boolean)
- `folders`: Array of folders to sync (empty array = all folders)
- `sync_attachments`: Enable automatic attachment download (boolean, requires daemon restart)

### Email Tools

#### `search_emails`

Hybrid BM25 + vector semantic search across emails.

| Parameter | Type | Description |
|-----------|------|-------------|
| `query` | string | Search query (natural language) - **required** |
| `intent` | string | `"search"` for semantic search, `"list"` for recent/unread (fast path) |
| `accounts` | array | Account emails/aliases to search (omit for all) |
| `limit` | integer | Max results (default: 10, max: 100) |
| `folder` | string | Filter by folder (e.g., `"INBOX"`, `"Sent"`) |
| `from` | string | Filter by sender email/name |
| `to` | string | Filter by recipient email/name |
| `date_from` | string | Filter after date (YYYY-MM-DD) |
| `date_to` | string | Filter before date (YYYY-MM-DD) |
| `has_attachment` | boolean | Filter emails with attachments |

#### `list_recent_emails`

List recent emails sorted by date (newest first). Faster than `search_emails` when you just need recent messages.

| Parameter | Type | Description |
|-----------|------|-------------|
| `account` | string | Account email or alias (omit for all accounts) |
| `limit` | integer | Number of emails (default: 10, max: 100) |

#### `get_email`

Fetch single email by ID with full content.

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | string | Email ID - **required** |

Returns full email with attachments list. Body is truncated with `truncated: true` if it exceeds 40K chars (see [Limitations](#limitations)).

#### `get_thread`

Fetch all emails in a Gmail thread.

| Parameter | Type | Description |
|-----------|------|-------------|
| `thread_id` | string | Gmail thread ID - **required** |
| `accounts` | array | Filter to specific accounts |

#### `send_email`

Compose and send an email. Returns preview for user confirmation by default.

| Parameter | Type | Description |
|-----------|------|-------------|
| `from_account` | string | Account email or alias to send from - **required** |
| `to` | array | Recipient email addresses - **required** |
| `subject` | string | Email subject - **required** |
| `body` | string | Email body (plain text) - **required** |
| `cc` | array | CC recipients |
| `bcc` | array | BCC recipients |
| `reply_to_id` | string | Email ID to reply to (for threading) |
| `confirm` | boolean | Set `true` to send immediately; `false` returns preview |

#### `get_attachment`

Get an email attachment by ID or filename.

| Parameter | Type | Description |
|-----------|------|-------------|
| `email_id` | string | Email ID containing the attachment - **required** |
| `attachment_id` | string | Attachment ID (from email response) |
| `filename` | string | Attachment filename (alternative to attachment_id) |

Returns text content directly for text files, file path for binary files (PDF, images) that can be read with the Read tool.

#### `list_folders`

List all IMAP folders for accounts.

| Parameter | Type | Description |
|-----------|------|-------------|
| `accounts` | array | Filter to specific accounts |

### Calendar Tools

#### `search_calendar`

Search calendar events with semantic search.

| Parameter | Type | Description |
|-----------|------|-------------|
| `query` | string | Search query (natural language) - **required** |
| `accounts` | array | Account emails/aliases to search (omit for all) |
| `limit` | integer | Max results (default: 10, max: 100) |
| `calendar_id` | string | Filter to specific calendar |
| `date_from` | string | Filter after date (YYYY-MM-DD) |
| `date_to` | string | Filter before date (YYYY-MM-DD) |

#### `get_event`

Fetch single calendar event by ID.

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | string | Event ID - **required** |

#### `list_calendars`

List all calendars for accounts.

| Parameter | Type | Description |
|-----------|------|-------------|
| `accounts` | array | Filter to specific accounts |

#### `create_event`

Create a new calendar event.

| Parameter | Type | Description |
|-----------|------|-------------|
| `account` | string | Account email or alias - **required** |
| `summary` | string | Event title - **required** |
| `start` | string | Start time (ISO 8601) - **required** |
| `end` | string | End time (ISO 8601) - **required** |
| `calendar_id` | string | Calendar ID (omit for primary) |
| `description` | string | Event description |
| `location` | string | Event location |
| `attendees` | array | Attendee email addresses |

### Sync Management

#### `manage_sync`

Manage email and calendar sync.

| Action | Description | Required Parameters |
|--------|-------------|---------------------|
| `status` | Show sync status (email/event/attachment counts) | `account` (optional, shows all if omitted) |
| `reset` | Clear all synced data for account | `account`, `confirm: true`, `data_type` (optional) |
| `extend` | Sync older emails back to target date | `account`, `target_date` |
| `resume_from` | Force sync to resume from specific date | `account`, `target_date` |
| `download_attachments` | Download attachments for existing emails | `account` |

**Parameters:**
- `account`: Account email or alias
- `target_date`: Date in YYYY-MM-DD format (for `extend` or `resume_from`)
- `data_type`: `"email"`, `"calendar"`, or `"all"` (for `reset`, default: `"all"`)
- `confirm`: Must be `true` to confirm reset

### Daemon Management

#### `manage_daemon`

Manage the background sync daemon.

| Action | Description |
|--------|-------------|
| `start` | Start the daemon |
| `stop` | Stop the daemon |
| `restart` | Restart the daemon |
| `status` | Check if daemon is running |

**Optional parameters for `start`/`restart`:**
- `logging`: Enable logging to `~/.local/share/groundeffect/logs/` (boolean)
- `email_poll_interval`: Email poll interval in seconds (default: 300)
- `calendar_poll_interval`: Calendar poll interval in seconds (default: 300)
- `max_concurrent_fetches`: Max concurrent IMAP connections (default: 10, Gmail limit: 15)

### Attachments

Attachment **metadata** (filename, size, mime_type) syncs automatically with emails. Attachment **files** are downloaded only when enabled:

1. **Enable attachment sync**: `manage_accounts configure` with `sync_attachments: true`
2. **Download existing attachments**: `manage_sync download_attachments` (runs in background)
3. **View attachment**: `get_attachment` with `email_id` + `attachment_id` or `filename`

Attachments are stored in `~/.local/share/groundeffect/attachments/` with a 50MB size limit per file.

### Example queries in Claude Code

```
Search my emails for "quarterly report"
→ Uses search_emails with hybrid BM25 + vector search

Find emails with attachments from John
→ Uses search_emails with from: "john", has_attachment: true

Get the PDF from that invoice email
→ Uses get_attachment with email_id and filename

Show me my recent emails
→ Uses list_recent_emails (faster than search for just listing)

What meetings do I have about the product launch?
→ Uses search_calendar

Create a meeting for tomorrow at 2pm
→ Uses create_event

Add my work Gmail account
→ Uses manage_accounts with action 'add' to start OAuth flow

Enable attachment sync for my account
→ Uses manage_accounts configure with sync_attachments: true

Show me the sync status
→ Uses manage_sync with action 'status' (shows email/event/attachment counts)
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

# Per-account settings (managed via manage_accounts configure action)
[accounts."work@gmail.com"]
sync_email = true
sync_calendar = false
folders = ["INBOX", "Sent"]
```

## Logging

GroundEffect runs as **two separate processes**, each with its own log file:

| Process | Binary | Log File | What it does |
|---------|--------|----------|--------------|
| **Sync Daemon** | `groundeffect-daemon` | `daemon.log` | Background sync, IMAP/CalDAV, embeddings |
| **MCP Server** | `groundeffect-mcp` | `mcp.log` | Handles Claude Code tool calls, searches |

These processes run independently—the MCP server can start/stop the daemon, but they don't share a process. The daemon always logs to file. MCP server logging is disabled by default.

### Log File Location

```
~/.local/share/groundeffect/logs/
├── daemon.log    # Sync daemon logs
└── mcp.log       # MCP server logs (if enabled)
```

### Enable MCP Server Logging

To enable MCP server logging, add the env var to your `~/.claude.json`:

```json
{
  "mcpServers": {
    "groundeffect": {
      "type": "stdio",
      "command": "groundeffect-mcp",
      "env": {
        "GROUNDEFFECT_GOOGLE_CLIENT_ID": "${GROUNDEFFECT_GOOGLE_CLIENT_ID}",
        "GROUNDEFFECT_GOOGLE_CLIENT_SECRET": "${GROUNDEFFECT_GOOGLE_CLIENT_SECRET}",
        "GROUNDEFFECT_MCP_LOGGING": "true"
      }
    }
  }
}
```

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
├── logs/                  # Log files
└── cache/
    └── sync_state/        # Per-account sync state
```

## Limitations

### Email Body Size

Email bodies are limited to **40,000 characters** (~10 pages of text) when returned via MCP tools. This limit exists because Claude Code's MCP protocol has a token limit for tool results. Emails exceeding this limit are automatically truncated, with `truncated: true` and `total_body_chars` included in the response.

Most emails are well under this limit. Emails that may be truncated include:
- Marketing newsletters with extensive product listings
- Long email threads (use `get_thread` to fetch individual messages)
- Emails with large embedded content

HTML emails are automatically converted to plain text before this limit is applied, which typically reduces size significantly.

## Troubleshooting

### "OAuth token expired and refresh failed"

Re-authenticate by asking Claude Code to add the account again:
```
"Add my Gmail account to groundeffect"
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
