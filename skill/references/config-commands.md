# Config Commands Reference

Configuration management for GroundEffect daemon settings and Claude Code integration.

## groundeffect config settings

View or modify daemon settings.

```bash
groundeffect config settings [options]
```

### Options
| Flag | Description | Default |
|------|-------------|---------|
| `--logging <bool>` | Enable/disable file logging | Current value |
| `--email-interval <secs>` | Email poll interval (60-3600 seconds) | 300 |
| `--calendar-interval <secs>` | Calendar poll interval (60-3600 seconds) | 300 |
| `--max-fetches <num>` | Max concurrent fetches (1-50) | 10 |
| `--timezone <tz>` | Timezone (e.g., America/Los_Angeles, UTC) | Current value |
| `--embedding-provider <p>` | Embedding backend: local/openrouter/remote | local |
| `--openrouter-model <id>` | OpenRouter model ID | openai/text-embedding-3-small |
| `--openrouter-api-key-env <name>` | Env var holding OpenRouter API key | OPENROUTER_API_KEY |
| `--human` | Human-readable output | |

### Output Fields
- `settings.logging_enabled` - Whether file logging is active
- `settings.email_poll_interval_secs` - Email sync interval
- `settings.calendar_poll_interval_secs` - Calendar sync interval
- `settings.max_concurrent_fetches` - Max parallel connections
- `settings.embedding_provider` - Active embedding backend
- `settings.openrouter_model` - OpenRouter model (if configured)
- `settings.openrouter_api_key_env` - Env var for OpenRouter key
- `daemon_config_path` - Path to daemon config file
- `changes` - Array of changes made (if any)

### Notes
- Without flags, shows current settings
- Config file: `~/.config/groundeffect/daemon.toml`
- Search/general config file: `~/.config/groundeffect/config.toml`
- Changes require a daemon restart to take effect

### Examples
```bash
# View current settings
groundeffect config settings

# Enable logging
groundeffect config settings --logging true

# Set poll intervals
groundeffect config settings --email-interval 600 --calendar-interval 600

# Set all options
groundeffect config settings --logging true --email-interval 300 --max-fetches 15

# Use OpenRouter embeddings
groundeffect config settings --embedding-provider openrouter

# Human-readable output
groundeffect config settings --human
```

---

## groundeffect config add-permissions

Add GroundEffect to Claude Code's allowed commands.

```bash
groundeffect config add-permissions
```

### Output Fields
- `added` - Boolean indicating if permission was added
- `settings_path` - Path to settings file

### Notes
- Modifies `~/.claude/settings.json`
- Adds `Bash(groundeffect:*)` to the allow list
- After adding, groundeffect commands run without permission prompts

### Examples
```bash
# Add permissions
groundeffect config add-permissions
```

---

## groundeffect config remove-permissions

Remove GroundEffect from Claude Code's allowed commands.

```bash
groundeffect config remove-permissions
```

### Output Fields
- `removed` - Boolean indicating if permission was removed
- `settings_path` - Path to settings file

### Notes
- Modifies `~/.claude/settings.json`
- Removes `Bash(groundeffect:*)` from the allow list
- After removing, groundeffect commands require permission prompts

### Examples
```bash
# Remove permissions
groundeffect config remove-permissions
```
