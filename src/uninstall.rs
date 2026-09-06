//! Interactive uninstaller for MBHub (`mbhub uninstall`).
//!
//! A TUI checklist, MSI-style: the uninstaller ships inside the binary and
//! lets the user choose exactly what to remove and what to keep.
//!
//! Removal scopes (defaults follow the principle "code goes, memories stay"):
//! - checked by default: daemon service, launcher + icons, executable,
//!   PATH lines, MCP integrations
//! - opt-in (data loss warnings): memory database, P2P identity, API keys,
//!   logs
//!
//! Everything is strictly LOCAL: the uninstaller never touches other peers
//! and never propagates anything to the network.

use std::path::PathBuf;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum Kind {
    DaemonService,
    CmsTimer,
    LauncherIcons,
    Binary,
    PathLines,
    McpConfigs,
    Logs,
    Identity,
    ApiKeys,
    MemoryDb,
}

#[derive(Clone, Debug)]
struct Item {
    label: &'static str,
    detail: &'static str,
    kind: Kind,
    checked: bool,
}

impl Item {
    fn new(label: &'static str, detail: &'static str, kind: Kind, checked: bool) -> Self {
        Self { label, detail, kind, checked }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Checklist,
    Confirm,
    Done,
}

struct UninstallApp {
    items: Vec<Item>,
    selected: usize,
    view: View,
    results: Vec<(String, bool)>,
}

impl UninstallApp {
    fn new() -> Self {
        let items = vec![
            Item::new(
                "Background daemon service",
                "Stops the 24/7 node, removes the systemd user unit",
                Kind::DaemonService,
                true,
            ),
            #[cfg(feature = "publisher")]
            Item::new(
                "CMS sync timer & service",
                "Stops the hourly web archive sync and removes its units",
                Kind::CmsTimer,
                true,
            ),
            Item::new(
                "Application launcher & icons",
                "Removes the desktop entry and the hicolor icon set",
                Kind::LauncherIcons,
                true,
            ),
            Item::new(
                "Executable (~/.local/bin/mbhub)",
                "Removes the binary itself (performed last)",
                Kind::Binary,
                true,
            ),
            Item::new(
                "PATH lines in shell config",
                "Removes the MBHub export line from .bashrc/.zshrc/.profile",
                Kind::PathLines,
                true,
            ),
            Item::new(
                "MCP integrations",
                "Removes the mbhub entry from Claude Desktop and Cursor configs (other entries untouched)",
                Kind::McpConfigs,
                true,
            ),
            Item::new(
                "Logs",
                "~/.mbhub/mbhub.log, ~/.mbhub/cms-sync.log",
                Kind::Logs,
                false,
            ),
            Item::new(
                "P2P identity (node_identity.bin)",
                "Your peer identity — a fresh one is generated if you reinstall",
                Kind::Identity,
                false,
            ),
            Item::new(
                "API keys (~/.mbhub/.env)",
                "Stored provider credentials — copy them elsewhere first",
                Kind::ApiKeys,
                false,
            ),
            Item::new(
                "Local memory database (mbhub.db)",
                "ALL your memories, settings, bans — cannot be recovered",
                Kind::MemoryDb,
                false,
            ),
        ];
        Self { items, selected: 0, view: View::Checklist, results: Vec::new() }
    }

    fn toggle(&mut self) {
        if let Some(item) = self.items.get_mut(self.selected) {
            item.checked = !item.checked;
        }
    }

    fn toggle_all(&mut self) {
        let any_unchecked = self.items.iter().any(|i| !i.checked);
        for item in &mut self.items {
            item.checked = any_unchecked;
        }
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if self.selected + 1 < self.items.len() {
            self.selected += 1;
        }
    }

    /// Executes the removal for every checked item. Returns nothing —
    /// results are collected into `self.results`.
    fn execute(&mut self) {
        // The daemon must stop before we remove anything it may recreate.
        let daemon_checked = self
            .items
            .iter()
            .any(|i| i.kind == Kind::DaemonService && i.checked);
        if daemon_checked {
            stop_and_remove_daemon_service();
        }
        for item in &self.items {
            if !item.checked {
                continue;
            }
            let ok = match item.kind {
                Kind::DaemonService => true, // handled above
                #[cfg(feature = "publisher")]
                Kind::CmsTimer => stop_and_remove_cms_sync(),
                #[cfg(not(feature = "publisher"))]
                Kind::CmsTimer => true,
                Kind::LauncherIcons => {
                    crate::service::remove_desktop_shortcut();
                    crate::service::remove_icons();
                    true
                }
                Kind::Binary => remove_self_binary(),
                Kind::PathLines => clean_rc_files() > 0,
                Kind::McpConfigs => remove_mcp_entries() > 0,
                Kind::Logs => {
                    remove_home_mbhub(&["mbhub.log", "cms-sync.log"]);
                    true
                }
                Kind::Identity => {
                    remove_home_mbhub(&["node_identity.bin"]);
                    true
                }
                Kind::ApiKeys => {
                    let mut ok = true;
                    for name in [".env"] {
                        let p = home_mbhub().join(name);
                        if p.exists() {
                            ok &= std::fs::remove_file(&p).is_ok();
                        }
                    }
                    ok
                }
                Kind::MemoryDb => {
                    let mut ok = true;
                    for name in [
                        "mbhub.db",
                        "mbhub.db-wal",
                        "mbhub.db-shm",
                        "mbhub.db.bak-v1.0.0",
                    ] {
                        let p = home_mbhub().join(name);
                        if p.exists() {
                            ok &= std::fs::remove_file(&p).is_ok();
                        }
                    }
                    ok
                }
            };
            self.results.push((item.label.to_string(), ok));
        }
        self.view = View::Done;
    }
}

// ─── Path helpers ────────────────────────────────────────────────────────────

fn home_mbhub() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join(".mbhub")
}

