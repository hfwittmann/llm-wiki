#!/usr/bin/env bash
# Print an argon2 hash for the given password, suitable for users.toml.
#
# Usage:
#   scripts/hash-password.sh <password>
#
# Example:
#   echo "[users.alice]" >> ~/Documents/llm-wiki-data/users.toml
#   echo "password_hash = \"$(scripts/hash-password.sh mypassword)\"" >> ~/Documents/llm-wiki-data/users.toml

set -euo pipefail

if [ $# -ne 1 ]; then
  echo "Usage: $0 <password>" >&2
  exit 1
fi

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_DIR"

if ! command -v cargo >/dev/null 2>&1; then
  if [ -d "$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin" ]; then
    export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
  fi
fi

# One-off binary: write, run, remove.
TMP_BIN="$REPO_DIR/src-tauri/src/bin/hash_password_oneshot.rs"
cleanup() { rm -f "$TMP_BIN"; }
trap cleanup EXIT

cat > "$TMP_BIN" <<'EOF'
use llm_wiki_lib::auth::users::hash_password;
fn main() {
    let pw = std::env::args().nth(1).expect("usage: hash_password_oneshot <password>");
    println!("{}", hash_password(&pw).unwrap());
}
EOF

(cd src-tauri && cargo run --release --quiet --bin hash_password_oneshot -- "$1" 2>/dev/null) | tail -1
