# <img src="assets/logo.png" width="32" height="32" align="absmiddle" alt="ai-limit-ping logo" /> ai-limit-ping (`alp`)

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)
![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)

A high-efficiency, near-0% CPU and RAM background daemon and CLI that keeps **Codex**, **Antigravity (`agy`)**, and **Claude Code** 5-hour rate limit windows alive.

Whenever a 5-hour quota window resets, `alp` sends a tiny synthetic request through the official provider CLI to restart the 5-hour window immediately, preventing your quota cycle from drifting.

---

## Highlights

- **Near-0% CPU & Minimal RAM**: Native compiled Rust binary (< 3 MB stripped) that sleeps in OS kernel wait queues (`kqueue` on macOS, wait objects on Windows).
- **Anti-API-Spam Strategy**: Rather than constantly querying endpoints, `alp` caches exact reset timestamps locally (`state.json`). While a window is active, it sleeps until the deadline and **never spams provider APIs**.
- **Sleep & Wake Resilient**: When your laptop goes to sleep and wakes up past the reset deadline, `alp` immediately detects the clock jump.
- **Network-Aware**: When resuming from sleep, Wi-Fi can take a few seconds to connect. `alp` waits for network connectivity before attempting queries or pings instead of failing prematurely.
- **Official Ephemeral Triggers**: Uses official CLI non-interactive modes with sandbox flags, leaving zero conversation clutter in your history:
  - **Codex**: `codex exec --ephemeral --json --sandbox read-only ...`
  - **Antigravity**: `agy -p "hello" --output-format json --effort low`
  - **Claude Code**: `claude -p "ok" --no-session-persistence --tools ""`
- **Zero-Configuration Setup**: Works out of the box without any initial configuration. Intelligent defaults are automatically generated on first run.
- **Zero-Quota Usage Checks**: Reads usage from read-only endpoints without spending any user quota.

---

## Supported Providers

| Provider | Read 5h Limit (0 Quota) | Headless Ephemeral Ping | Auth Source |
|---|---|---|---|
| **Codex** | Direct GET `/wham/usage` | `codex exec --ephemeral --sandbox read-only` | `~/.codex/auth.json` (auto-refreshed via OpenAI OAuth) |
| **Antigravity (`agy`)** | Zero-turn CLI: `agy -p "/usage"` | `agy -p "hello" --output-format json --effort low` | `~/.gemini/antigravity-cli/antigravity-oauth-token` |
| **Claude Code** | Direct GET `/api/oauth/usage` | `claude -p "ok" --no-session-persistence --tools ""` | macOS Keychain (`Claude Code-credentials`) or `~/.claude/.credentials.json` |

---

## Quick Start

### Build & Install

```bash
# Clone and build
git clone https://github.com/kvn1351/ai-limit-ping.git
cd ai-limit-ping
cargo build --release

# Copy to your PATH (e.g. ~/.local/bin or /usr/local/bin)
cp target/release/alp ~/.local/bin/
```

### CLI Commands

```bash
# Show current limits and countdowns (from local fast cache)
alp status

# Force a live refresh from provider endpoints
alp status --refresh

# Test a dry-run ping (prints what command would run without spending quota)
alp ping --dry-run

# Trigger a manual ping right now
alp ping codex
alp ping antigravity
alp ping all

# Run foreground watcher with live countdowns
alp watch
```

---

## Background Daemon

You can run `alp` as an unattended background service managed directly by your operating system:

### macOS (`launchd`)

```bash
# Install and register as a macOS launchd user agent
alp daemon install

# Check daemon service status
alp daemon status

# Start / Stop service
alp daemon start
alp daemon stop

# Uninstall launchd service
alp daemon uninstall
```

Logs are saved to `~/.local/share/ai-limit-ping/daemon.stdout.log`.

### Linux (`systemd --user`)

```bash
# Install and enable as a systemd user service (no sudo required)
alp daemon install

# Check service status
alp daemon status

# Start / Stop service
alp daemon start
alp daemon stop

# Uninstall service
alp daemon uninstall
```

> **Tip for headless servers/VPS:** Run `loginctl enable-linger $USER` if you want the user service to continue running after SSH logout.

### Windows (Task Scheduler)

```cmd
alp daemon install
alp daemon status
alp daemon uninstall
```

---

## Configuration

> **Configuration is completely optional!** `alp` works out of the box with zero setup required. On its first run, a sensible default configuration is automatically generated.
>
> You only need to edit the configuration if you want to customize specific preferences, such as overriding which models are used for pings (e.g. specifying `codex_model` or `claude_model`), adjusting reset buffer margins, or enabling/disabling individual providers.

The configuration file is located at:
- macOS: `~/Library/Application Support/ai-limit-ping/config.json`
- Windows: `%APPDATA%\ai-limit-ping\config.json`
- Linux: `~/.config/ai-limit-ping/config.json`

Manage configuration from the CLI:
```bash
# View active configuration and its file path
alp config show

# Manually write/reset default configuration
alp config init
```

Default autogenerated configuration:
```json
{
  "reset_buffer_seconds": 5,
  "coarse_tick_seconds": 30,
  "enabled_providers": [
    "codex",
    "antigravity",
    "claude"
  ],
  "codex_model": null,
  "antigravity_model": "gemini-3.8-flash-low",
  "claude_model": null,
  "notify": true
}
```

---

## License

MIT License. See [LICENSE](LICENSE) for details.
