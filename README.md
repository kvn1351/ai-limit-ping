# <img src="assets/logo.png" width="32" height="32" align="absmiddle" alt="ai-limit-ping logo" /> ai-limit-ping (`alp`)

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)
![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)

A high-efficiency, near-0% CPU and RAM background daemon and CLI that perpetually claims **Codex**, **Antigravity**, and **Claude Code** 5-hour rate limit windows in the background.

Many AI subscriptions, especially at lower tiers, don't run on fixed global reset schedules; their 5-hour countdown **only starts ticking when you send a prompt**. `alp` continuously claims new windows by sending a tiny prompt via the official harness CLI tools the moment each window expires, so whenever you sit down to code, you step into an already active window and are statistically far closer to your next quota replenishment rather than facing a full 5-hour lockout.

### Why `alp`?

- **Without `alp`**: Starting a session after being idle starts your 5 hour clock from zero. If you burn through your quota during a coding sprint, you're stuck waiting the full 5 hours for the next reset.
- **With `alp`**: Windows are claimed around the clock. When you sit down to work, an active window is already ticking; meaning your next quota replenishment is statistically much closer :) .
---

## Highlights

- **Near-0% CPU & Minimal RAM**: Native compiled Rust binary (< 3 MB stripped) that sleeps in OS kernel wait queues (`kqueue` on macOS, wait objects on Windows).
- **Anti-API-Spam Strategy**: Rather than constantly querying endpoints, `alp` caches exact reset timestamps locally (`state.json`). While a window is active, it sleeps until the deadline and **never spams provider APIs**.
- **Sleep & Wake Resilient**: When your laptop goes to sleep and wakes up past the reset deadline, `alp` immediately detects the clock jump.
- **Network-Aware**: When resuming from sleep, Wi-Fi can take a few seconds to connect. `alp` waits for network connectivity before attempting queries or pings instead of failing prematurely.
- **Official Headless Triggers**: Uses official CLI non-interactive modes with sandbox flags, leaving zero conversation clutter in your history:
  - **Codex**: `codex exec --ephemeral --json --sandbox read-only ...`
  - **Antigravity**: `agy -p "ok" --sandbox --output-format json --effort low --model gemini-3.8-flash-low`
  - **Claude Code**: `claude -p "ok" --no-session-persistence --tools ""`
- **Zero-Configuration Setup**: Works out of the box without any initial configuration. Intelligent defaults are automatically generated on first run.
- **Zero-Quota Usage Checks**: Reads usage from read-only endpoints without spending any user quota.
- **Banked Codex Reset Reminders**: Desktop alerts at 24 hours, 3 hours, and 1 hour before earned reset credits expire, with restart-safe catch-up.

---

## Supported Providers

> [!IMPORTANT]
> **Prerequisites:** The official CLI harness (`codex`, `agy`, or `claude`) for any provider you wish to use must be installed and logged in on your system. `alp` dispatches prompt requests through these official harnesses, so you only need the CLI tools installed for the providers you want to keep active.

| Provider | CLI Tool | Read 5h Limit (0 Quota) | Headless Ping | Auth Source |
|---|---|---|---|---|
| **Codex** | `codex` | Direct GET `/wham/usage` | `codex exec --ephemeral --sandbox read-only` | `~/.codex/auth.json` (auto-refreshed via OpenAI OAuth) |
| **Antigravity (`agy`)** | `agy` | Zero-turn CLI: `agy -p "/usage"` | `agy -p "ok" --sandbox --output-format json --effort low --model gemini-3.8-flash-low` | `~/.gemini/antigravity-cli/antigravity-oauth-token` |
| **Claude Code** | `claude` | Direct GET `/api/oauth/usage` | `claude -p "ok" --no-session-persistence --tools ""` | macOS Keychain (`Claude Code-credentials`) or `~/.claude/.credentials.json` |

---

## Installation & Setup

### 1. Install `alp`

**One-line installer (macOS & Linux):**
```bash
curl -fsSL https://raw.githubusercontent.com/kvn1351/ai-limit-ping/main/install.sh | sh
```

**One-line installer (Windows PowerShell):**
```powershell
irm https://raw.githubusercontent.com/kvn1351/ai-limit-ping/main/install.ps1 | iex
```

