#!/bin/bash
# Wrapper script for GroundEffect MCP server
# Sources credentials from ~/.secrets

source ~/.secrets
exec /Users/jamiequint/Development/groundeffect/target/release/groundeffect-mcp
