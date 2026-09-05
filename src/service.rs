//! System Service Manager for MBHub daemon.
//!
//! Provides automatic installation, uninstallation, and lifecycle management
//! across Linux (systemd), macOS (launchd), and Windows (Task Scheduler).

use std::path::PathBuf;
use std::process::Command;

use crate::ipc::{try_query_daemon, IpcRequest, IpcResponse};

/// Installs MBHub daemon as a system background service that auto-starts on login.
pub fn install() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("Failed to get current executable path: {}", e))?;
    let exe_str = exe.to_str().ok_or("Invalid UTF-8 in executable path")?;

    // 1. Mark terms accepted for background headless execution
    crate::db::set_meta("terms_accepted", "true");

    #[cfg(target_os = "linux")]
    {
        install_linux_systemd(exe_str)?;
    }

    #[cfg(target_os = "macos")]
    {
        install_macos_launchd(exe_str)?;
    }

    #[cfg(target_os = "windows")]
    {
        install_windows_task(exe_str)?;
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        return Err("Unsupported operating system for service management.".to_string());
    }

    // 2. Auto-configure MCP servers in Claude Desktop and Cursor (zero-friction)
    let _ = auto_configure_mcp();

    // 3. Create desktop launcher / shortcut
    let _ = create_desktop_shortcut(exe_str);

    println!("\n[MBHub] Zero-friction installation complete!");
    println!("- Background Daemon: ACTIVE (boots on system start)");
    println!("- MCP Integration:   CONFIGURED (Cursor & Claude Desktop)");
    println!("- Terminal TUI:      Ready ('mbhub')\n");

    Ok(())
}

/// Uninstalls and stops the background service.
pub fn uninstall() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        uninstall_linux_systemd()
    }

    #[cfg(target_os = "macos")]
    {
        uninstall_macos_launchd()
    }

    #[cfg(target_os = "windows")]
    {
        uninstall_windows_task()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err("Unsupported operating system for service management.".to_string())
    }
}

/// Checks the operational status of the service and P2P swarm.
pub fn status() {
    println!("Checking MBHub service status...\n");

    if let Some(resp) = try_query_daemon(&IpcRequest::Status) {
        if let IpcResponse::Status { running: _, peers, reserved_gb, records } = resp {
            println!("Status:          RUNNING");
            println!("P2P Swarm Peers: {} online", peers);
            println!("Memory Records:  {} cached", records);
            println!("Reserved Space:  {} GB", reserved_gb);
            println!("\nService is healthy and responding to queries.");
            return;
        }
    }

    println!("Status:          STOPPED (Daemon is not responding to IPC)");
    println!("\nTo start the service, run: mbhub service start");
    println!("To run in foreground, run: mbhub daemon");
}

/// Starts the installed service.
pub fn start() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let status = Command::new("systemctl")
            .args(["--user", "start", "mbhub"])
            .status()
            .map_err(|e| format!("Failed to execute systemctl: {}", e))?;
        if status.success() {
            println!("MBHub service started.");
            Ok(())
        } else {
            Err("Failed to start MBHub service via systemctl.".to_string())
        }
    }
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("launchctl")
            .args(["start", "dev.mbhub.daemon"])
            .status()
            .map_err(|e| format!("Failed to execute launchctl: {}", e))?;
        if status.success() {
            println!("MBHub service started.");
            Ok(())
        } else {
            Err("Failed to start MBHub service via launchctl.".to_string())
        }
    }
    #[cfg(target_os = "windows")]
    {
        let status = Command::new("schtasks")
            .args(["/Run", "/TN", "MBHubDaemon"])
            .status()
            .map_err(|e| format!("Failed to execute schtasks: {}", e))?;
        if status.success() {
            println!("MBHub task started.");
            Ok(())
        } else {
            Err("Failed to start MBHub task.".to_string())
        }
    }
}

/// Stops the running service.
pub fn stop() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let status = Command::new("systemctl")
            .args(["--user", "stop", "mbhub"])
            .status()
            .map_err(|e| format!("Failed to execute systemctl: {}", e))?;
        if status.success() {
            println!("MBHub service stopped.");
            Ok(())
        } else {
            Err("Failed to stop MBHub service via systemctl.".to_string())
        }
    }
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("launchctl")
            .args(["stop", "dev.mbhub.daemon"])
            .status()
            .map_err(|e| format!("Failed to execute launchctl: {}", e))?;
        if status.success() {
            println!("MBHub service stopped.");
            Ok(())
        } else {
            Err("Failed to stop MBHub service via launchctl.".to_string())
        }
    }
    #[cfg(target_os = "windows")]
    {
        let status = Command::new("schtasks")
            .args(["/End", "/TN", "MBHubDaemon"])
            .status()
            .map_err(|e| format!("Failed to execute schtasks: {}", e))?;
        if status.success() {
            println!("MBHub task stopped.");
            Ok(())
        } else {
            Err("Failed to stop MBHub task.".to_string())
        }
    }
}

