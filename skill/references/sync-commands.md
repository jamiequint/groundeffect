# Sync Commands Reference

## groundeffect sync status

Show synchronization status and statistics.

```bash
groundeffect sync status [options]
```

### Options
| Flag | Description | Example |
|------|-------------|---------|
| `--account` | Show status for specific account | `--account work` |
| `--human` | Human-readable output | `--human` |

### Output Fields
- `account` - Account email/alias
- `configured_range` - How far back sync is configured (years)
- `oldest_synced` - Oldest email/event actually synced
- `email_count` - Number of synced emails
- `event_count` - Number of synced calendar events
- `attachment_stats` - Attachment download statistics
  - `total` - Total attachments found
  - `downloaded` - Attachments downloaded
  - `pending` - Attachments not yet downloaded
  - `size` - Total size of downloaded attachments
- `last_email_sync` - Timestamp of last email sync
- `last_calendar_sync` - Timestamp of last calendar sync

### Examples
```bash
# Show status for all accounts
groundeffect sync status

# Show status for specific account
groundeffect sync status --account work --human
```

---

## groundeffect sync reset

Clear synced data and reset sync state. Requires confirmation.

```bash
groundeffect sync reset <email|alias> --confirm [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--confirm` | Confirm data deletion | Yes |
| `--type` | What to reset: email, calendar, or all (default: all) | No |
| `--human` | Human-readable output | No |

### What Gets Reset
- **email**: All synced emails, sync timestamps, attachment downloads
- **calendar**: All synced events, sync timestamps
- **all**: Both email and calendar data

### Notes
- After reset, daemon will re-sync from configured date range
- Does NOT delete the account or OAuth tokens
- Use this if sync state becomes corrupted

### Examples
```bash
# Reset all data for an account
groundeffect sync reset work --confirm

# Reset only email data
groundeffect sync reset personal --type email --confirm

# Reset only calendar data
groundeffect sync reset work --type calendar --confirm
```

---

## groundeffect sync extend

Expand sync range to include older data.

```bash
groundeffect sync extend <email|alias> --target-date YYYY-MM-DD [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--target-date` | New earliest date to sync (YYYY-MM-DD) | Yes |
| `--human` | Human-readable output | No |

### Notes
- Target date must be older than current sync range
- Daemon will fetch older emails on next sync cycle
- Does not immediately sync; waits for daemon poll

### Examples
```bash
# Extend sync to include 2020 data
groundeffect sync extend work --target-date 2020-01-01

# Extend personal account
groundeffect sync extend personal --target-date 2019-06-15 --human
```

---

## groundeffect sync download-attachments

Retroactively download attachments for already-synced emails.

```bash
groundeffect sync download-attachments <email|alias> [options]
```

### Options
| Flag | Description |
|------|-------------|
| `--human` | Human-readable output |

### Notes
- Downloads all pending attachments for the account
- Useful if `sync-attachments` was previously disabled
- Attachments are stored locally for offline access
- Large attachments may take time to download

### Examples
```bash
# Download all pending attachments
groundeffect sync download-attachments work

# With human-readable output
groundeffect sync download-attachments personal --human
```
