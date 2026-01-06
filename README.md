# GroundEffect

Hyper-fast, local Gmail and Google Calendar indexing for Claude Code.

GroundEffect is a local headless IMAP/CalDav client and MCP Server for Claude Code built in Rust with LanceDB.

## Features

- **Hybrid Search**: BM25 full-text + vector semantic search with Reciprocal Rank Fusion
- **Local Embeddings**: Runs nomic-embed-text-v1.5 locally via Candle with Metal acceleration
- **Multi-Account**: Connect unlimited Gmail/GCal accounts with independent sync
- **MCP Integration**: Exposes email and calendar tools directly to Claude Code
- **Real-time Sync**: IMAP IDLE for instant email notifications, CalDAV polling for calendar
- **HTML Text Extraction**: Automatically converts HTML emails to clean plain text using `html2text`

## Quick Start

### 1. Install

```bash
brew tap jamiequint/groundeffect
brew install groundeffect
```

This automatically:
- Installs the daemon (starts at login)
- Installs the Claude Code skill
- Adds permissions to run groundeffect commands

### 2. Configure OAuth

Create a Google Cloud project with OAuth credentials:

1. Go to [Google Cloud Console](https://console.cloud.google.com/)
2. Create a project and enable **Gmail API** and **Google Calendar API**
3. Go to **APIs & Services > Credentials**
4. Create **OAuth client ID** (Desktop app type)
5. Add your credentials:

```bash
echo 'export GROUNDEFFECT_GOOGLE_CLIENT_ID="your-client-id.apps.googleusercontent.com"' >> ~/.zshrc
echo 'export GROUNDEFFECT_GOOGLE_CLIENT_SECRET="your-client-secret"' >> ~/.zshrc
source ~/.zshrc
```

### 3. Add an Account

```bash
groundeffect account add
```

This opens a browser for Google OAuth. After authentication, the daemon syncs automatically.

That's it! Ask Claude Code to search your emails and calendar.

## CLI Reference

All commands output JSON by default. Add `--human` for readable output.

### Account Commands

| Command | Description |
|---------|-------------|
| `account list` | List all connected accounts |
| `account show <account>` | Show account details and sync status |
| `account add` | Add new Google account via OAuth |
| `account delete <account>` | Remove account and all synced data |
| `account configure <account>` | Update account settings (alias, attachments) |

**Parameters for `add`:**

| Parameter | Description | Default |
|-----------|-------------|---------|
| `--years` | Years of email history to sync (1-20 or "all") | 1 |
| `--attachments` | Enable automatic attachment download | off |
| `--alias` | Friendly name for the account | - |

### Email Commands

| Command | Description |
|---------|-------------|
| `email search <query>` | Hybrid BM25 + semantic search |
| `email list` | List recent emails |
| `email show <id>` | Show full email content |
| `email thread <thread_id>` | Show all emails in a thread |
| `email send` | Compose and send email |
| `email attachment <id>` | Get attachment content |
| `email folders` | List IMAP folders |

**Parameters for `search`:**

| Parameter | Description | Default |
|-----------|-------------|---------|
| `--account` | Filter to specific account | all |
| `--limit` | Max results (max: 100) | 10 |
| `--from` | Filter by sender | - |
| `--to` | Filter by recipient | - |
| `--date-from` | Filter after date (YYYY-MM-DD) | - |
| `--date-to` | Filter before date (YYYY-MM-DD) | - |
| `--has-attachment` | Filter emails with attachments | - |

**Parameters for `send`:**

| Parameter | Description |
|-----------|-------------|
| `--from` | Account to send from (**required**) |
| `--to` | Recipient(s) (**required**) |
| `--subject` | Email subject (**required**) |
| `--body` | Email body (**required**) |
| `--cc` | CC recipients |
| `--bcc` | BCC recipients |
| `--reply-to` | Email ID to reply to (for threading) |
| `--html` | Force HTML format (auto-detected from markdown/URLs) |
| `--save-as-draft` | Save as draft instead of sending |
| `--confirm` | Send immediately (without: preview only) |

### Draft Commands

| Command | Description |
|---------|-------------|
| `email draft create` | Create a new email draft |
| `email draft list` | List all drafts for an account |
| `email draft show <id>` | Show full draft content |
| `email draft update <id>` | Update an existing draft |
| `email draft send <id>` | Send a draft |
| `email draft delete <id>` | Delete a draft |

**Parameters for `draft create`:**

| Parameter | Description |
|-----------|-------------|
| `--from` | Account to create draft in (**required**) |
| `--to` | Recipient(s) |
| `--subject` | Email subject |
| `--body` | Email body |
| `--cc` | CC recipients |
| `--bcc` | BCC recipients |
| `--html` | Force HTML format |
| `--reply-to` | Email ID to reply to (for threading) |

### Calendar Commands

| Command | Description |
|---------|-------------|
| `calendar search <query>` | Search events with semantic search |
| `calendar show <id>` | Show event details |
| `calendar create` | Create new event |

**Parameters for `create`:**

| Parameter | Description | Default |
|-----------|-------------|---------|
| `--account` | Account to create event in (**required**) | - |
| `--summary` | Event title (**required**) | - |
| `--start` | Start time (ISO 8601) (**required**) | - |
| `--end` | End time (ISO 8601) (**required**) | - |
| `--description` | Event description | - |
| `--location` | Event location | - |
| `--attendees` | Attendee emails (repeatable) | - |
| `--calendar` | Calendar ID | primary |

### Sync Commands

| Command | Description |
|---------|-------------|
| `sync status` | Show sync status for all accounts |
| `sync reset --account <a> --confirm` | Clear all synced data |
| `sync extend --account <a> --target-date <d>` | Sync older emails back to date |
| `sync resume-from --account <a> --target-date <d>` | Force sync to resume from date |
| `sync download-attachments --account <a>` | Download pending attachments |

### Daemon Commands

| Command | Description |
|---------|-------------|
| `daemon install` | Install launchd agent (auto-start at login) |
| `daemon uninstall` | Remove launchd agent |
| `daemon status` | Check if daemon is running |
| `daemon restart` | Restart the daemon |

### Config Commands

| Command | Description |
|---------|-------------|
| `config settings` | View/modify daemon settings |
| `config add-permissions` | Add to Claude Code allowlist |
| `config remove-permissions` | Remove from Claude Code allowlist |

**Parameters for `settings`:**

| Parameter | Description | Default |
|-----------|-------------|---------|
| `--logging` | Enable/disable file logging | off |
| `--email-interval` | Email poll interval in seconds (60-3600) | 300 |
| `--calendar-interval` | Calendar poll interval in seconds (60-3600) | 300 |
| `--max-fetches` | Max concurrent fetches (1-50) | 10 |

## MCP Integration (Alternative)

If you prefer MCP over the CLI skill, add to `~/.claude.json`:

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

The skill is faster (direct CLI calls vs MCP JSON-RPC overhead) but MCP works with other MCP-compatible clients.

## Build from Source

```bash
git clone https://github.com/jamiequint/groundeffect.git
cd groundeffect
cargo build --release
```

Binaries:
- `target/release/groundeffect` - CLI
- `target/release/groundeffect-daemon` - Background sync daemon
- `target/release/groundeffect-mcp` - MCP server

Install manually:
```bash
# Install binaries
sudo cp target/release/groundeffect* /usr/local/bin/

# Install skill
cp -r skill ~/.claude/skills/groundeffect

# Install daemon
groundeffect daemon install

# Add permissions
groundeffect config add-permissions
```

## Data Storage

```
~/.config/groundeffect/
├── daemon.toml            # Daemon configuration

~/.local/share/groundeffect/
├── lancedb/               # LanceDB database
├── attachments/           # Downloaded attachments
├── models/                # Embedding model files
├── logs/                  # Log files
└── cache/
    └── sync_state/        # Sync state

~/.claude/skills/groundeffect/   # Claude Code skill
```

OAuth tokens are stored securely in the macOS Keychain.

## Troubleshooting

### "OAuth token expired"

Re-authenticate:
```bash
groundeffect account add
```

### Daemon not running

Check status and restart:
```bash
groundeffect daemon status
groundeffect daemon restart
```

Or check launchd:
```bash
launchctl list | grep groundeffect
```

### View logs

```bash
tail -f ~/.local/share/groundeffect/logs/daemon.log
```

Enable logging if disabled:
```bash
groundeffect config settings --logging true
groundeffect daemon restart
```

### High memory usage

Embedding model uses ~500MB-1GB during active embedding. Normal when idle.

## Architecture

```
┌──────────────┐      ┌──────────────────┐
│ Claude Code  │─────►│ groundeffect-mcp │────────┐
│ (MCP Host)   │stdio │                  │        │
└──────────────┘      └──────────────────┘        │
                                                  ▼
┌──────────────┐      ┌──────────────────┐    ┌─────────┐
│ Claude Code  │─────►│ groundeffect     │───►│ LanceDB │
│ (Skill/Bash) │      │ (CLI)            │    └─────────┘
└──────────────┘      └──────────────────┘         ▲
                                                   │
                      ┌──────────────────┐         │
                      │ groundeffect-    │─────────┘
                      │ daemon           │◄──── IMAP/CalDAV
                      └──────────────────┘
```

## License

MIT