#[cfg(target_os = "linux")]
fn install_linux_systemd(exe_path: &str) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "HOME environment variable not set")?;
    let unit_dir = PathBuf::from(home).join(".config").join("systemd").join("user");
    std::fs::create_dir_all(&unit_dir)
        .map_err(|e| format!("Failed to create systemd user directory: {}", e))?;

    let unit_file = unit_dir.join("mbhub.service");
    let content = format!(
        "[Unit]\n\
         Description=MBHub P2P Collective Memory Node\n\
         After=network-online.target\n\
         Wants=network-online.target\n\n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} daemon --accept-terms\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         UMask=0077\n\n\
         [Install]\n\
         WantedBy=default.target\n",
        exe_path
    );

    std::fs::write(&unit_file, content)
        .map_err(|e| format!("Failed to write unit file: {}", e))?;

    let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    let status = Command::new("systemctl")
        .args(["--user", "enable", "--now", "mbhub"])
        .status()
        .map_err(|e| format!("Failed to enable systemd service: {}", e))?;

    if status.success() {
        println!("Successfully installed and enabled MBHub systemd user service.");
        println!("Unit file: {}", unit_file.display());
        println!("The node is now active in the background and will start automatically on login.");
        Ok(())
    } else {
        Err("systemctl enable --now failed.".to_string())
    }
}

#[cfg(target_os = "linux")]
fn uninstall_linux_systemd() -> Result<(), String> {
    let _ = Command::new("systemctl").args(["--user", "stop", "mbhub"]).status();
    let _ = Command::new("systemctl").args(["--user", "disable", "mbhub"]).status();

    let home = std::env::var("HOME").map_err(|_| "HOME environment variable not set")?;
    let unit_file = PathBuf::from(home)
        .join(".config")
        .join("systemd")
        .join("user")
        .join("mbhub.service");

    if unit_file.exists() {
        let _ = std::fs::remove_file(unit_file);
    }

    let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    println!("MBHub service uninstalled.");
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_macos_launchd(exe_path: &str) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "HOME environment variable not set")?;
    let agents_dir = PathBuf::from(home).join("Library").join("LaunchAgents");
    std::fs::create_dir_all(&agents_dir)
        .map_err(|e| format!("Failed to create LaunchAgents directory: {}", e))?;

    let plist_file = agents_dir.join("dev.mbhub.daemon.plist");
    let content = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
             <key>Label</key>\n\
             <string>dev.mbhub.daemon</string>\n\
             <key>ProgramArguments</key>\n\
             <array>\n\
                 <string>{}</string>\n\
                 <string>daemon</string>\n\
                 <string>--accept-terms</string>\n\
             </array>\n\
             <key>RunAtLoad</key>\n\
             <true/>\n\
             <key>KeepAlive</key>\n\
             <true/>\n\
         </dict>\n\
         </plist>\n",
        exe_path
    );

    std::fs::write(&plist_file, content)
        .map_err(|e| format!("Failed to write plist file: {}", e))?;

    let status = Command::new("launchctl")
        .args(["load", "-w", plist_file.to_str().unwrap()])
        .status()
        .map_err(|e| format!("Failed to load launchd service: {}", e))?;

    if status.success() {
        println!("Successfully installed and loaded MBHub launchd service on macOS.");
        Ok(())
    } else {
        Err("launchctl load failed.".to_string())
    }
}

#[cfg(target_os = "macos")]
fn uninstall_macos_launchd() -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "HOME environment variable not set")?;
    let plist_file = PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join("dev.mbhub.daemon.plist");

    if plist_file.exists() {
        let _ = Command::new("launchctl")
            .args(["unload", "-w", plist_file.to_str().unwrap()])
            .status();
        let _ = std::fs::remove_file(plist_file);
    }

    println!("MBHub service uninstalled from launchd.");
    Ok(())
}

#[cfg(target_os = "windows")]
fn install_windows_task(exe_path: &str) -> Result<(), String> {
    let status = Command::new("schtasks")
        .args([
            "/Create",
            "/SC",
            "ONLOGON",
            "/TN",
            "MBHubDaemon",
            "/TR",
            &format!("\"{}\" daemon --accept-terms", exe_path),
            "/F",
        ])
        .status()
        .map_err(|e| format!("Failed to create Windows scheduled task: {}", e))?;

    if status.success() {
        let _ = Command::new("schtasks").args(["/Run", "/TN", "MBHubDaemon"]).status();
        println!("Successfully installed and started MBHub background task on Windows.");
        Ok(())
    } else {
        Err("schtasks /Create failed.".to_string())
    }
}

