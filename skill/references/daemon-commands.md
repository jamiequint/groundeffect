# Daemon Commands Reference

The GroundEffect daemon runs in the background to continuously sync email and calendar data.

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

## groundeffect daemon start

Start the background sync daemon.

```bash
groundeffect daemon start [options]
```

### Options
| Flag | Description | Default |
|------|-------------|---------|
| `--logging` | Enable file logging | Disabled |
| `--email-poll` | Email sync interval in seconds | 300 (5 min) |
| `--calendar-poll` | Calendar sync interval in seconds | 300 (5 min) |
| `--max-concurrent` | Max parallel API requests | 10 |
| `--human` | Human-readable output | |

### Notes
- Uses launchd on macOS for automatic restart on failure
- Log files stored in `~/.local/share/groundeffect/logs/`
- Will not start if already running (use restart instead)
- Syncs all configured accounts

### Examples
```bash
# Start with defaults
groundeffect daemon start

# Start with logging enabled
groundeffect daemon start --logging

# Start with custom sync intervals
groundeffect daemon start --email-poll 600 --calendar-poll 900

# Start with all options
groundeffect daemon start --logging --email-poll 120 --calendar-poll 300 --max-concurrent 5
```

---

## groundeffect daemon stop

Gracefully stop the sync daemon.

```bash
groundeffect daemon stop [options]
```

### Options
| Flag | Description |
|------|-------------|
| `--human` | Human-readable output |

### Notes
- Sends graceful shutdown signal (SIGTERM)
- Waits for current sync operation to complete
- Uses launchctl on macOS, direct signal on other systems

### Examples
```bash
# Stop the daemon
groundeffect daemon stop

# Stop with human-readable output
groundeffect daemon stop --human
```

---

## groundeffect daemon restart

Stop and restart the daemon with new settings.

```bash
groundeffect daemon restart [options]
```

### Options
| Flag | Description | Default |
|------|-------------|---------|
| `--logging` | Enable file logging | Previous setting |
| `--email-poll` | Email sync interval in seconds | 300 |
| `--calendar-poll` | Calendar sync interval in seconds | 300 |
| `--max-concurrent` | Max parallel API requests | 10 |
| `--human` | Human-readable output | |

### Notes
- Required after changing `sync-attachments` on any account
- Useful for applying new poll intervals
- Gracefully stops existing daemon before starting new one

### Examples
```bash
# Restart with defaults
groundeffect daemon restart

# Restart with new settings
groundeffect daemon restart --email-poll 60 --logging

# Full reconfiguration
groundeffect daemon restart --logging --email-poll 180 --calendar-poll 600 --max-concurrent 15
```
