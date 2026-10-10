use anyhow::{anyhow, Result};
use colored::Colorize;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[allow(dead_code)]
const SERVICE_NAME: &str = "ai-limit-ping";
const MACOS_LABEL: &str = "com.user.ai-limit-ping";

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
        format!(
            r#"[Unit]
Description=AI Limit Ping Daemon (alp)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={bin} watch
Restart=always
RestartSec=10

[Install]
WantedBy=default.target
"#,
            bin = bin_path.display()
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
            let status = Command::new("launchctl")
                .args(["start", MACOS_LABEL])
                .status()?;
            if status.success() {
                println!("{} Started daemon {}", "✓".green(), MACOS_LABEL);
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
            let status = Command::new("launchctl")
                .args(["stop", MACOS_LABEL])
                .status()?;
            if status.success() {
                println!("{} Stopped daemon {}", "✓".green(), MACOS_LABEL);
            }
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
            let out = Command::new("launchctl")
                .args(["list"])
                .output()?;
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = text.lines().find(|l| l.contains(MACOS_LABEL)) {
                println!("{} Daemon is registered in launchd:\n{}", "✓".green().bold(), line);
            } else {
                println!("{} Daemon is not registered in launchd. Run `alp daemon install`.", "!".yellow());
            }
        }

        #[cfg(target_os = "windows")]
        {
            let out = Command::new("schtasks")
                .args(["/query", "/tn", "AILimitPingDaemon"])
                .output()?;
            if out.status.success() {
                println!("{} Daemon task is registered:\n{}", "✓".green().bold(), String::from_utf8_lossy(&out.stdout));
            } else {
                println!("{} Daemon task is not registered. Run `alp daemon install`.", "!".yellow());
            }
        }

        #[cfg(target_os = "linux")]
        {
            let status = Command::new("systemctl")
                .args(["--user", "status", &format!("{}.service", SERVICE_NAME)])
                .status()?;
            if !status.success() {
                println!("{} Daemon service is not active. Run `alp daemon install`.", "!".yellow());
            }
        }

        Ok(())
    }
}
