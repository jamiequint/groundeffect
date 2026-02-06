# Account Commands Reference

## groundeffect account list

List all connected Gmail/Google Calendar accounts.

```bash
groundeffect account list [options]
```

### Options
| Flag | Description |
|------|-------------|
| `--human` | Human-readable output |

### Output Fields
- `email` - Account email address
- `alias` - Friendly name (if set)
- `display_name` - Google account display name
- `sync_email` - Whether email sync is enabled
- `sync_calendar` - Whether calendar sync is enabled
- `sync_attachments` - Whether attachment download is enabled
- `folders` - Configured folders to sync (empty = all)

### Examples
```bash
# List all accounts
groundeffect account list

# List with human-readable format
groundeffect account list --human
```

---

## groundeffect account show

Show details for a specific account.

```bash
groundeffect account show <email|alias> [options]
```

### Options
| Flag | Description |
|------|-------------|
| `--human` | Human-readable output |

### Examples
```bash
# Show by email
groundeffect account show user@gmail.com

# Show by alias
groundeffect account show work --human
```

---

## groundeffect account add

Add a new Google account via OAuth flow.

```bash
groundeffect account add [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--alias` | Friendly name for the account | No |
| `--years` | Years of email history to sync (1-20 or "all") | Prompted if not provided |

### Process
1. Opens browser for Google OAuth authentication
2. Waits for authorization (5 minute timeout)
3. Validates and stores tokens securely in OS keychain
4. Configures sync settings
5. Triggers initial sync via daemon

### Examples
```bash
# Add account with prompts
groundeffect account add

# Add with alias and sync 3 years
groundeffect account add --alias work --years 3

# Add and sync all available history
groundeffect account add --alias archive --years all
```

---

## groundeffect account reauth

Re-authenticate an existing account via OAuth without changing sync settings.

```bash
groundeffect account reauth <email|alias> [options]
```

### Options
| Flag | Description |
|------|-------------|
| `--human` | Human-readable output |

### Notes
- Opens browser for OAuth flow
- Refreshes stored OAuth tokens
- Marks account status back to active on success

### Examples
```bash
# Re-auth by email
groundeffect account reauth user@gmail.com

# Re-auth by alias
groundeffect account reauth work --human
```

---

## groundeffect account delete

Remove an account and all associated synced data.

```bash
groundeffect account delete <email|alias> --confirm [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--confirm` | Confirm deletion | Yes |
| `--human` | Human-readable output | No |

### What Gets Deleted
- All synced emails for this account
- All synced calendar events for this account
- OAuth tokens from keychain
- Account configuration

### Examples
```bash
# Delete account (requires --confirm)
groundeffect account delete old@gmail.com --confirm

# Delete by alias
groundeffect account delete old-work --confirm
```

---

## groundeffect account configure

Update settings for an existing account.

```bash
groundeffect account configure <email|alias> [options]
```

### Options
| Flag | Description | Example |
|------|-------------|---------|
| `--alias` | Set or update alias (use empty string to remove) | `--alias work` |
| `--attachments` | Enable automatic attachment downloads | `--attachments` |
| `--no-attachments` | Disable automatic attachment downloads | `--no-attachments` |
| `--human` | Human-readable output | `--human` |

### Notes
- Changes to attachment syncing require daemon restart to take effect

### Examples
```bash
# Set alias
groundeffect account configure user@gmail.com --alias personal

# Enable attachment downloads
groundeffect account configure work --attachments

# Disable attachment downloads
groundeffect account configure work --no-attachments

# Remove alias
groundeffect account configure user@gmail.com --alias ""
```
