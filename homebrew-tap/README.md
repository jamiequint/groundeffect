# GroundEffect

Hyper-fast, private email and calendar indexing for Claude Code.

GroundEffect is a local headless IMAP/CalDAV client and MCP Server for Claude Code built in Rust with LanceDB.

This is the official Homebrew tap for [GroundEffect](https://github.com/jamiequint/groundeffect).

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
1. Configure daemon settings interactively (these can be changed later with `groundeffect-daemon configure`)
2. Install a launchd agent for auto-start at login

## Usage

Add a Google account by asking Claude Code:
```
"Add my Gmail account to groundeffect"
```

Or from the command line:
```bash
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
