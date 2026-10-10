use anyhow::{anyhow, Result};
use colored::Colorize;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

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

            // Unload if already loaded
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

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            println!("{}", "Daemon installation is currently supported on macOS and Windows.".yellow());
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

        Ok(())
    }
}