<details>
<summary>Or build from source via Cargo</summary>

```bash
git clone https://github.com/kvn1351/ai-limit-ping.git
cd ai-limit-ping
cargo build --release
cp target/release/alp ~/.local/bin/
```
</details>

### 2. Enable Background Daemon

`alp` runs unattended in the background across reboots using your operating system's native service manager (macOS `launchd`, Linux `systemd --user`, or Windows Task Scheduler).

The commands are identical on all platforms:

```bash
# Install and start background daemon
alp daemon install

# Check daemon service status
alp daemon status

# Start / Stop service
alp daemon start
alp daemon stop

# Uninstall daemon service
alp daemon uninstall
```

> **Platform Details:**
> - **macOS**: Managed via `launchd` user agent (`~/Library/LaunchAgents/com.user.ai-limit-ping.plist`).
> - **Linux**: Managed via `systemd --user` (`~/.config/systemd/user/ai-limit-ping.service`, no root/sudo needed). On headless servers, run `loginctl enable-linger $USER` to persist across SSH disconnects.
> - **Windows**: Managed via Task Scheduler task `AILimitPingDaemon`.
> - **Logs**: Written to `~/.local/share/ai-limit-ping/daemon.stdout.log`.

### 3. Uninstall & Complete Removal

- **Remove the background daemon only:**
  ```bash
  alp daemon uninstall
  ```
  *(Unregisters the background service, preserving your CLI binary, config file, and logs).*

- **Full clean removal (binary, config & logs):**
  - **macOS:**
    ```bash
    alp daemon uninstall
    rm -f ~/.local/bin/alp
    rm -rf ~/.local/share/ai-limit-ping
    rm -rf ~/Library/Application\ Support/ai-limit-ping
    ```
  - **Linux:**
    ```bash
    alp daemon uninstall
    rm -f ~/.local/bin/alp
    rm -rf ~/.local/share/ai-limit-ping
    rm -rf ~/.config/ai-limit-ping
    ```
  - **Windows (PowerShell):**
    ```powershell
    alp daemon uninstall
    Remove-Item -Force "$HOME\.local\bin\alp.exe"
    Remove-Item -Recurse -Force "$env:APPDATA\ai-limit-ping", "$env:LOCALAPPDATA\ai-limit-ping"
    ```

---

## CLI Commands

```bash
# Show current limits and countdowns (from local fast cache)
alp status

# Force a live refresh from provider endpoints
alp status --refresh

# List available banked Codex resets with live time remaining and expiry dates
alp resets

# Test a dry-run ping (prints what command would run without spending quota)
alp ping --dry-run

# Trigger a manual ping right now
alp ping codex
alp ping antigravity
alp ping all

# Run foreground watcher with live countdowns
alp watch

# Update alp to the latest release from GitHub
alp update
```

---

## Banked Codex Reset Reminders

With Codex enabled and `"notify": true`, the daemon (or `alp watch`) sends expiry
reminders for each available banked reset at **24h, 3h, and 1h remaining**. These
are reminders to use earned reset credits, not the normal five-hour window timer.
No credits are redeemed automatically.

Run **`alp resets`** to fetch the signed-in Codex account's available banked resets.
The list is ordered by soonest expiry and shows a live countdown plus the local
expiry date and UTC offset. Non-expiring credits appear last; expired credits
are omitted. This read-only command works even when notifications are disabled
and does not redeem credits or change reminder delivery history.

- Startup and wake-up refresh the inventory. If multiple thresholds were missed,
  only the most urgent reminder is sent: starting with 50 minutes remaining sends
  one “expires in 50 minutes” alert, not three overdue alerts.
- Each notification shows the actual time remaining. Delivery history is saved
  per account and credit in `state.json`, preventing duplicates after restarts.
- Inventory is checked hourly and again before a due reminder, not on every
  scheduler tick. Failed checks or notification commands are retried after five
  minutes; redeemed, expired, and non-expiring credits do not generate reminders.
- `alp watch --dry-run` logs due reminders without delivering them or recording
  them as delivered. Set `"notify": false` to disable notifications and these
  inventory checks.

The Mac must be awake and logged in to deliver notifications. macOS notification
permissions and Focus settings can hide banners; `alp` catches up when it next runs.

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
