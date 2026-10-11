# AGENTS.md — Development, Architecture & Release Guide

This document is the single source of truth for AI agents (Antigravity, Codex, Claude Code, Cursor, etc.) and human contributors working on `ai-limit-ping` (`alp`).

---

## 1. Project Overview & Philosophy

`ai-limit-ping` (`alp`) is an ultra-lightweight (near 0% CPU & RAM) Rust daemon and CLI tool that keeps rolling 5-hour rate-limit windows continuously active for AI coding assistants:
- **Codex CLI** (`codex`)
- **Google Antigravity CLI** (`agy`)
- **Claude Code** (`claude`)

### Core Architectural Principles
1. **Anti-API-Spam**: Do not poll provider endpoints in tight loops. Cache exact reset timestamps locally in `state.json`. While a 5-hour window is active, sleep until the scheduled deadline.
2. **Zero Quota Waste**: Pings must use the smallest, cheapest supported model (`gpt-5.6-luna` for Codex, `gemini-3.8-flash-low` for Antigravity) with low/zero reasoning effort and headless/ephemeral execution so minimal quota is consumed.
3. **Sleep-Resilient**: Laptops sleep for hours. The scheduler uses coarse ticks and clock jump detection to safely recover on wake, wait for Wi-Fi re-association, and refresh provider status immediately.
4. **Zero-Configuration**: Automatically auto-detects installed tools and local OAuth credentials.

---

## 2. Fast Development Commands

```bash
# Check compilation
cargo check

# Run unit tests
cargo test

# Build optimized release binary
cargo build --release

# Format & Lint
cargo fmt --check
cargo clippy
```

---

## 3. Release & Deployment Checklist (MANDATORY)

Whenever modifying core logic or shipping bug fixes, **always** follow this exact sequence:

1. **Run tests**:
   ```bash
   cargo test
   ```
2. **Bump version** in `Cargo.toml`:
   ```toml
   [package]
   version = "0.1.X"
   ```
3. **Build release**:
   ```bash
   cargo build --release
   ```
4. **Install locally & codesign** (critical on macOS to prevent `launchd` permissions or kill signals):
   ```bash
   cp target/release/alp ~/.local/bin/alp
   codesign -s - -f ~/.local/bin/alp
   ```
5. **Restart daemon service**:
   ```bash
   alp daemon stop && sleep 1 && alp daemon start
   ```
6. **Verify status**:
   ```bash
   alp status --refresh
   ```
7. **Inspect daemon log**:
   ```bash
   tail -n 20 ~/.local/share/ai-limit-ping/daemon.stdout.log
   ```
8. **Commit & tag**:
   ```bash
   git add Cargo.toml Cargo.lock src/
   git commit -m "fix/feat: <description> (v0.1.X)"
   git tag v0.1.X
   git push origin main --tags
   ```

---

## 4. Provider Implementation Details & Gotchas

### Codex (`src/provider/codex.rs`)
- **Credentials**: Reads `~/.codex/auth.json`. Inspects JWT `exp` claim and automatically refreshes tokens against `https://auth.openai.com/oauth/token` if within 5 minutes of expiration.
- **Usage Endpoint**: `https://chatgpt.com/backend-api/wham/usage` with header `ChatGPT-Account-Id`.
- **CRITICAL GOTCHA — Idle Sliding Window**:
  When a user has **0% usage and no active session**, OpenAI's API dynamically returns:
  ```json
  "primary_window": {
    "used_percent": 0,
    "limit_window_seconds": 18000,
    "reset_after_seconds": 18000,
    "reset_at": <now + 18000>
  }
  ```
  **Rule**: Never assume `reset_at > now` means the window is active!
  A window is only active if `used_percent > 0.0` OR `(limit_window_seconds - reset_after_seconds) >= 10`. When idle, `is_active` must evaluate to `false` and `resets_at` to `None`.
- **Headless Ping**:
  ```bash
  codex exec --ephemeral --json --skip-git-repo-check --disable hooks --sandbox read-only -c model_reasoning_effort=low -m gpt-5.6-luna ok
  ```
  Must use `-m gpt-5.6-luna` as the default model to bypass unsupported or premium models (e.g. `gpt-6.1-sol`) configured in user settings.

### Antigravity (`src/provider/antigravity.rs`)
- **Binary**: `agy`. Uses `resolve_binary_path("agy")` across Homebrew and user bins.
- **Usage Command**: `agy -p /usage --output-format json --print-timeout 30s` executed in `~/.local/share/ai-limit-ping/sandbox`.
- **Reset Logic**: Reads `gemini-5h` bucket. `reset_time` is parsed as RFC3339 UTC.
- **Headless Ping**:
  ```bash
  agy -p ok --ephemeral --sandbox -m gemini-3.8-flash-low
  ```

### Claude Code (`src/provider/claude.rs`)
- **Credentials**: Extracts OAuth token from macOS Keychain (`Claude Code-credentials`) or `~/.claude/.credentials.json`.
- **Usage Endpoint**: `https://api.anthropic.com/api/oauth/usage?cedar_ember=1`.
- **Headless Ping**:
  ```bash
  claude -p "ok" --no-session-persistence --tools "" --output-format json
  ```
- **Error Backoff**: If ping fails (e.g. unrecognized model in user config), update `last_ping_at = Some(Utc::now())` to prevent tight 30s spin loops in the scheduler.

---

## 5. File System Layout

| Path | Purpose |
|---|---|
| `~/Library/Application Support/ai-limit-ping/config.json` | User configuration (intervals, model overrides) |
| `~/Library/Application Support/ai-limit-ping/state.json` | Rollover timestamps, percentage, ping status |
| `~/.local/share/ai-limit-ping/daemon.stdout.log` | Background daemon standard output log |
| `~/.local/share/ai-limit-ping/daemon.stderr.log` | Background daemon error log |
| `~/.local/share/ai-limit-ping/sandbox/` | Isolated empty directory for running headless pings |
| `~/Library/LaunchAgents/com.user.ai-limit-ping.plist` | macOS launchd daemon definition |

---

## 6. Sleep & Wake Detection Rules

- macOS puts the system into deep sleep on battery. User daemons are completely frozen during sleep.
- In `src/scheduler.rs`, track `last_tick_time`. If `Utc::now() - last_tick_time > (coarse_tick + 30s)`, a clock jump occurred (system woke from sleep).
- On wake:
  1. Call `wait_for_connectivity(Duration::from_secs(30))` to allow macOS Wi-Fi to re-associate.
  2. Force `need_query = true` across all providers to replace stale sleep timestamps with live provider state.
  3. All log output in `src/scheduler.rs` must include local timestamp prefixes `[YYYY-MM-DD HH:MM:SS]`.

---

## 7. Security & Privacy Invariants

- **Never Log Secrets**: Access tokens, refresh tokens, and cookies must never be printed to stdout, stderr, or log files.
- **Never Commit Sensitive Paths**: Keep sandbox paths, logs, and state files in OS data directories, outside the repository.
- **Check Git Status**: Always verify `git status` before committing to avoid staging unintended test artifacts.
