use anyhow::{anyhow, Result};
use colored::Colorize;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[allow(dead_code)]
const SERVICE_NAME: &str = "ai-limit-ping";
const MACOS_LABEL: &str = "com.user.ai-limit-ping";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonState {
    Active,
    Stopped,
    NotInstalled,
}

pub struct DaemonManager;

impl DaemonManager {
    fn current_exe() -> Result<PathBuf> {
        std::env::current_exe().map_err(|e| anyhow!("Failed to resolve current binary path: {}", e))
    }

    #[cfg(target_os = "macos")]
    fn launchd_plist_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", MACOS_LABEL))
    }

    #[cfg(target_os = "macos")]
    fn generate_launchd_plist(bin_path: &PathBuf) -> String {
        let log_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local/share/ai-limit-ping");
        let _ = fs::create_dir_all(&log_dir);

        let stdout_log = log_dir.join("daemon.stdout.log");
        let stderr_log = log_dir.join("daemon.stderr.log");

        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>watch</string>
    </array>
    <key>WorkingDirectory</key>
    <string>{workdir}</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
</dict>
</plist>
"#,
            label = MACOS_LABEL,
            bin = bin_path.display(),
            workdir = log_dir.display(),
            stdout = stdout_log.display(),
            stderr = stderr_log.display()
        )
    }

    #[cfg(target_os = "linux")]
    fn systemd_unit_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("systemd/user")
            .join(format!("{}.service", SERVICE_NAME))
    }

    #[cfg(target_os = "linux")]
    fn generate_systemd_unit(bin_path: &PathBuf) -> String {
        let work_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local/share/ai-limit-ping");
        let _ = fs::create_dir_all(&work_dir);

        format!(
            r#"[Unit]
Description=AI Limit Ping Daemon (alp)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory={workdir}
ExecStart={bin} watch
Restart=always
RestartSec=10

[Install]
WantedBy=default.target
"#,
            bin = bin_path.display(),
            workdir = work_dir.display()
        )
    }

    pub fn install() -> Result<()> {
        let exe = Self::current_exe()?;

        #[cfg(target_os = "macos")]
        {
            let plist_path = Self::launchd_plist_path();
            if let Some(parent) = plist_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let plist_content = Self::generate_launchd_plist(&exe);
            fs::write(&plist_path, plist_content)?;

            let _ = Command::new("launchctl")
                .args(["unload", plist_path.to_str().unwrap()])
                .output();

            let status = Command::new("launchctl")
                .args(["load", plist_path.to_str().unwrap()])
                .status()?;

            if status.success() {
                println!(
                    "{} Installed and started launchd service at {}",
                    "✓".green().bold(),
                    plist_path.display()
                );
            } else {
                return Err(anyhow!("Failed to load launchd service with launchctl"));
            }
        }

        #[cfg(target_os = "windows")]
        {
            let task_name = "AILimitPingDaemon";
            let cmd = format!("\"{}\" watch", exe.display());
            let status = Command::new("schtasks")
                .args([
                    "/create",
                    "/f",
                    "/tn",
                    task_name,
                    "/tr",
                    &cmd,
                    "/sc",
                    "onlogon",
                    "/rl",
                    "limited",
                ])
                .status()?;

            if status.success() {
                println!(
                    "{} Installed scheduled task '{}' on logon",
                    "✓".green().bold(),
                    task_name
                );
                let _ = Command::new("schtasks").args(["/run", "/tn", task_name]).status();
            } else {
                return Err(anyhow!("Failed to register scheduled task"));
            }
        }

        #[cfg(target_os = "linux")]
        {
            let unit_path = Self::systemd_unit_path();
            if let Some(parent) = unit_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let unit_content = Self::generate_systemd_unit(&exe);
            fs::write(&unit_path, unit_content)?;

            let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
            let status = Command::new("systemctl")
                .args(["--user", "enable", "--now", &format!("{}.service", SERVICE_NAME)])
                .status()?;

            if status.success() {
                println!(
                    "{} Installed and enabled systemd user service at {}",
                    "✓".green().bold(),
                    unit_path.display()
                );
            } else {
                return Err(anyhow!("Failed to enable systemd service via systemctl --user"));
            }
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            println!("{}", "Daemon installation is not supported on this platform.".yellow());
        }

        Ok(())
    }

    pub fn state() -> DaemonState {
        #[cfg(target_os = "macos")]
        {
            if let Ok(out) = Command::new("launchctl").args(["list"]).output() {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Some(line) = text.lines().find(|l| l.contains(MACOS_LABEL)) {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if let Some(pid_str) = parts.first() {
                        if let Ok(pid) = pid_str.parse::<i32>() {
                            if pid > 0 {
                                return DaemonState::Active;
                            }
                        }
                    }
                    return DaemonState::Stopped;
                }
            }
            if Self::launchd_plist_path().exists() {
                DaemonState::Stopped
            } else {
                DaemonState::NotInstalled
            }
        }

        #[cfg(target_os = "windows")]
        {
            if let Ok(out) = Command::new("schtasks")
                .args(["/query", "/tn", "AILimitPingDaemon", "/fo", "csv", "/nh"])
                .output()
            {
                if out.status.success() {
                    let text = String::from_utf8_lossy(&out.stdout);
                    if text.to_lowercase().contains("running") {
                        DaemonState::Active
                    } else {
                        DaemonState::Stopped
                    }
                } else {
                    DaemonState::NotInstalled
                }
            } else {
                DaemonState::NotInstalled
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Ok(out) = Command::new("systemctl")
                .args(["--user", "is-active", &format!("{}.service", SERVICE_NAME)])
                .output()
            {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if text == "active" {
                    DaemonState::Active
                } else if Self::systemd_unit_path().exists() {
                    DaemonState::Stopped
                } else {
                    DaemonState::NotInstalled
                }
            } else if Self::systemd_unit_path().exists() {
                DaemonState::Stopped
            } else {
                DaemonState::NotInstalled
            }
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            DaemonState::NotInstalled
        }
    }

    pub fn status_line() -> String {
        let backend = if cfg!(target_os = "macos") {
            "(launchd)"
        } else if cfg!(target_os = "windows") {
            "(Task Scheduler)"
        } else {
            "(systemd)"
        };
        match Self::state() {
            DaemonState::Active => format!("Daemon: {} {}", "active".green(), backend.dimmed()),
            DaemonState::Stopped => format!("Daemon: {} {}", "stopped".yellow(), backend.dimmed()),
            DaemonState::NotInstalled => format!("Daemon: {}", "not installed".dimmed()),
        }
    }

    pub fn uninstall() -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let plist_path = Self::launchd_plist_path();
            if plist_path.exists() {
                let _ = Command::new("launchctl")
                    .args(["unload", plist_path.to_str().unwrap()])
                    .output();
                fs::remove_file(&plist_path)?;
                println!("{} Uninstalled launchd daemon plist", "✓".green());
            } else {
                let _ = Command::new("launchctl")
                    .args(["remove", MACOS_LABEL])
                    .output();
                println!("No daemon plist found at {}", plist_path.display());
            }
        }

        #[cfg(target_os = "windows")]
        {
            let task_name = "AILimitPingDaemon";
            let _ = Command::new("schtasks")
                .args(["/delete", "/f", "/tn", task_name])
                .status();
            println!("{} Uninstalled Windows scheduled task", "✓".green());
        }

        #[cfg(target_os = "linux")]
        {
            let _ = Command::new("systemctl")
                .args(["--user", "disable", "--now", &format!("{}.service", SERVICE_NAME)])
                .status();
            let unit_path = Self::systemd_unit_path();
            if unit_path.exists() {
                fs::remove_file(&unit_path)?;
            }
            let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
            println!("{} Uninstalled systemd user service", "✓".green());
        }

        Ok(())
    }

    pub fn start() -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let plist_path = Self::launchd_plist_path();
            if plist_path.exists() {
                let _ = Command::new("launchctl")
                    .args(["load", plist_path.to_str().unwrap()])
                    .status();
                let status = Command::new("launchctl")
                    .args(["start", MACOS_LABEL])
                    .status()?;
                if status.success() {
                    println!("{} Started daemon {}", "✓".green(), MACOS_LABEL);
                }
            } else {
                return Err(anyhow!("Daemon plist not found at {}. Run `alp daemon install` first.", plist_path.display()));
            }
        }

        #[cfg(target_os = "windows")]
        {
            let _ = Command::new("schtasks")
                .args(["/run", "/tn", "AILimitPingDaemon"])
                .status()?;
            println!("{} Started Windows daemon task", "✓".green());
        }

        #[cfg(target_os = "linux")]
        {
            let status = Command::new("systemctl")
                .args(["--user", "start", &format!("{}.service", SERVICE_NAME)])
                .status()?;
            if status.success() {
                println!("{} Started systemd service {}", "✓".green(), SERVICE_NAME);
            }
        }

        Ok(())
    }

    pub fn stop() -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let plist_path = Self::launchd_plist_path();
            if plist_path.exists() {
                let _ = Command::new("launchctl")
                    .args(["unload", plist_path.to_str().unwrap()])
                    .status();
            } else {
                let _ = Command::new("launchctl")
                    .args(["stop", MACOS_LABEL])
                    .status();
            }
            println!("{} Stopped daemon {}", "✓".green(), MACOS_LABEL);
        }

        #[cfg(target_os = "windows")]
        {
            let _ = Command::new("schtasks")
                .args(["/end", "/tn", "AILimitPingDaemon"])
                .status()?;
            println!("{} Stopped Windows daemon task", "✓".green());
        }

        #[cfg(target_os = "linux")]
        {
            let status = Command::new("systemctl")
                .args(["--user", "stop", &format!("{}.service", SERVICE_NAME)])
                .status()?;
            if status.success() {
                println!("{} Stopped systemd service {}", "✓".green(), SERVICE_NAME);
            }
        }

        Ok(())
    }

    pub fn status() -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            match Self::state() {
                DaemonState::Active => {
                    let out = Command::new("launchctl")
                        .args(["list"])
                        .output()?;
                    let text = String::from_utf8_lossy(&out.stdout);
                    let line = text.lines().find(|l| l.contains(MACOS_LABEL)).unwrap_or("");
                    println!("{} Daemon is registered and active in launchd:\n{}", "✓".green().bold(), line);
                }
                DaemonState::Stopped => {
                    println!("{} Daemon plist is installed but service is stopped. Run `alp daemon start`.", "!".yellow());
                }
                DaemonState::NotInstalled => {
                    println!("{} Daemon is not registered in launchd. Run `alp daemon install`.", "!".yellow());
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            match Self::state() {
                DaemonState::Active => {
                    let out = Command::new("schtasks")
                        .args(["/query", "/tn", "AILimitPingDaemon"])
                        .output()?;
                    println!("{} Daemon task is active:\n{}", "✓".green().bold(), String::from_utf8_lossy(&out.stdout));
                }
                DaemonState::Stopped => {
                    println!("{} Daemon task is registered but stopped. Run `alp daemon start`.", "!".yellow());
                }
                DaemonState::NotInstalled => {
                    println!("{} Daemon task is not registered. Run `alp daemon install`.", "!".yellow());
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            match Self::state() {
                DaemonState::Active => {
                    println!("{} Daemon service is active and running in systemd.", "✓".green().bold());
                }
                DaemonState::Stopped => {
                    println!("{} Daemon service is installed but stopped. Run `alp daemon start`.", "!".yellow());
                }
                DaemonState::NotInstalled => {
                    println!("{} Daemon service is not installed. Run `alp daemon install`.", "!".yellow());
                }
            }
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            println!("{}", "Daemon management is not supported on this platform.".yellow());
        }

        Ok(())
    }
}
