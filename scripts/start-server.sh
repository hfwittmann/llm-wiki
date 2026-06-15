#!/usr/bin/env bash
# Start llm-wiki-server with permanent paths under ~/Documents/llm-wiki-*.
#
# Usage:
#   scripts/start-server.sh              # foreground (Ctrl+C to stop)
#   scripts/start-server.sh --background # detached; writes pid to /tmp/llm-wiki-server.pid
#
# Override defaults via env vars:
#   LLM_WIKI_DATA_ROOT, LLM_WIKI_PROJECTS_ROOT, LLM_WIKI_PORT
#   LLM_WIKI_LEGACY_19828_ENABLED (set to "true" if the bundled MCP server should reach us)

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_DIR"

# Make cargo reachable (rustup install is not always on the default non-interactive PATH).
if ! command -v cargo >/dev/null 2>&1; then
  if [ -d "$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin" ]; then
    export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
  fi
fi

# Defaults — permanent locations under the user's home.
: "${LLM_WIKI_DATA_ROOT:=$HOME/Documents/llm-wiki-data}"
: "${LLM_WIKI_PROJECTS_ROOT:=$HOME/Documents/llm-wiki-projects}"
: "${LLM_WIKI_PORT:=8080}"
: "${LLM_WIKI_LEGACY_19828_ENABLED:=false}"

mkdir -p "$LLM_WIKI_DATA_ROOT" "$LLM_WIKI_PROJECTS_ROOT"

if [ ! -f "$LLM_WIKI_DATA_ROOT/users.toml" ]; then
  echo "=> No users.toml at $LLM_WIKI_DATA_ROOT — add one before starting." >&2
  echo "   Example:" >&2
  echo "     [users.alice]" >&2
  echo "     password_hash = \"\$argon2id\$...\"" >&2
  echo "   Generate a hash via: scripts/hash-password.sh <password>" >&2
  exit 2
fi

# Build (incremental) and start.
echo "=> Building llm-wiki-server (dev profile)..."
(cd src-tauri && cargo build --bin llm-wiki-server --quiet)

BIN="$REPO_DIR/src-tauri/target/debug/llm-wiki-server"

export LLM_WIKI_DATA_ROOT LLM_WIKI_PROJECTS_ROOT LLM_WIKI_PORT LLM_WIKI_LEGACY_19828_ENABLED

if [ "${1:-}" = "--background" ]; then
  # Detach + record pid.
  nohup "$BIN" >/tmp/llm-wiki-server.log 2>&1 &
  echo $! > /tmp/llm-wiki-server.pid
  sleep 1
  echo "=> Started in background, pid $(cat /tmp/llm-wiki-server.pid)"
  echo "   Logs: tail -f /tmp/llm-wiki-server.log"
  echo "   Stop: kill \$(cat /tmp/llm-wiki-server.pid)"
  echo "   URL:  http://localhost:$LLM_WIKI_PORT"
else
  echo "=> Listening on http://0.0.0.0:$LLM_WIKI_PORT"
  echo "   Data: $LLM_WIKI_DATA_ROOT"
  echo "   Projects: $LLM_WIKI_PROJECTS_ROOT"
  echo "   (Ctrl+C to stop)"
  exec "$BIN"
fi
