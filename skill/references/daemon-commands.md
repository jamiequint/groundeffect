# Daemon Commands Reference

The GroundEffect daemon runs in the background to continuously sync email and calendar data.

## groundeffect daemon install

Install the launchd daemon for automatic startup at login.

```bash
groundeffect daemon install [options]
```

### Options
| Flag | Description | Default |
|------|-------------|---------|
| `--logging` | Enable file logging | Disabled |
| `--human` | Human-readable output | |

### Output Fields
- `installed` - Boolean indicating success
- `plist_path` - Path to launchd plist file
- `started` - Boolean indicating if daemon was started

### Notes
- Creates launchd plist at `~/Library/LaunchAgents/com.groundeffect.daemon.plist`
- Automatically starts the daemon after installation
- Sources `~/.secrets` for OAuth credentials
- Daemon will restart automatically if it crashes (KeepAlive)

### Examples
```bash
# Install with defaults
groundeffect daemon install

# Install with logging enabled
groundeffect daemon install --logging

# Human-readable output
groundeffect daemon install --human
```

---

## groundeffect daemon uninstall

Remove the launchd daemon.

```bash
groundeffect daemon uninstall [options]
```

### Options
| Flag | Description |
|------|-------------|
| `--human` | Human-readable output |

### Output Fields
- `uninstalled` - Boolean indicating success
- `was_running` - Boolean indicating if daemon was running

### Notes
- Stops the daemon if running
- Removes the launchd plist file
- Daemon will no longer start automatically at login

### Examples
```bash
# Uninstall daemon
groundeffect daemon uninstall

# Human-readable output
groundeffect daemon uninstall --human
```

---

## groundeffect daemon status

Check if the sync daemon is running.

```bash
groundeffect daemon status [options]
```

### Options
| Flag | Description |
|------|-------------|
| `--human` | Human-readable output |

### Output Fields
- `running` - Boolean indicating if daemon is active
- `pid` - Process ID (if running)
- `email_poll_interval` - Seconds between email syncs
- `calendar_poll_interval` - Seconds between calendar syncs
- `logging_enabled` - Whether file logging is active
- `log_file` - Path to log file (if logging enabled)

### Examples
```bash
# Check daemon status
groundeffect daemon status

# Human-readable output
groundeffect daemon status --human
```

---

## groundeffect daemon restart

Restart the daemon.

```bash
groundeffect daemon restart [options]
```

### Options
| Flag | Description |
|------|-------------|
| `--human` | Human-readable output |

### Notes
- Required after changing `sync-attachments` on any account
- Useful for applying config changes
- Uses launchctl kickstart to restart

### Examples
```bash
# Restart daemon
groundeffect daemon restart

# Human-readable output
groundeffect daemon restart --human
```
