# Phase 2 smoke test — manual verification runbook

After Phase 2 lands, this runbook verifies end-to-end behavior of the new `llm-wiki-server` binary.

## Prereqs

- macOS with Rust toolchain at `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/`.
- Project at `$REPO`.
- Release binary built: `cd src-tauri && cargo build --release --bin llm-wiki-server` (~6 min first time).

## Important — port 19828 conflict with the existing Tauri desktop app

The Tauri `llm-wiki` desktop app binds `127.0.0.1:19828` for its legacy agent-facing HTTP API. The new `llm-wiki-server` also wants `19828` for back-compat with the same agent integrations. **They can't both run at once on that port.** Two choices when smoke-testing while the desktop app is open:

- Quit the desktop app first, OR
- Run the smoke test with `LLM_WIKI_LEGACY_19828_ENABLED=false` (this is what the recipe below does).

In a real deployment, only one of the two will be running on a given host, so the conflict doesn't persist.

## Steps

```bash
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
SMOKE=$(mktemp -d)
mkdir -p "$SMOKE/data" "$SMOKE/projects"

# (1) Generate an argon2 hash via a temporary one-off binary.
#     This helper isn't checked in — it's recreated here when needed.
cat > $REPO/src-tauri/src/bin/hash_password_oneshot.rs <<'EOF'
use llm_wiki_lib::auth::users::hash_password;
fn main() {
    let pw = std::env::args().nth(1).expect("usage: hash_password_oneshot <password>");
    println!("{}", hash_password(&pw).unwrap());
}
EOF
HASH=$(cd $REPO/src-tauri \
  && cargo run --release --bin hash_password_oneshot -- demo-password 2>/dev/null | tail -1)
rm $REPO/src-tauri/src/bin/hash_password_oneshot.rs

# (2) Populate users.toml
cat > "$SMOKE/data/users.toml" <<EOF
[users.alice]
password_hash = "$HASH"
EOF

# (3) Start the server (legacy 19828 disabled to avoid Tauri-app port conflict)
LLM_WIKI_DATA_ROOT="$SMOKE/data" LLM_WIKI_PROJECTS_ROOT="$SMOKE/projects" \
  LLM_WIKI_LEGACY_19828_ENABLED=false \
  $REPO/src-tauri/target/release/llm-wiki-server \
  > "$SMOKE/server.log" 2>&1 &
SERVER_PID=$!
sleep 2

# (a) whoami without cookie → 401 UNAUTHENTICATED
curl -s -w 'HTTP %{http_code}\n' http://localhost:8080/api/v1/auth/whoami

# (b) login wrong password → 401 INVALID_CREDENTIALS
curl -s -w 'HTTP %{http_code}\n' -X POST -H 'content-type: application/json' \
  -d '{"username":"alice","password":"WRONG"}' \
  http://localhost:8080/api/v1/auth/login

# (c) login right password → 200 + Set-Cookie (HttpOnly; SameSite=Lax; Max-Age=2592000)
curl -s -c "$SMOKE/cookies.txt" -D - -w 'HTTP %{http_code}\n' \
  -X POST -H 'content-type: application/json' \
  -d '{"username":"alice","password":"demo-password"}' \
  http://localhost:8080/api/v1/auth/login

# (d) whoami with cookie → 200 + {user_id, username, recently_opened}
curl -s -b "$SMOKE/cookies.txt" -w 'HTTP %{http_code}\n' http://localhost:8080/api/v1/auth/whoami

# (e) SPA fallback → 200 + placeholder HTML
curl -s -w 'HTTP %{http_code}\n' http://localhost:8080/some/spa/route | head -3

# (f) logout → 204
curl -s -b "$SMOKE/cookies.txt" -w 'HTTP %{http_code}\n' -X POST http://localhost:8080/api/v1/auth/logout

# (g) whoami after logout → 401
curl -s -b "$SMOKE/cookies.txt" -w 'HTTP %{http_code}\n' http://localhost:8080/api/v1/auth/whoami

kill $SERVER_PID
```

## Expected outcomes

| Step | Expected HTTP | Expected body |
|---|---|---|
| (a) | 401 | `{"error":{"code":"UNAUTHENTICATED","message":"authentication required"}}` |
| (b) | 401 | `{"error":{"code":"INVALID_CREDENTIALS","message":"invalid username or password"}}` |
| (c) | 200 | `{"user_id":"alice","username":"alice"}` + `Set-Cookie: llm_wiki_session=<base64url>; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000` |
| (d) | 200 | `{"recently_opened":[],"user_id":"alice","username":"alice"}` |
| (e) | 200 | starts with `<!DOCTYPE html>` (the rust-embed placeholder index.html) |
| (f) | 204 | (empty body) |
| (g) | 401 | `{"error":{"code":"UNAUTHENTICATED","message":"authentication required"}}` |

## Last verified

2026-06-14 — all 7 steps green; commit `e1c9494`.