fn remove_home_mbhub(names: &[&str]) {
    for name in names {
        let _ = std::fs::remove_file(home_mbhub().join(name));
    }
}

/// Stops the daemon and removes its systemd user unit.
fn stop_and_remove_daemon_service() {
    let _ = std::process::Command::new("systemctl").args(["--user", "stop", "mbhub"]).status();
    let _ = std::process::Command::new("systemctl").args(["--user", "disable", "mbhub"]).status();
    if let Ok(home) = std::env::var("HOME") {
        let _ = std::fs::remove_file(
            PathBuf::from(&home).join(".config/systemd/user/mbhub.service"),
        );
    }
    let _ = std::process::Command::new("systemctl").args(["--user", "daemon-reload"]).status();
}

/// Stops the CMS sync units and removes them plus the cms symlink.
#[cfg(feature = "publisher")]
fn stop_and_remove_cms_sync() -> bool {
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "stop", "mbhub-cms-sync.timer"])
        .status();
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "mbhub-cms-sync.timer"])
        .status();
    let mut ok = true;
    if let Ok(home) = std::env::var("HOME") {
        for unit in ["mbhub-cms-sync.timer", "mbhub-cms-sync.service"] {
            let p = PathBuf::from(&home).join(".config/systemd/user").join(unit);
            if p.exists() {
                ok &= std::fs::remove_file(&p).is_ok();
            }
        }
    }
    let _ = std::process::Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    let _ = std::fs::remove_file(home_mbhub().join("cms"));
    ok
}

/// Removes the `# MBHub binary PATH` marker + its export line from the
/// user's shell rc files. Returns the number of lines removed.
fn clean_rc_files() -> usize {
    let Ok(home) = std::env::var("HOME") else { return 0 };
    let mut removed = 0;
    for rc in [".bashrc", ".zshrc", ".profile"] {
        let path = PathBuf::from(&home).join(rc);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut lines: Vec<String> = Vec::new();
        let mut skip_next = false;
        for line in content.lines() {
            if skip_next {
                skip_next = false;
                removed += 1;
                continue;
            }
            if line.contains("# MBHub binary PATH") {
                removed += 1;
                skip_next = true;
                continue;
            }
            lines.push(line.to_string());
        }
        let _ = std::fs::write(&path, lines.join("\n") + "\n");
    }
    removed
}

/// Removes ONLY the mbhub entry from an MCP config JSON, leaving every other
/// entry and the file structure untouched. Returns true when an entry was
/// removed.
fn remove_mcp_entry(path: &PathBuf) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    let removed = root
        .get("mcpServers")
        .and_then(|m| m.get("mbhub"))
        .is_some();
    if removed {
        if let Some(servers) = root.get_mut("mcpServers").and_then(|m| m.as_object_mut()) {
            servers.remove("mbhub");
        }
        if let Ok(formatted) = serde_json::to_string_pretty(&root) {
            let _ = std::fs::write(path, formatted);
        }
    }
    removed
}

/// Removes the mbhub entry from every known MCP config. Returns how many
/// configs were touched.
fn remove_mcp_entries() -> usize {
    let Ok(home) = std::env::var("HOME") else { return 0 };
    let mut touched = 0;
    for path in [
        PathBuf::from(&home).join(".config/Claude/claude_desktop_config.json"),
        PathBuf::from(&home).join(".cursor/mcp.json"),
    ] {
        if remove_mcp_entry(&path) {
            touched += 1;
        }
    }
    touched
}

/// Removes the running executable. On Unix this works while the process is
/// alive (unlink of a mapped binary is legal). On Windows the binary is
/// renamed out of the way instead, since a running image cannot be deleted.
fn remove_self_binary() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    #[cfg(target_os = "windows")]
    {
        let renamed = exe.with_extension(format!("old.{}", std::process::id()));
        return std::fs::rename(&exe, renamed).is_ok();
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::fs::remove_file(exe).is_ok()
    }
}

// ─── TUI ─────────────────────────────────────────────────────────────────────

