# Homebrew Tap for GroundEffect

This is the official Homebrew tap for [GroundEffect](https://github.com/jamiequint/groundeffect), a local IMAP/CalDAV sync daemon with MCP server for Claude Code.

## Installation

```bash
brew tap jamiequint/groundeffect
brew install groundeffect
```

## Setup

After installation, follow the caveats shown by Homebrew, or run:

```bash
groundeffect-daemon setup --install
```

This will:
1. Configure daemon settings interactively
2. Install a launchd agent for auto-start at login

## Usage

```bash
# Add a Google account
groundeffect-daemon add-account

# Check daemon status
groundeffect-daemon list-accounts

# Change settings
groundeffect-daemon configure
```

## Uninstallation

```bash
groundeffect-daemon setup --uninstall  # Remove launchd agent first
brew uninstall groundeffect
```

## More Information

See the [main repository](https://github.com/jamiequint/groundeffect) for full documentation.
