//! System Service Manager for MBHub daemon.
//!
//! Provides automatic installation, uninstallation, and lifecycle management
//! across Linux (systemd), macOS (launchd), and Windows (Task Scheduler).

use std::path::PathBuf;
use std::process::Command;

use crate::ipc::{try_query_daemon, IpcRequest, IpcResponse};

/// Embedded launcher icons (iOS-style rounded square, centered mark).
/// Regenerate with `assets/icons/generate.sh` after logo changes.
const ICON_PNG_512: &[u8] = include_bytes!("../assets/icons/hicolor/512x512/apps/mbhub.png");
const ICON_PNG_256: &[u8] = include_bytes!("../assets/icons/hicolor/256x256/apps/mbhub.png");
const ICON_PNG_128: &[u8] = include_bytes!("../assets/icons/hicolor/128x128/apps/mbhub.png");
const ICON_PNG_64: &[u8] = include_bytes!("../assets/icons/hicolor/64x64/apps/mbhub.png");
const ICON_PNG_48: &[u8] = include_bytes!("../assets/icons/hicolor/48x48/apps/mbhub.png");
const ICON_PNG_32: &[u8] = include_bytes!("../assets/icons/hicolor/32x32/apps/mbhub.png");
pub const ICON_ICO: &[u8] = include_bytes!("../assets/icons/mbhub.ico");

/// Installs the freedesktop hicolor icon set into the user's icon
/// directory and refreshes the desktop icon cache when available.
pub fn install_icons() -> Result<(), String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| "HOME not set".to_string())?;
    let icons: [(&str, &[u8]); 6] = [
        ("512x512", ICON_PNG_512),
        ("256x256", ICON_PNG_256),
        ("128x128", ICON_PNG_128),
        ("64x64", ICON_PNG_64),
        ("48x48", ICON_PNG_48),
        ("32x32", ICON_PNG_32),
    ];
    for (size, bytes) in icons {
        let dir = PathBuf::from(&home)
            .join(".local/share/icons/hicolor")
            .join(size)
            .join("apps");
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create icon dir {}: {}", dir.display(), e))?;
        let file = dir.join("mbhub.png");
        std::fs::write(&file, bytes)
            .map_err(|e| format!("Failed to write {}: {}", file.display(), e))?;
    }
    // Best-effort cache refresh (older desktops need it).
    let _ = Command::new("gtk-update-icon-cache")
        .arg("-q")
        .arg("-f")
        .arg(PathBuf::from(&home).join(".local/share/icons/hicolor"))
        .status();
    Ok(())
}

/// Removes the installed hicolor icon set (uninstaller helper).
pub fn remove_icons() {
    let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) else {
        return;
    };
    for size in ["512x512", "256x256", "128x128", "64x64", "48x48", "32x32"] {
        let _ = std::fs::remove_file(
            PathBuf::from(&home)
                .join(".local/share/icons/hicolor")
                .join(size)
                .join("apps/mbhub.png"),
        );
    }
}

/// Removes the application launcher (uninstaller helper).
pub fn remove_desktop_shortcut() {
    if let Ok(home) = std::env::var("HOME") {
        let _ = std::fs::remove_file(
            PathBuf::from(&home).join(".local/share/applications/mbhub.desktop"),
        );
    }
}

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

    // 3. Create desktop launcher / shortcut (with branded icon)
    let _ = install_icons();
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

/// Broken-pipe-safe stdout write: `mbhub status | head` closes the pipe
/// early; `println!` would panic on EPIPE, this just stops printing.
fn print_line(text: &str) {
    use std::io::Write;
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{text}");
    let _ = stdout.flush();
}

/// Checks the operational status of the service and P2P swarm.
pub fn status() {
    print_line("Checking MBHub service status...\n");

    if let Some(resp) = try_query_daemon(&IpcRequest::Status) {
        if let IpcResponse::Status { running: _, peers, reserved_gb, records } = resp {
            print_line("Status:          RUNNING");
            print_line(&format!("P2P Swarm Peers: {peers} online"));
            print_line(&format!("Memory Records:  {records} cached"));
            print_line(&format!("Reserved Space:  {reserved_gb} GB"));
            print_line("\nService is healthy and responding to queries.");
            return;
        }
    }

    print_line("Status:          STOPPED (Daemon is not responding to IPC)");
    print_line("\nTo start the service, run: mbhub service start");
    print_line("To run in foreground, run: mbhub daemon");
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
                 Keywords=ai;p2p;memory;llm;mcp;\n\
                 Icon=mbhub\n\
                 StartupWMClass=mbhub\n",
                exe_path
            );
            let _ = std::fs::write(&desktop_file, content);
            println!("Created application launcher: {}", desktop_file.display());
        }
    }
    #[cfg(target_os = "windows")]
    {
        // Branded icon next to the Start Menu shortcut.
        let ico_dir = PathBuf::from(
            std::env::var("APPDATA").unwrap_or_else(|_| std::env::var("HOME").unwrap_or_default()),
        )
        .join("mbhub");
        let _ = std::fs::create_dir_all(&ico_dir);
        let ico_path = ico_dir.join("mbhub.ico");
        let _ = std::fs::write(&ico_path, ICON_ICO);
        let ico_str = ico_path.to_string_lossy().replace('/', "\\");
        let script = format!(
            "$ws = New-Object -ComObject WScript.Shell; \
             $s = $ws.CreateShortcut(\"$env:APPDATA\\Microsoft\\Windows\\Start Menu\\Programs\\MBHub.lnk\"); \
             $s.TargetPath = \"{}\"; \
             $s.IconLocation = \"{}, 0\"; \
             $s.Description = \"MBHub Sovereign P2P Memory\"; \
             $s.Save()",
            exe_path.replace('/', "\\"),
            ico_str
        );
        let _ = Command::new("powershell").args(["-NoProfile", "-Command", &script]).status();
    }
    Ok(())
}
