#!/bin/bash
# Wrapper script for GroundEffect MCP server
# Sources credentials from ~/.secrets

source ~/.secrets
export GROUNDEFFECT_GOOGLE_CLIENT_ID="$GROUNDEFFECT_CLIENT_ID"
export GROUNDEFFECT_GOOGLE_CLIENT_SECRET="$GROUNDEFFECT_CLIENT_SECRET"
exec /Users/jamiequint/Development/groundeffect/target/release/groundeffect-mcp