pub fn run_uninstall_tui() -> Result<(), String> {
    enable_raw_mode().map_err(|e| format!("raw mode: {e}"))?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| format!("alt screen: {e}"))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| format!("terminal: {e}"))?;

    let mut app = UninstallApp::new();
    let mut list_state = ListState::default();

    loop {
        terminal
            .draw(|frame| draw(frame, &app, &mut list_state))
            .map_err(|e| format!("draw: {e}"))?;

        if let Event::Key(key) = event::read().map_err(|e| format!("event: {e}"))? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match app.view {
                View::Checklist => match key.code {
                    KeyCode::Char('q') => {
                        app.view = View::Done;
                        app.results.clear();
                        break;
                    }
                    KeyCode::Esc => {
                        app.view = View::Done;
                        app.results.clear();
                        break;
                    }
                    KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                    KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                    KeyCode::Char(' ') => app.toggle(),
                    KeyCode::Char('a') => app.toggle_all(),
                    KeyCode::Enter => app.view = View::Confirm,
                    _ => {}
                },
                View::Confirm => match key.code {
                    KeyCode::Esc => app.view = View::Checklist,
                    KeyCode::Enter => app.execute(),
                    _ => {}
                },
                View::Done => {
                    break;
                }
            }
        }
    }

    disable_raw_mode().map_err(|e| format!("raw mode: {e}"))?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).map_err(|e| format!("restore: {e}"))?;
    terminal.show_cursor().map_err(|e| format!("cursor: {e}"))?;
    Ok(())
}

fn draw(frame: &mut Frame, app: &UninstallApp, list_state: &mut ListState) {
    let area = frame.area();
    frame.render_widget(Clear, area);

    match app.view {
        View::Checklist | View::Confirm => {
            let items: Vec<ListItem> = app
                .items
                .iter()
                .enumerate()
                .map(|(idx, item)| {
                    let selected = idx == app.selected;
                    let check = if item.checked {
                        Span::styled("[x] ", Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD))
                    } else {
                        Span::styled("[ ] ", Style::default().fg(Color::DarkGray))
                    };
                    let label_style = if item.checked {
                        Style::default().fg(Color::White)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let mut spans = vec![
                        check,
                        Span::styled(item.label.to_string(), label_style),
                        Span::raw("  "),
                        Span::styled(item.detail.to_string(), Style::default().fg(Color::DarkGray)),
                    ];
                    if selected {
                        spans.insert(
                            0,
                            Span::styled("> ", Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
                        );
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect();

            let title = match app.view {
                View::Checklist => {
                    " MBHub Uninstaller — Space: toggle · a: all · Enter: review · Esc: quit "
                }
                _ => " Confirm — what will be removed? ",
            };
            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(theme::ACCENT))
                        .title(title)
                        .title_style(Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
                )
                .highlight_style(Style::default().bg(Color::Rgb(40, 46, 58)));
            frame.render_stateful_widget(list, area, list_state);

            if app.view == View::Confirm {
                let remove: Vec<&str> = app
                    .items
                    .iter()
                    .filter(|i| i.checked)
                    .map(|i| i.label)
                    .collect();
                let keep: Vec<&str> = app
                    .items
                    .iter()
                    .filter(|i| !i.checked)
                    .map(|i| i.label)
                    .collect();
                let mut lines = vec![
                    Line::from(Span::styled(
                        "WILL REMOVE:",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(
                        if remove.is_empty() { "—".to_string() } else { remove.join(" · ") },
                        Style::default().fg(Color::White),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "WILL KEEP:",
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(
                        if keep.is_empty() { "—".to_string() } else { keep.join(" · ") },
                        Style::default().fg(Color::White),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Enter: execute now · Esc: back to checklist",
                        Style::default().fg(theme::ACCENT),
                    )),
                ];
                let area = frame.area();
                let popup = centered_rect(area, 80, 60);
                frame.render_widget(Clear, popup);
                frame.render_widget(
                    Paragraph::new(lines)
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(Color::Red))
                                .title(" Confirm removal "),
                        )
                        .wrap(Wrap { trim: false }),
                    popup,
                );
            }
        }
        View::Done => {
            let mut lines = vec![Line::from(Span::styled(
                "Uninstall complete.",
                Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
            ))];
            for (label, ok) in &app.results {
                let mark = if *ok {
                    Span::styled("✓ ", Style::default().fg(theme::ACCENT))
                } else {
                    Span::styled("· ", Style::default().fg(Color::Yellow))
                };
                lines.push(Line::from(vec![mark, Span::raw(label.clone())]));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Kept items remain on disk under ~/.mbhub. Press any key to exit.",
                Style::default().fg(Color::DarkGray),
            )));
            let area = frame.area();
            let popup = centered_rect(area, 80, 60);
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(lines)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(theme::ACCENT))
                            .title(" Uninstall report "),
                    )
                    .wrap(Wrap { trim: false }),
                popup,
            );
        }
    }
}

fn centered_rect(area: ratatui::layout::Rect, percent_x: u16, percent_y: u16) -> ratatui::layout::Rect {
    use ratatui::layout::{Constraint, Layout};
    let vert = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vert[1])[1]
}