#[cfg(target_os = "windows")]
fn uninstall_windows_task() -> Result<(), String> {
    let _ = Command::new("schtasks").args(["/End", "/TN", "MBHubDaemon"]).status();
    let status = Command::new("schtasks")
        .args(["/Delete", "/TN", "MBHubDaemon", "/F"])
        .status()
        .map_err(|e| format!("Failed to delete Windows task: {}", e))?;

    if status.success() {
        println!("MBHub background task uninstalled.");
        Ok(())
    } else {
        Err("schtasks /Delete failed.".to_string())
    }
}

/// Automatically configures MBHub stdio MCP server for Claude Desktop and Cursor.
pub fn auto_configure_mcp() -> Result<(), String> {
    let mut configured_any = false;
    let mut targets: Vec<(PathBuf, &'static str)> = Vec::new();

    // 1. Claude Desktop config locations
    #[cfg(target_os = "linux")]
    if let Ok(home) = std::env::var("HOME") {
        targets.push((
            PathBuf::from(home).join(".config/Claude/claude_desktop_config.json"),
            "Claude Desktop (Linux)",
        ));
    }
    #[cfg(target_os = "macos")]
    if let Ok(home) = std::env::var("HOME") {
        targets.push((
            PathBuf::from(home).join("Library/Application Support/Claude/claude_desktop_config.json"),
            "Claude Desktop (macOS)",
        ));
    }
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        targets.push((
            PathBuf::from(appdata).join("Claude/claude_desktop_config.json"),
            "Claude Desktop (Windows)",
        ));
    }

    // 2. Cursor MCP config locations
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Ok(home) = std::env::var("HOME") {
        targets.push((
            PathBuf::from(home).join(".cursor/mcp.json"),
            "Cursor (User)",
        ));
    }
    #[cfg(target_os = "windows")]
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        targets.push((
            PathBuf::from(userprofile).join(".cursor/mcp.json"),
            "Cursor (User)",
        ));
    }

    // 3. Inject mbhub config into each target
    for (target, label) in targets {
        if inject_mcp_config(&target).is_ok() {
            println!("Auto-configured MCP server in {} ({})", label, target.display());
            configured_any = true;
        }
    }

    if configured_any {
        println!("MCP server auto-configuration active: Cursor and Claude can query MBHub.");
    }
    Ok(())
}

fn inject_mcp_config(file_path: &PathBuf) -> Result<(), String> {
    let mut root: serde_json::Value = if file_path.exists() {
        let content = std::fs::read_to_string(file_path)
            .map_err(|e| format!("Failed to read {}: {}", file_path.display(), e))?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if !root.is_object() {
        root = serde_json::json!({});
    }

    let servers = root.as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    if let Some(servers_obj) = servers.as_object_mut() {
        servers_obj.insert(
            "mbhub".to_string(),
            serde_json::json!({
                "command": "mbhub",
                "args": ["mcp", "--accept-terms"]
            }),
        );
    }

    let formatted = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize MCP JSON: {}", e))?;

    if let Some(parent) = file_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    std::fs::write(file_path, formatted)
        .map_err(|e| format!("Failed to write {}: {}", file_path.display(), e))?;

    Ok(())
}

fn create_desktop_shortcut(exe_path: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let apps_dir = PathBuf::from(&home).join(".local/share/applications");
            let _ = std::fs::create_dir_all(&apps_dir);
            let desktop_file = apps_dir.join("mbhub.desktop");
            let content = format!(
                "[Desktop Entry]\n\
                 Name=MBHub\n\
                 Comment=Sovereign P2P Collective AI Memory Layer\n\
                 Exec=sh -c '{} || $SHELL'\n\
                 Terminal=true\n\
                 Type=Application\n\
                 Categories=Development;Utility;\n\
                 Keywords=ai;p2p;memory;llm;mcp;\n",
                exe_path
            );
            let _ = std::fs::write(&desktop_file, content);
            println!("Created application launcher: {}", desktop_file.display());
        }
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "$ws = New-Object -ComObject WScript.Shell; \
             $s = $ws.CreateShortcut(\"$env:APPDATA\\Microsoft\\Windows\\Start Menu\\Programs\\MBHub.lnk\"); \
             $s.TargetPath = \"{}\"; \
             $s.Description = \"MBHub Sovereign P2P Memory\"; \
             $s.Save()",
            exe_path.replace('/', "\\")
        );
        let _ = Command::new("powershell").args(["-NoProfile", "-Command", &script]).status();
    }
    Ok(())
}
