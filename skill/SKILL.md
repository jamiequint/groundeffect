---
name: GroundEffect
description: |
  Use this skill when the user asks about email, calendar, or Gmail/Google Calendar
  management via GroundEffect CLI. Triggers include: "search my email", "list recent emails",
  "check my calendar", "create a calendar event", "manage groundeffect accounts",
  "sync status", "start the daemon", "groundeffect", "ge command".
version: 1.0.0
---

# GroundEffect CLI

GroundEffect provides local-first email and calendar management with semantic search capabilities. All data is synced locally and searchable offline.

## Quick Reference

### Email Commands
```bash
ge email search "query"              # Search emails with natural language
ge email list                        # List recent emails
ge email show <id>                   # Show single email
ge email thread <thread_id>          # Show email thread
ge email send --to X --subject "X" --body "X"  # Send email
ge email attachment <email_id> <filename>      # Get attachment
ge email folders                     # List IMAP folders
```

### Calendar Commands
```bash
ge calendar search "query"           # Search events
ge calendar list                     # List calendars
ge calendar show <event_id>          # Show event details
ge calendar create --summary "X" --start "ISO" --end "ISO"  # Create event
```

### Account Commands
```bash
ge account list                      # List all accounts
ge account show <email|alias>        # Show account details
ge account add                       # Add new Google account
ge account delete <email|alias>      # Remove account
ge account configure <email|alias>   # Update settings
```

### Sync Commands
```bash
ge sync status                       # Check sync status
ge sync reset <email|alias>          # Reset synced data
ge sync extend <email|alias>         # Sync older emails
ge sync download-attachments <email|alias>  # Download pending attachments
```

### Daemon Commands
```bash
ge daemon status                     # Check if daemon running
ge daemon start                      # Start sync daemon
ge daemon stop                       # Stop sync daemon
ge daemon restart                    # Restart daemon
```

## Usage Notes

- **CLI binary**: `groundeffect` (or `ge` alias)
- **Output format**: JSON by default, use `--human` for readable output
- **Date format**: Use YYYY-MM-DD for date parameters
- **Account references**: Use email address or alias interchangeably
- **Help**: Add `--help` to any command for detailed options

## Detailed Documentation

For complete command documentation with all flags and examples, read the appropriate reference file:

- **Email**: `references/email-commands.md` - search, list, show, thread, send, attachment, folders
- **Calendar**: `references/calendar-commands.md` - search, list, show, create
- **Accounts**: `references/account-commands.md` - list, show, add, delete, configure
- **Sync**: `references/sync-commands.md` - status, reset, extend, download-attachments
- **Daemon**: `references/daemon-commands.md` - status, start, stop, restart

## Common Workflows

See `examples/common-workflows.md` for step-by-step usage patterns.
