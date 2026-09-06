//! MBHub — P2P Distributed Intelligence Network (terminal client).
//!
//! Run the interactive TUI with `cargo run`.
//! Render headless screen snapshots with `cargo run -- --snapshot`.

mod api;
mod app;
#[cfg(feature = "publisher")]
mod cms;
mod content_hash;
mod content_safety;
mod daemon;
mod db;
mod dlp;
mod env;
mod headless;
mod input;
mod ipc;
mod mcp;
mod model;
mod p2p;
mod sanitize;
mod seed;
mod service;
mod simhash;
mod theme;
mod tos;
mod ui;
mod update;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::Terminal;

use app::{App, Screen};

fn main() -> io::Result<()> {
    // Install panic hook to restore terminal state on unexpected panics.
    // Without this, a panic inside run_tui() leaves the terminal stuck in
    // raw mode with cursor hidden — unusable until `reset` is manually typed.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = execute!(io::stdout(), crossterm::cursor::Show);
        original_hook(panic_info);
    }));

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--seed") {
        let n = db::reseed();
        println!("Seeded {n} inference records into SQLite.");
        return Ok(());
    }
    if args.iter().any(|a| a == "--snapshot") {
        return snapshot();
    }

    if args.len() > 1 {
        match args[1].as_str() {
            "daemon" | "--daemon" => {
                let accept_terms = args.iter().any(|a| a == "--accept-terms");
                return daemon::run_daemon(accept_terms);
            }
            "bootstrap" => {
                // Dedicated rendezvous node (Kademlia server + relay server):
                // runs on cheap VPS instances, carries no user content.
                if let Err(e) = p2p::server::run_bootstrap_server() {
                    eprintln!("Bootstrap node error: {e}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            "mcp" => {
                let accept_terms = args.iter().any(|a| a == "--accept-terms");
                return mcp::run_mcp_server(accept_terms);
            }
            "ask" => {
                return handle_cli_ask(&args[2..]);
            }
            "service" => {
                return handle_cli_service(&args[2..]);
            }
            "status" => {
                service::status();
                return Ok(());
            }
            "update" => {
                let check_only = args.iter().any(|a| a == "--check");
                if let Err(e) = update::execute_update(check_only) {
                    eprintln!("Update error: {e}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            #[cfg(feature = "publisher")]
            "export-blog" => {
                return handle_cli_export_blog(&args[2..]);
            }
            #[cfg(feature = "publisher")]
            "cms" => {
                return handle_cli_cms(&args[2..]);
            }
            #[cfg(feature = "publisher")]
            "simhash" => {
                // Prints the 64-bit SimHash of each argument — used by the
                // maintainer's archive rehydration pipeline to restore exact
                // fingerprints without re-deriving them in JS.
                for arg in &args[2..] {
                    println!("{}", simhash::compute_simhash(arg));
                }
                return Ok(());
            }
            "help" | "--help" | "-h" => {
                print_cli_help();
                return Ok(());
            }
            _ => {}
        }
    }

    run_tui()
}

/// Broken-pipe-safe stdout write: `mbhub ask ... | head` closes the pipe
/// early; `println!` would panic on EPIPE, this just stops printing.
fn print_line(text: &str) {
    use std::io::Write;
    let mut stdout = io::stdout();
    let _ = writeln!(stdout, "{text}");
    let _ = stdout.flush();
}

fn handle_cli_ask(args: &[String]) -> io::Result<()> {
    let mut is_json = false;
    let mut accept_terms = false;
    let mut query_parts = Vec::new();

    for arg in args {
        if arg == "--json" {
            is_json = true;
        } else if arg == "--accept-terms" {
            accept_terms = true;
        } else {
            query_parts.push(arg.as_str());
        }
    }

    if accept_terms {
        db::set_meta("terms_accepted", "true");
    }

    let query = query_parts.join(" ");
    let trimmed = query.trim();
    if trimmed.is_empty() {
        eprintln!("Usage: mbhub ask <query> [--json] [--accept-terms]");
        std::process::exit(1);
    }

    if db::get_meta("terms_accepted") != Some("true".to_string()) {
        eprintln!("Error: MBHub Terms of Service have not been accepted yet.");
        eprintln!("Please launch `mbhub` once in your terminal to review and accept the Terms of Service, or run `mbhub ask --accept-terms <query>`.");
        std::process::exit(1);
    }

    // Try communicating with background daemon via IPC first
    let response = if let Some(ipc_resp) =
        ipc::try_query_daemon(&ipc::IpcRequest::Ask {
            query: trimmed.to_string(),
        })
    {
        match ipc_resp {
            ipc::IpcResponse::Answer {
                question,
                content,
                source,
                similarity,
                is_swarm,
            } => Ok((
                question,
                content,
                format!("{source} (via daemon IPC)"),
                similarity,
                is_swarm,
            )),
            ipc::IpcResponse::Error(err) => Err(err),
            _ => Err("Unexpected daemon response".to_string()),
        }
    } else {
        // Standalone fallback: execute directly
        match headless::execute_ask(trimmed, None) {
            Ok(ipc::IpcResponse::Answer {
                question,
                content,
                source,
                similarity,
                is_swarm,
            }) => Ok((question, content, source, similarity, is_swarm)),
            Ok(ipc::IpcResponse::Error(err)) | Err(err) => Err(err),
            _ => Err("Unexpected query response".to_string()),
        }
    };

    match response {
        Ok((question, content, source, similarity, _is_swarm)) => {
            if is_json {
                let out = serde_json::json!({
                    "question": question,
                    "content": content,
                    "source": source,
                    "similarity": similarity,
                });
                print_line(&serde_json::to_string_pretty(&out).unwrap_or_default());
            } else {
                print_line(&format!("# {question}\n"));
                print_line(&format!("{content}\n"));
                print_line("---");
                print_line(&format!("Source: {source} | Hit Rate: {similarity:.2}%"));
            }
            Ok(())
        }
        Err(err) => {
            eprintln!("Error: {}", err);
            std::process::exit(1);
        }
    }
}

fn handle_cli_service(args: &[String]) -> io::Result<()> {
    let subcmd = args.first().map(|s| s.as_str()).unwrap_or("status");
    match subcmd {
        "install" => {
            if let Err(e) = service::install() {
                eprintln!("Error installing service: {}", e);
                std::process::exit(1);
            }
        }
        "uninstall" => {
            if let Err(e) = service::uninstall() {
                eprintln!("Error uninstalling service: {}", e);
                std::process::exit(1);
            }
        }
        "status" => {
            service::status();
        }
        "start" => {
            if let Err(e) = service::start() {
                eprintln!("Error starting service: {}", e);
                std::process::exit(1);
            }
        }
        "stop" => {
            if let Err(e) = service::stop() {
                eprintln!("Error stopping service: {}", e);
                std::process::exit(1);
            }
        }
        "mcp" => {
            if let Err(e) = service::auto_configure_mcp() {
                eprintln!("Error configuring MCP: {}", e);
                std::process::exit(1);
            }
        }
        other => {
            eprintln!(
                "Unknown service command: {}. Available: install, uninstall, status, start, stop, mcp",
                other
            );
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Maintainer-only web archive pipeline (compiled exclusively into
/// `--features publisher` builds and never distributed).
#[cfg(feature = "publisher")]
fn handle_cli_cms(args: &[String]) -> io::Result<()> {
    if args.is_empty() {
        println!("MBHub CMS & Web Publisher Management");
        println!("Usage: mbhub cms <command>\n");
        println!("Commands:");
        println!("  sync        Execute export and deploy to mbhub.dev");
        println!("  status      Show local candidates vs web archive statistics");
        println!("  rehydrate   Restore local SQLite from content/ markdown files");
        return Ok(());
    }

    let Some(cms_dir) = cms::cms_dir() else {
        eprintln!("Error: no mbhub-cms repository found.");
        eprintln!("Set MBHUB_CMS_DIR=<path> to the local web archive repository,");
        eprintln!("or clone it into ~/.mbhub/cms and retry.");
        std::process::exit(1);
    };

    // Pin the child pipeline to the exact database this process is using.
    let db_path = db::db_path();

    match args[0].as_str() {
        "sync" => {
            println!("[CMS] Triggering synchronous web archive build & deploy...");
            let status = std::process::Command::new("node")
                .arg("scripts/sync.js")
                .current_dir(&cms_dir)
                .env("MBHUB_DB", &db_path)
                .status()?;
            if status.success() {
                println!("[CMS] Sync and deployment finished successfully.");
            } else {
                eprintln!("[CMS] Sync failed with exit code: {:?}", status.code());
            }
        }
        "status" => {
            println!("MBHub CMS Status:");
            println!("Repository: {}", cms_dir.display());
            let candidates = db::fetch_blog_export_candidates(0, true);
            println!("Local Approved Candidates: {} inquiry(ies)", candidates.len());
            let content_dir = cms_dir.join("content");
            if content_dir.exists() {
                let count = std::fs::read_dir(&content_dir)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
                            .count()
                    })
                    .unwrap_or(0);
                println!("Live Content Files:       {} article(s)", count);
            }
        }
        "rehydrate" => {
            println!("[CMS] Rehydrating local SQLite from web content directory...");
            let status = std::process::Command::new("node")
                .arg("scripts/rehydrate.js")
                .current_dir(&cms_dir)
                .env("MBHUB_DB", &db_path)
                .status()?;
            if status.success() {
                println!("[CMS] Rehydration finished successfully.");
            } else {
                eprintln!("[CMS] Rehydration failed with exit code: {:?}", status.code());
            }
        }
        other => {
            eprintln!("Unknown CMS command: {}. Available: sync, status, rehydrate", other);
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Maintainer-only blog export (compiled exclusively into
/// `--features publisher` builds and never distributed).
#[cfg(feature = "publisher")]
fn handle_cli_export_blog(args: &[String]) -> io::Result<()> {
    use chrono::TimeZone;
    let mut out_dir = None;
    let mut export_all = false;
    let mut dry_run = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                if i + 1 < args.len() {
                    out_dir = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--all" => export_all = true,
            "--dry-run" => dry_run = true,
            _ => {}
        }
        i += 1;
    }

    let target_dir = out_dir.unwrap_or_else(|| {
        if std::path::Path::new("../mbhub-cms/content").exists() {
            "../mbhub-cms/content".to_string()
        } else if std::path::Path::new("content").exists() {
            "content".to_string()
        } else {
            "./blog-export".to_string()
        }
    });

    let target_path = std::path::Path::new(&target_dir);
    if !dry_run {
        std::fs::create_dir_all(target_path)?;
    }

    let last_id = if export_all {
        0
    } else {
        db::get_last_blog_export_id()
    };

    let candidates = db::fetch_blog_export_candidates(last_id, export_all);
    if candidates.is_empty() {
        println!("No new blog export candidates found (last processed ID: {last_id}).");
        return Ok(());
    }

    println!("Found {} candidate(s) for blog export...", candidates.len());
    let mut exported = 0;
    let mut skipped = 0;
    let mut highest_id = last_id;

    for item in &candidates {
        // Double-pass DLP verification (Phase 1 — Privacy & Secret Leakage Gate)
        let dlp_q = dlp::scan_text(&item.question);
        let dlp_c = dlp::scan_text(&item.content);
        if dlp_q.is_sensitive || dlp_c.is_sensitive {
            eprintln!(
                "⚠️  DLP Gate intercepted record #{}: {:?} (question: {})",
                item.id,
                dlp_q.matched_pattern.or(dlp_c.matched_pattern),
                item.question
            );
            skipped += 1;
            continue;
        }

        // Double-pass Content Safety verification (Phase 1 — Content Integrity Gate)
        let safety_q = content_safety::screen_text(&item.question);
        let safety_c = content_safety::screen_text(&item.content);
        if matches!(safety_q, content_safety::SafetyVerdict::Reject { .. })
            || matches!(safety_c, content_safety::SafetyVerdict::Reject { .. })
        {
            eprintln!(
                "⚠️  Content Safety Gate blocked record #{}: {}",
                item.id, item.question
            );
            skipped += 1;
            continue;
        }

        // Generate human-readable SEO question slug (e.g. /how-to-implement-distributed-consensus-in-rust)
        let raw_key = format!("{}:{}", item.question, item.timestamp);
        let hash_hex = blake3::hash(raw_key.as_bytes()).to_hex();
        let base_slug = slugify(&item.question);
        let clean_title = item.question.replace('"', "\\\"");

        let base_file = target_path.join(format!("{base_slug}.md"));
        let slug = if base_file.exists() {
            // Check if existing file is for the same question
            let is_same = if let Ok(existing) = std::fs::read_to_string(&base_file) {
                existing.contains(&format!("title: \"{}\"", clean_title))
                    || (!item.content_hash.is_empty() && existing.contains(&item.content_hash))
            } else {
                false
            };

            if is_same {
                // If a legacy hash file existed, clean it up
                let legacy_hash_file = target_path.join(format!("{base_slug}-{}.md", &hash_hex[..6]));
                if legacy_hash_file.exists() && !dry_run {
                    let _ = std::fs::remove_file(&legacy_hash_file);
                }
                base_slug
            } else {
                format!("{base_slug}-{}", &hash_hex[..6])
            }
        } else {
            // Clean up any stray hash-suffixed file for this question
            let legacy_hash_file = target_path.join(format!("{base_slug}-{}.md", &hash_hex[..6]));
            if legacy_hash_file.exists() && !dry_run {
                let _ = std::fs::remove_file(&legacy_hash_file);
            }
            base_slug
        };

        // Format ISO 8601 date in real UTC. The `Z` suffix must never be
        // attached to a local-time value, or RSS/sitemap/JSON-LD dates drift
        // by the local UTC offset.
        let dt = chrono::Utc
            .timestamp_opt(item.timestamp, 0)
            .single()
            .unwrap_or_else(chrono::Utc::now);
        let date_str = dt.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let source_label = if item.is_swarm {
            "L2"
        } else if item.provider == "Local" {
            "L1"
        } else {
            "L3"
        };

        let markdown = format!(
            "---\ntitle: \"{clean_title}\"\nslug: \"{slug}\"\ndate: {date_str}\nsimilarity: {:.2}\nsource: \"{source_label}\"\nprovider: \"{provider}\"\nmodel: \"{model}\"\ncontent_hash: \"{content_hash}\"\nsimhash: {simhash}\nprovider_verified: false\n---\n\n{content}\n",
            item.similarity,
            provider = item.provider,
            model = item.model,
            content_hash = item.content_hash,
            simhash = item.simhash,
            content = item.content.trim()
        );

        let file_path = target_path.join(format!("{slug}.md"));
        if dry_run {
            println!("  [dry-run] Would export #{}: {} -> {:?}", item.id, item.question, file_path);
        } else {
            std::fs::write(&file_path, markdown)?;
            db::mark_published(item.id, chrono::Local::now().timestamp());
            if item.id > highest_id {
                highest_id = item.id;
            }
        }
        exported += 1;
    }

    if !dry_run && highest_id > last_id {
        db::set_last_blog_export_id(highest_id);
    }

    println!(
        "✓ Blog export complete: {exported} exported, {skipped} skipped. Target: {}",
        target_path.display()
    );
    Ok(())
}

#[cfg(feature = "publisher")]
fn slugify(text: &str) -> String {
    let transliterated = text
        .replace('ı', "i").replace('İ', "i")
        .replace('ğ', "g").replace('Ğ', "g")
        .replace('ü', "u").replace('Ü', "u")
        .replace('ş', "s").replace('Ş', "s")
        .replace('ö', "o").replace('Ö', "o")
        .replace('ç', "c").replace('Ç', "c");
    let mut slug = String::new();
    let mut prev_dash = true;
    for ch in transliterated.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "question".to_string()
    } else {
        trimmed.to_string()
    }
}

fn print_cli_help() {
    println!("MBHub — Sovereign P2P Collective AI Memory (v{})", env!("CARGO_PKG_VERSION"));
    println!("\nUsage:");
    println!("  mbhub                     Launch the interactive retro terminal UI");
    println!("  mbhub ask <query> [--json] Ask a question via headless 3-layer pipeline");
    println!("  mbhub daemon              Run the 24/7 background P2P & IPC daemon");
    println!("  mbhub bootstrap           Run a dedicated rendezvous node (Kademlia + relay server, no data)");
    println!("  mbhub mcp [--accept-terms] Start stdio JSON-RPC 2.0 MCP server (Cursor, Claude, agents)");
    #[cfg(feature = "publisher")]
    {
        println!("  mbhub export-blog [--out <dir>] [--all] Export local Q&A records to Astro markdown");
        println!("  mbhub cms <sync|status|rehydrate> Manage the local web archive pipeline");
        println!("  mbhub simhash <text>      Print the 64-bit SimHash fingerprint of text");
    }
    println!("  mbhub status              Check operational status of service & P2P swarm");
    println!("  mbhub service install     Install MBHub daemon as system auto-start service");
    println!("  mbhub service status      Check operational status of service & P2P swarm");
    println!("  mbhub service start|stop  Start or stop the background service");
    println!("  mbhub service uninstall   Uninstall and remove the background service");
    println!("  mbhub update [--check]    Seamlessly upgrade MBHub executable (zero DB loss)");
    println!("  mbhub help                Show this help message");
}

fn run_tui() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();

    let result = (|| -> io::Result<()> {
        loop {
            terminal.draw(|f| ui::render(f, &app))?;

            if app.quit {
                break;
            }

            if event::poll(Duration::from_millis(50))? {
                let ev = event::read()?;
                let is_release = matches!(&ev, Event::Key(k) if k.kind == KeyEventKind::Release);
                if !is_release {
                    app.handle_event(ev);
                }
            }

            app.tick();
        }
        Ok(())
    })();

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

/// Render every screen to a fixed-size buffer and print it, so the layout can
/// be reviewed without a real terminal (`cargo run -- --snapshot`).
fn snapshot() -> io::Result<()> {
    for screen in [Screen::Search, Screen::Memory, Screen::Settings] {
        let app = App::for_screen(screen);
        println!("============== {screen:?} ==============");
        println!("{}", render_string(&app, 110, 30));
    }

    let mut terms_app = App::for_screen(Screen::Search);
    terms_app.terms_modal = true;
    println!("============== Terms of Service Gate Modal ==============");
    println!("{}", render_string(&terms_app, 110, 30));

    let mut viewer_app = App::for_screen(Screen::Search);
    viewer_app.search_input.insert_str("How does distributed P2P inference work?");
    viewer_app.handle_event(Event::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    )));
    println!("============== Markdown Viewer (Search Response) ==============");
    println!("{}", render_string(&viewer_app, 110, 30));

    Ok(())
}

fn render_string(app: &App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, app)).unwrap();

    let buf = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            line.push_str(buf[(x, y)].symbol());
        }
        while line.ends_with(' ') {
            line.pop();
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::SettingsField;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::buffer::Buffer;
    use std::sync::Mutex;

    static DB_LOCK: Mutex<()> = Mutex::new(());

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn buffer_of(app: &App) -> Buffer {
        buffer_of_size(app, 110, 30)
    }

    fn buffer_of_size(app: &App, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::render(f, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn lock_db() -> (std::sync::MutexGuard<'static, ()>, std::sync::MutexGuard<'static, ()>) {
        let guard_env = crate::env::ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let guard_db = DB_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("MBHUB_DB", "mbhub_test.db");
            std::env::set_var("MBHUB_ENV_FILE", "mbhub_test.env");
        }
        let _ = std::fs::remove_file("mbhub_test.env");
        let records = db::load_records();
        if records.is_empty() {
            let _ = db::reseed();
        }
        db::set_meta("terms_accepted", "true");
        (guard_db, guard_env)
    }

    #[test]
    fn memory_selected_row_is_white() {
        let _guard = lock_db();
        let _ = db::reseed();
        let app = App::for_screen(Screen::Memory);
        let buf = buffer_of(&app);
        // y=0 header bar, body from y=1:
        //   y=1 indicator, y=2 column header, list starts at y=3
        assert_eq!(buf[(0, 3)].bg, theme::SELECT_BG);
        assert_eq!(buf[(0, 3)].bg, ratatui::style::Color::White);
        assert_ne!(buf[(0, 4)].bg, theme::SELECT_BG);
    }

    #[test]
    fn memory_scrolls_only_at_edges() {
        let _guard = lock_db();
        let _ = db::reseed();
        let mut app = App::for_screen(Screen::Memory);
        app.memory_height.set(10);

        for _ in 0..15 {
            app.handle_event(key(KeyCode::Down));
        }
        assert_eq!(app.memory_selected, 15);
        assert_eq!(app.memory_offset, 6); // 15 - 10 + 1

        // Moving up within the visible window must not scroll.
        for _ in 0..3 {
            app.handle_event(key(KeyCode::Up));
        }
        assert_eq!(app.memory_selected, 12);
        assert_eq!(app.memory_offset, 6);

        // Reach the top of the window without scrolling yet.
        for _ in 0..6 {
            app.handle_event(key(KeyCode::Up));
        }
        assert_eq!(app.memory_selected, 6);
        assert_eq!(app.memory_offset, 6);

        // Now move one above offset -> offset should scroll up to 5.
        app.handle_event(key(KeyCode::Up));
        assert_eq!(app.memory_selected, 5);
        assert_eq!(app.memory_offset, 5);
    }

    #[test]
    fn footer_highlights_active_screen() {
        let _guard = lock_db();
        let app = App::for_screen(Screen::Search);
        let buf = buffer_of(&app);
        let y = 29u16; // footer row in the 110x30 buffer
        assert_eq!(buf[(13, y)].symbol(), "A");
        assert_eq!(buf[(13, y)].bg, ratatui::style::Color::White);
        assert_eq!(buf[(12, y)].bg, ratatui::style::Color::White); // left extension
        assert_eq!(buf[(13, y)].bg, ratatui::style::Color::White); // 'A'
        assert_eq!(buf[(16, y)].bg, ratatui::style::Color::White); // right extension
        assert_eq!(buf[(11, y)].bg, theme::ACCENT); // outer gap stays green
        assert_eq!(buf[(18, y)].bg, theme::ACCENT); // inactive "MEMORY" on green
    }

    #[test]
    fn footer_labels_do_not_shift_between_screens() {
        let _guard = lock_db();
        let y = 29u16;
        let symbols = |screen: Screen| -> String {
            let app = App::for_screen(screen);
            let buf = buffer_of(&app);
            (0..110)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        };
        let search = symbols(Screen::Search);
        let memory = symbols(Screen::Memory);
        let settings = symbols(Screen::Settings);
        assert_eq!(search, memory);
        assert_eq!(search, settings);
    }

    #[test]
    fn search_caps_at_80_chars() {
        let mut app = App::for_screen(Screen::Search);
        for _ in 0..450 {
            app.handle_event(key(KeyCode::Char('a')));
        }
        assert_eq!(app.query_char_count(), 80);
    }

    #[test]
    fn search_input_wraps_at_screen_width() {
        let mut app = App::for_screen(Screen::Search);
        app.search_input.insert_str("abcdefghij");
        // Narrow frame (4 cols) so the 10-char query soft-wraps.
        let backend = TestBackend::new(4, 20);
        let mut t = Terminal::new(backend).unwrap();
        t.draw(|f| ui::render(f, &app)).unwrap();
        let w = app.search_width.get();
        assert_eq!(app.search_input.visual_text(w), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn settings_flat_navigation_navigates_all_fields_with_arrows() {
        let mut app = App::for_screen(Screen::Settings);
        assert_eq!(app.focus, SettingsField::DateFormat);

        app.handle_event(key(KeyCode::Down));
        assert_eq!(app.focus, SettingsField::Storage);

        app.handle_event(key(KeyCode::Down));
        assert_eq!(app.focus, SettingsField::ShardingMode);

        app.handle_event(key(KeyCode::Up));
        assert_eq!(app.focus, SettingsField::Storage);
    }

    #[test]
    fn settings_date_format_cycles_with_arrows() {
        let mut app = App::for_screen(Screen::Settings);
        assert_eq!(app.settings.date_format, crate::model::DateFormat::DotDmy);
        app.handle_event(key(KeyCode::Right));
        assert_eq!(app.settings.date_format, crate::model::DateFormat::IsoDash);
        app.handle_event(key(KeyCode::Left));
        assert_eq!(app.settings.date_format, crate::model::DateFormat::DotDmy);
    }

    #[test]
    fn settings_storage_edits_numeric_value() {
        let mut app = App::for_screen(Screen::Settings);
        app.handle_event(key(KeyCode::Down));
        assert_eq!(app.focus, SettingsField::Storage);
        // enter edit mode (buffer prefilled with "32")
        app.handle_event(key(KeyCode::Enter));
        assert!(app.editing);
        // replace with "64"
        app.handle_event(key(KeyCode::Backspace));
        app.handle_event(key(KeyCode::Backspace));
        app.handle_event(key(KeyCode::Char('6')));
        app.handle_event(key(KeyCode::Char('4')));
        app.handle_event(key(KeyCode::Enter));
        assert!(!app.editing);
        assert_eq!(app.settings.reserved_gb, 64);
    }

    #[test]
    fn seed_corpus_has_at_least_100_records() {
        let _guard = lock_db();
        let _ = db::reseed();
        let records = db::load_records();
        assert!(
            records.len() >= 100,
            "expected >=100 records, got {}",
            records.len()
        );
        for r in &records {
            assert!(
                (1.0..=100.0).contains(&(r.similarity as f64)),
                "similarity out of range: {}",
                r.similarity
            );
            assert!(!r.question.trim().is_empty(), "record has empty question");
            assert!(r.question.chars().count() <= 80, "question exceeds 80 chars: {}", r.question);
            assert!(!r.content.trim().is_empty(), "record has empty content");
        }
    }

    #[test]
    fn settings_sharding_mode_cycles_with_purge_confirm() {
        let _guard = lock_db();
        let mut app = App::for_screen(Screen::Settings);
        while app.focus != SettingsField::ShardingMode {
            app.handle_event(key(KeyCode::Down));
        }
        assert_eq!(app.focus, SettingsField::ShardingMode);
        assert_eq!(app.settings.sharding_mode, crate::model::ShardingMode::QueryLocality);

        // Arrow right triggers purge confirm modal
        app.handle_event(key(KeyCode::Right));
        assert!(app.confirm_modal.is_some());
        assert_eq!(
            app.confirm_modal.as_ref().unwrap().pending_sharding_mode,
            Some(crate::model::ShardingMode::BlindSwarm)
        );

        // Confirm
        app.handle_event(key(KeyCode::Enter));
        assert!(app.confirm_modal.is_none());
        assert_eq!(app.settings.sharding_mode, crate::model::ShardingMode::BlindSwarm);

        // Restore seed
        let _ = db::reseed();
    }

    #[test]
    fn settings_hit_rate_cycles_and_picker() {
        let mut app = App::for_screen(Screen::Settings);
        while app.focus != SettingsField::HitRate {
            app.handle_event(key(KeyCode::Down));
        }
        assert_eq!(app.focus, SettingsField::HitRate);
        assert_eq!(app.settings.hit_rate, crate::model::HitRate::Percent85);

        app.handle_event(key(KeyCode::Right));
        assert_eq!(app.settings.hit_rate, crate::model::HitRate::Percent90);

        app.handle_event(key(KeyCode::Left));
        assert_eq!(app.settings.hit_rate, crate::model::HitRate::Percent85);

        // Open Picker Modal
        app.handle_event(key(KeyCode::Enter));
        assert!(app.picker_modal.is_some());
        assert_eq!(app.picker_modal.as_ref().unwrap().title, "Select hit rate threshold");
        app.handle_event(key(KeyCode::Esc));
        assert!(app.picker_modal.is_none());
    }

    #[test]
    fn settings_freshness_cycles_with_arrows() {
        let mut app = App::for_screen(Screen::Settings);
        while app.focus != SettingsField::Freshness {
            app.handle_event(key(KeyCode::Down));
        }
        assert_eq!(app.focus, SettingsField::Freshness);
        assert_eq!(app.settings.freshness, crate::model::Freshness::AnyTime);

        app.handle_event(key(KeyCode::Right));
        assert_eq!(app.settings.freshness, crate::model::Freshness::Hours24);

        app.handle_event(key(KeyCode::Right));
        assert_eq!(app.settings.freshness, crate::model::Freshness::Days7);

        app.handle_event(key(KeyCode::Left));
        assert_eq!(app.settings.freshness, crate::model::Freshness::Hours24);

        app.handle_event(key(KeyCode::Left));
        assert_eq!(app.settings.freshness, crate::model::Freshness::AnyTime);
    }

    #[test]
    fn freshness_cutoff_calculation() {
        use crate::model::Freshness;
        let now = 1_700_000_000i64;
        assert_eq!(Freshness::AnyTime.min_timestamp(now), None);
        assert_eq!(Freshness::Hours24.min_timestamp(now), Some(now - 86400));
        assert_eq!(Freshness::Days7.min_timestamp(now), Some(now - 7 * 86400));
        assert_eq!(Freshness::Days30.min_timestamp(now), Some(now - 30 * 86400));
        assert_eq!(Freshness::Days90.min_timestamp(now), Some(now - 90 * 86400));
        assert_eq!(Freshness::Year1.min_timestamp(now), Some(now - 365 * 86400));
    }

    #[test]
    fn settings_provider_picker_modal_and_selection() {
        let _guard = lock_db();
        db::clear_meta();
        let mut app = App::for_screen(Screen::Settings);
        while app.focus != SettingsField::Provider {
            app.handle_event(key(KeyCode::Down));
        }
        assert_eq!(app.focus, SettingsField::Provider);
        assert!(app.picker_modal.is_none());

        // Press Enter to open Modal Picker
        app.handle_event(key(KeyCode::Enter));
        assert!(app.picker_modal.is_some());
        let picker = app.picker_modal.as_ref().unwrap();
        assert_eq!(picker.title, "Select AI provider");

        // Move down in picker list
        app.handle_event(key(KeyCode::Down));
        assert_eq!(app.picker_modal.as_ref().unwrap().selected, 1);

        // Press Enter to confirm selection (Anthropic)
        app.handle_event(key(KeyCode::Enter));
        assert!(app.picker_modal.is_none());
        assert_eq!(app.settings.provider_idx, 1);
        assert_eq!(crate::model::PROVIDERS[1].name, "Anthropic");
    }

    #[test]
    fn settings_provider_model_conditional_visibility() {
        let _guard = lock_db();
        db::clear_meta();
        let mut app = App::for_screen(Screen::Settings);
        // By default with no API key, provider_models is empty
        assert!(app.provider_models.is_empty());
        assert!(!app.visible_fields().contains(&SettingsField::ProviderModel));

        // When models are discovered (or key entered):
        app.provider_models = vec!["gpt-4o".to_string(), "o3-mini".to_string()];
        assert!(app.visible_fields().contains(&SettingsField::ProviderModel));

        // Focus ProviderModel
        while app.focus != SettingsField::ProviderModel {
            app.handle_event(key(KeyCode::Down));
        }
        assert_eq!(app.focus, SettingsField::ProviderModel);

        // Cycle with Right arrow
        app.handle_event(key(KeyCode::Right));
        assert_eq!(app.settings.provider_model, "o3-mini");
    }

    #[test]
    fn inference_source_gossip_policy_check() {
        use crate::model::InferenceSource;

        let cloud_source = InferenceSource::CloudProvider {
            provider: "OpenAI".to_string(),
            model: "gpt-4o".to_string(),
        };
        assert!(cloud_source.can_gossip_to_swarm());

        let peer_source = InferenceSource::SwarmPeer {
            peer_id: "12D3KooWtest...".to_string(),
        };
        assert!(!peer_source.can_gossip_to_swarm());
    }

    #[test]
    fn settings_clear_cache_modal_and_purge() {
        let _guard = lock_db();
        let mut app = App::for_screen(Screen::Settings);
        assert!(!app.records.is_empty());

        // Step focus to ClearCache dynamically
        while app.focus != SettingsField::ClearCache {
            app.handle_event(key(KeyCode::Down));
        }
        assert_eq!(app.focus, SettingsField::ClearCache);
        assert!(app.confirm_modal.is_none());

        // Press Enter to open modal
        app.handle_event(key(KeyCode::Enter));
        assert!(app.confirm_modal.is_some());

        // Press Esc to cancel
        app.handle_event(key(KeyCode::Esc));
        assert!(app.confirm_modal.is_none());
        assert!(!app.records.is_empty());

        // Press Enter again to reopen modal
        app.handle_event(key(KeyCode::Enter));
        assert!(app.confirm_modal.is_some());

        // Confirm purge with Enter
        app.handle_event(key(KeyCode::Enter));
        assert!(app.confirm_modal.is_none());
        assert!(app.records.is_empty());
        assert_eq!(app.memory_selected, 0);

        // Restore seed records for clean state
        let _ = db::reseed();
    }

    #[test]
    fn search_enter_opens_response_viewer_and_esc_returns_clean_input() {
        let mut app = App::for_screen(Screen::Search);
        app.search_input.insert_str("how to scale vector search");
        assert!(app.viewer.is_none());

        // Press Enter to submit question
        app.handle_event(key(KeyCode::Enter));
        assert!(app.viewer.is_some());

        let viewer = app.viewer.as_ref().unwrap();
        assert!(viewer.content.contains("how to scale vector search"));

        // Render with viewer open
        let buf = buffer_of(&app);
        // Footer should show "esc: back"
        let y = 29u16;
        let footer_left: String = (0..10).map(|x| buf[(x, y)].symbol()).collect();
        assert!(footer_left.starts_with("esc: back"));

        // Press Esc to return to the ASK screen: the response viewer closes
        // AND the query input is cleared for the next atomic question —
        // the answer itself remains available in MEMORY.
        app.handle_event(key(KeyCode::Esc));
        assert!(app.viewer.is_none());
        assert_eq!(app.search_input.text(), "");
        assert_eq!(app.query_char_count(), 0);
    }

    #[test]
    fn memory_enter_opens_record_viewer_and_esc_restores_selection() {
        let _guard = lock_db();
        let _ = db::reseed();
        let mut app = App::for_screen(Screen::Memory);
        for _ in 0..5 {
            app.handle_event(key(KeyCode::Down));
        }
        assert_eq!(app.memory_selected, 5);
        let expected_content = format!(
            "# {}\n\n{}",
            app.records[5].question,
            app.records[5].content
        );

        // Press Enter on record #5
        app.handle_event(key(KeyCode::Enter));
        assert!(app.viewer.is_some());
        assert_eq!(app.viewer.as_ref().unwrap().content, expected_content);

        // Press Esc to return to list
        app.handle_event(key(KeyCode::Esc));
        assert!(app.viewer.is_none());
        assert_eq!(app.memory_selected, 5);
    }

    #[test]
    fn viewer_scrolling_keys_navigate_content() {
        let mut app = App::for_screen(Screen::Search);
        let mut long_text = String::new();
        for i in 0..50 {
            long_text.push_str(&format!("Line number {i} of long test content\n"));
        }
        app.viewer = Some(crate::ui::viewer::ViewerState::new(long_text));

        // Set body dimensions
        app.body_width.set(80);
        app.body_height.set(10);

        // Scroll down
        app.handle_event(key(KeyCode::Down));
        assert_eq!(app.viewer.as_ref().unwrap().scroll_offset, 1);

        // Scroll page down
        app.handle_event(key(KeyCode::PageDown));
        assert!(app.viewer.as_ref().unwrap().scroll_offset > 1);

        // Scroll to bottom
        app.handle_event(key(KeyCode::End));
        let max_off = app.viewer.as_ref().unwrap().max_offset(80, 10);
        assert_eq!(app.viewer.as_ref().unwrap().scroll_offset, max_off);

        // Scroll to top
        app.handle_event(key(KeyCode::Home));
        assert_eq!(app.viewer.as_ref().unwrap().scroll_offset, 0);
    }

    #[test]
    fn viewer_scroll_to_bottom_shows_last_line_with_metadata() {
        let mut app = App::for_screen(Screen::Search);
        let mut text = String::new();
        for i in 0..30 {
            text.push_str(&format!("Line #{i:02} answer text\n"));
        }
        app.viewer = Some(crate::ui::viewer::ViewerState::with_metadata(
            text,
            "DeepSeek",
            "deepseek-chat",
            "01.01.2026 12:00",
        ));

        // Terminal height 15 (header: 1, footer: 1, body: 13)
        // With metadata, content_area is 13 - 1 = 12.
        let _buf = buffer_of_size(&app, 60, 15);
        let v = app.viewer.as_ref().unwrap();
        assert_eq!(v.visible_height(13), 12);

        // Scroll to bottom
        app.handle_event(key(KeyCode::End));
        let buf_bottom = buffer_of_size(&app, 60, 15);

        // Scan all lines in buffer for the very last line: "Line #29 answer text"
        let mut found_last_line = false;
        for y in 0..15 {
            let row_str: String = (0..60).map(|x| buf_bottom[(x, y)].symbol()).collect();
            if row_str.contains("Line #29 answer text") {
                found_last_line = true;
                break;
            }
        }
        assert!(found_last_line, "The very last line must be rendered and visible on screen");
    }

    #[test]
    fn settings_help_box_never_clipped_at_narrow_widths() {
        let mut app = App::for_screen(Screen::Settings);
        // Focus on Storage, which has a very long description
        while app.focus != SettingsField::Storage {
            app.handle_event(key(KeyCode::Down));
        }
        assert_eq!(app.focus, SettingsField::Storage);

        // Render at narrow width (50 columns, 24 rows)
        let buf = buffer_of_size(&app, 50, 24);

        // Check that description lines and the hint line are rendered
        let mut full_screen_text = String::new();
        for y in 0..24 {
            let row_str: String = (0..50).map(|x| buf[(x, y)].symbol()).collect();
            full_screen_text.push_str(&row_str);
            full_screen_text.push('\n');
        }

        assert!(full_screen_text.contains("Reserved storage"), "Title must be present");
        assert!(full_screen_text.contains("Blind swarm evicts the oldest"), "End of description must not be clipped");
        assert!(full_screen_text.contains("💡"), "Hint icon must be present");
        assert!(full_screen_text.contains("Enter: edit storage quota"), "Hint text must not be clipped");
    }

    #[test]
    fn memory_blind_swarm_hides_hit_column() {
        let _guard = lock_db();
        let _ = db::reseed();
        let mut app = App::for_screen(Screen::Memory);
        // Under QueryLocality, HIT (%) is in the header (row 2: row 0=header, row 1=indicator, row 2=table header)
        let buf = buffer_of(&app);
        let header_row = 2u16;
        let line: String = (0..110).map(|x| buf[(x, header_row)].symbol()).collect();
        assert!(line.contains("HIT (%)"));

        // Under BlindSwarm, HIT (%) column is omitted
        app.settings.sharding_mode = crate::model::ShardingMode::BlindSwarm;
        let buf2 = buffer_of(&app);
        let line2: String = (0..110).map(|x| buf2[(x, header_row)].symbol()).collect();
        assert!(!line2.contains("HIT (%)"));
    }

    #[test]
    fn db_simhash_match_and_save_inference() {
        let _guard = lock_db();
        let _ = db::reseed();

        // Exact or close match should return the record
        let hit = db::find_best_match("When should Arc<Mutex<T>> be used in Rust?", 85.0);
        assert!(hit.is_some());
        let record = hit.unwrap();
        assert!(record.question.contains("Arc<Mutex<T>>"));
        assert!(record.similarity >= 85.0);

        // Save a fresh inference with precomputed SimHash and provider/model
        let q = "What is zero-cost abstraction?";
        let h = crate::simhash::compute_simhash(q);
        let saved = db::save_inference(
            q,
            "Zero-cost abstractions compile down to optimal assembly.",
            h,
            "DeepSeek",
            "deepseek-chat",
        ).expect("valid inference saves");
        assert_eq!(saved.question, q);
        assert_eq!(saved.simhash, h);
        assert_eq!(saved.provider, "DeepSeek");
        assert_eq!(saved.model, "deepseek-chat");

        // Querying the freshly saved record
        let hit_saved = db::find_best_match(q, 90.0);
        assert!(hit_saved.is_some());
        let hit_rec = hit_saved.unwrap();
        assert_eq!(hit_rec.content, "Zero-cost abstractions compile down to optimal assembly.");
        assert_eq!(hit_rec.provider, "DeepSeek");
        assert_eq!(hit_rec.model, "deepseek-chat");

        let _ = db::clear_all();
    }

    #[test]
    fn ask_query_cache_hit_resolves_instantly() {
        let _guard = lock_db();
        let _ = db::reseed();
        let mut app = App::for_screen(Screen::Search);

        // Query known seed entry
        app.search_input.insert_str("When should Arc<Mutex<T>> be used in Rust?");
        app.handle_event(key(KeyCode::Enter));

        assert!(app.viewer.is_some());
        let viewer = app.viewer.as_ref().unwrap();
        assert!(!viewer.is_streaming);
        assert!(viewer.content.contains("Arc<Mutex<T>>"));

        let _ = db::clear_all();
    }

    #[test]
    fn ask_query_cache_miss_without_api_key_shows_guidance() {
        let _guard = lock_db();
        let _ = db::clear_all();
        let mut app = App::for_screen(Screen::Search);
        app.settings.api_key.clear();

        // Query unknown entry
        app.search_input.insert_str("Quantum teleportation protocol details");
        app.handle_event(key(KeyCode::Enter));

        assert!(app.viewer.is_some());
        let viewer = app.viewer.as_ref().unwrap();
        assert!(!viewer.is_streaming);
        assert!(viewer.content.contains("API Key Required"));
        assert!(viewer.content.contains("SETTINGS > Cloud AI provider"));
    }

    #[test]
    fn swarm_query_request_and_peer_response_flow() {
        let _guard = lock_db();
        let _ = db::clear_all();
        let mut app = App::for_screen(Screen::Search);

        // Setup simulated peer query
        let q = "What is Raft consensus?";
        let h = crate::simhash::compute_simhash(q);
        let req_id = "test-req-123".to_string();

        app.pending_query = Some(crate::app::PendingSwarmQuery {
            request_id: req_id.clone(),
            question: q.to_string(),
            simhash: h,
            started_at: std::time::Instant::now(),
            broadcast_at: None, // already broadcast
        });

        // Simulate incoming peer response on query_response_rx channel.
        // The response must carry a valid BLAKE3 content hash (§5.1) — peers
        // without integrity data are silently rejected by the receiver gate.
        if let Some(p2p) = &app.p2p {
            let mut resp = crate::p2p::SwarmQueryResponse {
                request_id: req_id,
                responder_peer_id: "peer-999".to_string(),
                question: q.to_string(),
                content: "Raft is an understandable consensus algorithm.".to_string(),
                simhash: h,
                provider: "DeepSeek".to_string(),
                model: "deepseek-chat".to_string(),
                content_hash: String::new(),
            };
            resp.content_hash = resp.canonical_content_hash();
            // Send response directly through channel
            p2p.simulate_query_response(resp.clone());
        }

        // Advance tick loop
        app.tick();

        assert!(app.viewer.is_some());
        let viewer = app.viewer.as_ref().unwrap();
        assert!(viewer.metadata.is_some());
        let meta = viewer.metadata.as_ref().unwrap();
        // §5.2: swarm hits must never present the claimed brand as verified.
        assert!(meta.is_swarm, "swarm response must be marked unverified");
        assert_eq!(meta.model, "deepseek-chat");

        let _ = db::clear_all();
    }

    #[test]
    fn per_provider_api_key_and_model_persistence_across_switches_and_restarts() {
        let _guard = lock_db();
        db::clear_meta();
        let _ = db::clear_all();

        let mut app = App::for_screen(Screen::Settings);
        let openrouter_idx = crate::model::PROVIDERS
            .iter()
            .position(|p| p.name == "OpenRouter")
            .expect("OpenRouter provider exists");
        let deepseek_idx = crate::model::PROVIDERS
            .iter()
            .position(|p| p.name == "DeepSeek")
            .expect("DeepSeek provider exists");

        // 1. Switch to OpenRouter and save its API key
        app.set_provider(openrouter_idx);
        assert_eq!(app.settings.provider_idx, openrouter_idx);
        app.focus = SettingsField::ApiKey;
        app.editing = true;
        app.edit_buffer = crate::app::single_line("sk-or-v1-test-openrouter-key");
        app.handle_event(key(KeyCode::Enter));

        assert_eq!(app.settings.api_key, "sk-or-v1-test-openrouter-key");
        assert_eq!(
            db::get_provider_api_key("OpenRouter"),
            "sk-or-v1-test-openrouter-key"
        );

        // 2. Switch to DeepSeek - active API key should become empty (or previous DeepSeek key)
        app.set_provider(deepseek_idx);
        assert_eq!(app.settings.provider_idx, deepseek_idx);
        assert_eq!(app.settings.api_key, "");

        // 3. Set DeepSeek API key
        app.focus = SettingsField::ApiKey;
        app.editing = true;
        app.edit_buffer = crate::app::single_line("sk-ds-test-deepseek-key");
        app.handle_event(key(KeyCode::Enter));

        assert_eq!(app.settings.api_key, "sk-ds-test-deepseek-key");
        assert_eq!(
            db::get_provider_api_key("DeepSeek"),
            "sk-ds-test-deepseek-key"
        );

        // 4. Switch back to OpenRouter - verify OpenRouter key is automatically restored!
        app.set_provider(openrouter_idx);
        assert_eq!(app.settings.api_key, "sk-or-v1-test-openrouter-key");

        // 5. Switch back to DeepSeek - verify DeepSeek key is automatically restored!
        app.set_provider(deepseek_idx);
        assert_eq!(app.settings.api_key, "sk-ds-test-deepseek-key");

        // 6. Simulate fresh app start (rebooting after 2 days)
        let restarted_app = App::new();
        assert_eq!(
            restarted_app.settings.provider_keys.get("OpenRouter").unwrap(),
            "sk-or-v1-test-openrouter-key"
        );
        assert_eq!(
            restarted_app.settings.provider_keys.get("DeepSeek").unwrap(),
            "sk-ds-test-deepseek-key"
        );

        let _ = db::clear_all();
    }

    #[test]
    fn picker_modal_scrolls_only_at_edges_matching_memory() {
        let items: Vec<String> = (0..25).map(|i| format!("model-{i}")).collect();
        let picker = crate::app::PickerModal::new(
            "Select model".to_string(),
            items,
            0,
            SettingsField::ProviderModel,
        );

        let visible_h = 10;
        // Initially at index 0, offset must be 0
        assert_eq!(picker.scroll_into_view(visible_h), 0);

        // Move down inside visible window (0 -> 9)
        for i in 1..10 {
            let mut p = picker.clone();
            p.selected = i;
            // Moving within the window (0..10) must NOT scroll: offset stays 0!
            assert_eq!(p.scroll_into_view(visible_h), 0, "selection {i} should not scroll");
        }

        // When moving down past the edge to index 15
        let mut p = picker.clone();
        p.selected = 15;
        // 15 - 10 + 1 = 6
        assert_eq!(p.scroll_into_view(visible_h), 6);

        // Crucial test: When moving UP from 15 to 14, 13, 12, 11, 10, 9, 8, 7, 6:
        // Because 6..=15 are all within the visible window [6, 16),
        // offset MUST REMAIN 6! The whole menu must NOT scroll when moving to upper items!
        for i in (6..15).rev() {
            p.selected = i;
            assert_eq!(
                p.scroll_into_view(visible_h),
                6,
                "moving up to index {i} within visible window must keep offset at 6 without scrolling menu"
            );
        }

        // Only when moving up past the top edge (index 5 < 6) should the offset scroll up to 5!
        p.selected = 5;
        assert_eq!(p.scroll_into_view(visible_h), 5);
    }

    #[test]
    fn viewer_provenance_bar_shows_model_and_provider_on_detail_screen() {
        let _guard = lock_db();
        let _ = db::reseed();
        let mut app = App::for_screen(Screen::Memory);

        // Enter on first memory record to open detail viewer
        app.handle_event(key(KeyCode::Enter));
        assert!(app.viewer.is_some());

        let viewer = app.viewer.as_ref().unwrap();
        assert!(viewer.metadata.is_some());
        let meta = viewer.metadata.as_ref().unwrap();
        assert_eq!(meta.provider, "OpenAI");
        assert_eq!(meta.model, "gpt-4o");
        // Render to buffer
        let buf = buffer_of(&app);
        // Row 1 (inside body area) should contain the provenance bar with solid ACCENT background
        assert_eq!(buf[(0, 1)].bg, theme::ACCENT);
        let line_text: String = (0..80).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(line_text.contains("PROVIDER:"), "line should contain PROVIDER: in {}", line_text);
        assert!(line_text.contains("OpenAI"), "line should contain OpenAI in {}", line_text);
        assert!(line_text.contains("MODEL:"), "line should contain MODEL: in {}", line_text);
        assert!(line_text.contains("gpt-4o"), "line should contain gpt-4o in {}", line_text);
        assert!(line_text.contains("DATE:"), "line should contain DATE: in {}", line_text);

        // Row 2 starts directly with the markdown content (no divider dashes row)
        let row2_text: String = (0..80).map(|x| buf[(x, 2)].symbol()).collect();
        assert!(!row2_text.starts_with("───"), "row 2 must not be a divider line");

        let _ = db::clear_all();
    }

    #[test]
    fn db_backup_and_restore_cycle_restores_identical_inferences() {
        let _guard = lock_db();
        let _ = db::reseed();
        let initial_records = db::load_records();
        assert!(!initial_records.is_empty());

        let temp_dir = std::env::temp_dir();
        let backup_path = temp_dir.join(format!("mbhub_test_backup_{}.db", std::process::id()));

        // 1. Export database snapshot
        db::backup_to_file(&backup_path).expect("backup succeeds");
        assert!(backup_path.exists());
        assert!(backup_path.metadata().unwrap().len() > 0);

        // 2. Clear current database completely
        let _ = db::clear_all();
        let empty_records = db::load_records();
        assert!(empty_records.is_empty());

        // 3. Restore from backup snapshot
        let restored_count = db::restore_from_file(&backup_path).expect("restore succeeds");
        assert_eq!(restored_count, initial_records.len());

        let restored_records = db::load_records();
        assert_eq!(restored_records.len(), initial_records.len());
        assert_eq!(restored_records[0].question, initial_records[0].question);
        assert_eq!(restored_records[0].simhash, initial_records[0].simhash);
        assert_eq!(restored_records[0].provider, initial_records[0].provider);
        assert_eq!(restored_records[0].model, initial_records[0].model);

        // Cleanup
        let _ = std::fs::remove_file(&backup_path);
        let _ = db::clear_all();
    }

    #[test]
    fn settings_backup_and_restore_modals_open_and_navigate() {
        let _guard = lock_db();
        let mut app = App::for_screen(Screen::Settings);

        // Navigate to BackupDatabase field
        app.focus = SettingsField::BackupDatabase;
        app.handle_event(key(KeyCode::Enter));

        assert!(app.file_browser_modal.is_some());
        let browser = app.file_browser_modal.as_ref().unwrap();
        assert_eq!(browser.mode, crate::app::FileBrowserMode::SelectDirectory);
        assert!(browser.entries.iter().any(|e| e.is_action));

        // Close modal with Esc
        app.handle_event(key(KeyCode::Esc));
        assert!(app.file_browser_modal.is_none());

        // Navigate to RestoreDatabase field
        app.focus = SettingsField::RestoreDatabase;
        app.handle_event(key(KeyCode::Enter));

        assert!(app.file_browser_modal.is_some());
        let browser = app.file_browser_modal.as_ref().unwrap();
        assert_eq!(browser.mode, crate::app::FileBrowserMode::SelectFile);

        // Close modal with Esc
        app.handle_event(key(KeyCode::Esc));
        assert!(app.file_browser_modal.is_none());
    }

    #[test]
    fn file_browser_size_column_stays_fixed_on_hover() {
        let _guard = lock_db();
        let mut app = App::for_screen(Screen::Settings);

        app.focus = SettingsField::BackupDatabase;
        app.handle_event(key(KeyCode::Enter));
        assert!(app.file_browser_modal.is_some());

        // Locate two file rows in the browser entries.
        let file_indices: Vec<usize> = {
            let browser = app.file_browser_modal.as_ref().unwrap();
            browser
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| !e.is_dir && !e.is_action)
                .map(|(i, _)| i)
                .collect()
        };
        assert!(file_indices.len() >= 2, "test needs at least two files in cwd");

        let modal_x = 21u16; // (110-68)/2 for the 110-wide test buffer
        let inner_x = modal_x + 1;
        let inner_w = 66u16; // 68 - 2 borders
        let size_cols: u16 = 10;
        let size_x0 = inner_x + inner_w - size_cols; // fixed size column origin
        let entries_y0 = 8u16 + 2; // modal_y(7)+border(1)+path+separator

        let row_of = |app: &App, idx: usize| -> Option<u16> {
            let browser = app.file_browser_modal.as_ref().unwrap();
            let list_h = 11usize; // modal_h 16 - borders 2 - path/sep/help 3
            let start = browser.offset.get();
            if idx < start || idx >= start + list_h {
                return None;
            }
            Some(entries_y0 + (idx - start) as u16)
        };

        let size_text_at = |app: &App, idx: usize| -> Option<String> {
            let buf = buffer_of(app);
            let y = row_of(app, idx)?;
            let mut s = String::new();
            for x in size_x0..size_x0 + size_cols {
                s.push_str(buf[(x, y)].symbol());
            }
            Some(s)
        };

        // 1. Select file A and capture its size text + exact position.
        let a = file_indices[0];
        while app.file_browser_modal.as_ref().unwrap().selected < a {
            app.handle_event(key(KeyCode::Down));
        }
        let size_a_selected = size_text_at(&app, a).expect("row A visible");
        let trimmed_a = size_a_selected.trim().to_string();
        assert!(!trimmed_a.is_empty(), "size text must be visible");

        // 2. Move hover to file B: row A's size text must be IDENTICAL and in
        //    the SAME column — the reported "size shifts left on hover" bug.
        let b = file_indices[1];
        while app.file_browser_modal.as_ref().unwrap().selected < b {
            app.handle_event(key(KeyCode::Down));
        }
        let size_a_unselected = size_text_at(&app, a).expect("row A still visible");
        assert_eq!(
            size_a_unselected, size_a_selected,
            "size text must not move or change when hover leaves the row"
        );

        // 3. The size column is right-aligned inside its fixed block for B too.
        let size_b = size_text_at(&app, b).expect("row B visible");
        let trimmed_b = size_b.trim().to_string();
        assert!(!trimmed_b.is_empty());
        assert!(
            size_b.ends_with(trimmed_b.as_str()),
            "size must be right-aligned in the fixed column"
        );

        app.handle_event(key(KeyCode::Esc));
    }

    #[test]
    fn db_load_records_window_pagination() {
        let _guard = lock_db();
        let _ = db::reseed();
        let total = db::count_records();
        assert!(total >= 100);

        let slice1 = db::load_records_window(0, 10);
        assert_eq!(slice1.len(), 10);

        let slice2 = db::load_records_window(10, 10);
        assert_eq!(slice2.len(), 10);

        // Ensure slices represent consecutive rows without overlap
        assert_ne!(slice1[0].question, slice2[0].question);
        assert_eq!(slice1[9].question, db::get_record_at(9).unwrap().question);
        assert_eq!(slice2[0].question, db::get_record_at(10).unwrap().question);

        let _ = db::clear_all();
    }

    #[test]
    fn virtual_memory_window_sliding_and_scrolling() {
        let _guard = lock_db();
        let _ = db::reseed();
        let mut app = App::for_screen(Screen::Memory);
        assert_eq!(app.total_records, db::count_records());
        assert!(app.total_records >= 100);
        assert_eq!(app.records_offset, 0);

        app.memory_height.set(10);

        // Scroll down 40 items
        for _ in 0..40 {
            app.handle_event(key(KeyCode::Down));
        }
        assert_eq!(app.memory_selected, 40);
        assert_eq!(app.memory_offset, 31); // 40 - 10 + 1

        // Record at index 40 is accessible via get_memory_record
        let rec = app.get_memory_record(40);
        assert!(rec.is_some());

        // Press Enter on item 40 to open viewer
        app.handle_event(key(KeyCode::Enter));
        assert!(app.viewer.is_some());
        let v = app.viewer.as_ref().unwrap();
        assert!(v.content.contains(&rec.unwrap().question));

        // Esc closes viewer and restores memory state
        app.handle_event(key(KeyCode::Esc));
        assert!(app.viewer.is_none());
        assert_eq!(app.memory_selected, 40);

        let _ = db::clear_all();
    }

    #[test]
    fn search_dlp_blocks_sensitive_query_input() {
        let _guard = lock_db();
        let mut app = App::for_screen(Screen::Search);

        // Type sensitive API key into search input
        for c in "sk-proj1234567890abcdefghij".chars() {
            app.handle_event(key(KeyCode::Char(c)));
        }

        // Press Enter
        app.handle_event(key(KeyCode::Enter));

        // Must display DLP warning and block query from dispatching
        assert!(app.viewer.is_some());
        let v = app.viewer.as_ref().unwrap();
        assert!(v.content.contains("Sensitive Data Detected"));
        assert!(v.content.contains("API Key"));
        assert!(app.pending_query.is_none());
    }

    #[test]
    fn db_save_inference_computes_and_persists_content_hash() {
        let _guard = lock_db();
        let record = db::save_inference(
            "What is BLAKE3?",
            "Cryptographic hash function.",
            0x1234567890ABCDEF,
            "OpenAI",
            "gpt-4o",
        ).expect("valid inference saves");

        let expected_hash = content_hash::compute_content_hash(
            "What is BLAKE3?",
            "Cryptographic hash function.",
            "OpenAI",
            "gpt-4o",
        );

        // Verify stored content_hash directly in SQLite
        let conn = rusqlite::Connection::open(db::db_path()).unwrap();
        let stored_hash: String = conn.query_row(
            "SELECT content_hash FROM inferences WHERE question = ?1",
            ["What is BLAKE3?"],
            |row| row.get(0),
        ).unwrap();

        assert_eq!(stored_hash, expected_hash);
        assert_eq!(stored_hash.len(), 64);
        assert!(content_hash::verify_content_hash(
            &stored_hash,
            &record.question,
            &record.content,
            &record.provider,
            &record.model,
        ));

        let _ = db::clear_all();
    }

    #[test]
    fn sanitize_strips_terminal_escape_in_markdown() {
        let malicious_payload = "# Normal Title\n\n\x1b[31mRed text\x1b[0m and \x1b]52;c;bWFsaWNpb3Vz\x07normal text.";
        let lines = ui::markdown::render_markdown(malicious_payload, 80);

        let mut all_text = String::new();
        for line in lines {
            for span in line.spans {
                assert!(!span.content.contains('\x1b'), "Escape byte found in span: {}", span.content);
                assert!(!span.content.contains('\x07'), "BEL byte found in span: {}", span.content);
                assert!(!span.content.contains("bWFsaWNpb3Vz"), "OSC 52 payload should be stripped");
                all_text.push_str(&span.content);
            }
        }
        assert!(all_text.contains("Red text"));
        assert!(all_text.contains("normal text"));
    }

    // ──────────────────────────────────────────────────────────────
    // Security integration tests
    // ──────────────────────────────────────────────────────────────

    /// Builds a valid, integrity-check-passing inbound inference message.
    fn valid_swarm_message(
        question: &str,
        content: &str,
        provider: &str,
        model: &str,
    ) -> crate::p2p::SwarmInferenceMessage {
        let mut msg = crate::p2p::SwarmInferenceMessage {
            question: question.to_string(),
            content: content.to_string(),
            timestamp: chrono::Local::now().timestamp(),
            simhash: crate::simhash::compute_simhash(question),
            provider: provider.to_string(),
            model: model.to_string(),
            content_hash: String::new(),
            hop_ttl: crate::p2p::MAX_HOP_TTL,
            is_truncated: false,
        };
        msg.content_hash = msg.canonical_content_hash();
        msg
    }

    #[test]
    fn swarm_inbound_integrity_gate_rejects_tampered_content() {
        let _guard = lock_db();
        let _ = db::clear_all();
        let mut app = App::for_screen(Screen::Search);
        let before = app.total_records;

        // 1. Tampered payload (hash mismatch) must be dropped.
        let mut tampered = valid_swarm_message(
            "What is a monad?",
            "A monad is a monoid in the category of endofunctors.",
            "OpenAI",
            "gpt-4o",
        );
        tampered.content = "EVIL REPLACEMENT".to_string();
        if let Some(p2p) = &app.p2p {
            p2p.simulate_inbound_inference(tampered);
        }
        app.tick();
        assert_eq!(app.total_records, before, "tampered gossip must be dropped");

        // 2. Valid payload must be stored as swarm-sourced.
        let valid = valid_swarm_message(
            "What is a monad?",
            "A monad is a monoid in the category of endofunctors.",
            "OpenAI",
            "gpt-4o",
        );
        if let Some(p2p) = &app.p2p {
            p2p.simulate_inbound_inference(valid);
        }
        app.tick();
        assert_eq!(app.total_records, before + 1);
        let rec = db::find_best_match("What is a monad?", 90.0).unwrap();
        assert!(rec.is_swarm, "swarm record must carry the unverified-source flag");

        let _ = db::clear_all();
    }

    #[test]
    fn swarm_inbound_anti_poison_gate_rejects_empty_and_short_content() {
        let _guard = lock_db();
        let _ = db::clear_all();
        let mut app = App::for_screen(Screen::Search);
        let before = app.total_records;

        // 1. Inbound empty content must be dropped by peer client intelligence
        let empty_msg = valid_swarm_message("What is distributed consensus?", "", "OpenAI", "gpt-4o");
        if let Some(p2p) = &app.p2p {
            p2p.simulate_inbound_inference(empty_msg.clone());
        }
        app.tick();
        assert_eq!(app.total_records, before, "empty gossip content must be dropped");

        // 2. Inbound whitespace-only content must be dropped
        let whitespace_msg = valid_swarm_message("What is distributed consensus?", "   \n\t  ", "OpenAI", "gpt-4o");
        if let Some(p2p) = &app.p2p {
            p2p.simulate_inbound_inference(whitespace_msg);
        }
        app.tick();
        assert_eq!(app.total_records, before, "whitespace-only gossip content must be dropped");

        // 3. Inbound content < 10 chars must be dropped
        let short_msg = valid_swarm_message("What is distributed consensus?", "Too short", "OpenAI", "gpt-4o");
        if let Some(p2p) = &app.p2p {
            p2p.simulate_inbound_inference(short_msg);
        }
        app.tick();
        assert_eq!(app.total_records, before, "content under 10 chars must be dropped");

        // 4. Inbound question < 3 chars must be dropped
        let short_q_msg = valid_swarm_message("ab", "Consensus is achieved via Byzantine fault tolerance.", "OpenAI", "gpt-4o");
        if let Some(p2p) = &app.p2p {
            p2p.simulate_inbound_inference(short_q_msg);
        }
        app.tick();
        assert_eq!(app.total_records, before, "question under 3 chars must be dropped");

        // 5. Inbound truncated content must be dropped
        let trunc_msg = valid_swarm_message(
            "What is distributed consensus?",
            "Consensus is...\n\n[⚠️ RESPONSE INCOMPLETE: Timeout]",
            "OpenAI",
            "gpt-4o",
        );
        if let Some(p2p) = &app.p2p {
            p2p.simulate_inbound_inference(trunc_msg);
        }
        app.tick();
        assert_eq!(app.total_records, before, "truncated gossip content must be dropped");

        // 6. Verify Outbound Hard Gate: empty content is never enqueued for broadcast
        if let Some(p2p) = &app.p2p {
            p2p.broadcast_inference(empty_msg);
            assert!(p2p.outbound_inference_tx.is_empty(), "empty inference must be dropped at broadcast gate");
        }

        let _ = db::clear_all();
    }

    #[test]
    fn swarm_inbound_content_safety_and_dlp_gates_reject() {
        let _guard = lock_db();
        let _ = db::clear_all();
        let mut app = App::for_screen(Screen::Search);
        let before = app.total_records;

        // Illegal content must never touch disk.
        let illegal = valid_swarm_message(
            "explosives",
            "How to make a pipe bomb with household items",
            "OpenRouter",
            "openai/gpt-4o",
        );
        if let Some(p2p) = &app.p2p {
            p2p.simulate_inbound_inference(illegal);
        }
        app.tick();
        assert_eq!(app.total_records, before, "prohibited gossip must be dropped");

        // Leaked secret must never touch disk.
        let leaking = valid_swarm_message(
            "keys",
            "Here is my key: sk-proj1234567890abcdefghij",
            "OpenAI",
            "gpt-4o",
        );
        if let Some(p2p) = &app.p2p {
            p2p.simulate_inbound_inference(leaking);
        }
        app.tick();
        assert_eq!(app.total_records, before, "secret-carrying gossip must be dropped");

        let _ = db::clear_all();
    }

    #[test]
    fn swarm_inbound_replay_dedupe_stores_once() {
        let _guard = lock_db();
        let _ = db::clear_all();
        let mut app = App::for_screen(Screen::Search);

        let msg = valid_swarm_message(
            "What is BLAKE3?",
            "A fast cryptographic hash function.",
            "Anthropic",
            "claude-3-5-sonnet",
        );
        for _ in 0..3 {
            if let Some(p2p) = &app.p2p {
                p2p.simulate_inbound_inference(msg.clone());
            }
            app.tick();
        }
        assert_eq!(app.total_records, 1, "replayed gossip must be stored exactly once");

        let _ = db::clear_all();
    }

    #[test]
    fn swarm_query_held_until_jitter_broadcast_moment() {
        let _guard = lock_db();
        let mut app = App::for_screen(Screen::Search);

        app.pending_query = Some(crate::app::PendingSwarmQuery {
            request_id: "jitter-req".to_string(),
            question: "What is jitter?".to_string(),
            simhash: 0xDEADBEEF,
            started_at: std::time::Instant::now(),
            broadcast_at: Some(std::time::Instant::now() + std::time::Duration::from_millis(500)),
        });

        app.tick();
        // Query is held: not dispatched to AI, still pending, still jittered.
        assert!(app.pending_query.is_some());
        assert!(app.pending_query.as_ref().unwrap().broadcast_at.is_some());
        assert!(app.active_stream.is_none(), "must not fall back to AI during jitter hold");
    }

    #[test]
    fn swarm_sourced_viewer_shows_unverified_label() {
        let _guard = lock_db();
        let _ = db::clear_all();
        let mut app = App::for_screen(Screen::Search);

        let msg = valid_swarm_message(
            "What is a vector?",
            "A vector is a quantity with magnitude and direction.",
            "Anthropic",
            "claude-3-5-sonnet",
        );
        if let Some(p2p) = &app.p2p {
            p2p.simulate_inbound_inference(msg);
        }
        app.tick();

        // Ask the same question: L1 hit on the swarm-sourced record.
        app.search_input.insert_str("What is a vector?");
        app.handle_event(key(KeyCode::Enter));

        let viewer = app.viewer.as_ref().expect("viewer opens on L1 hit");
        assert!(viewer.metadata.as_ref().unwrap().is_swarm);
        assert!(
            viewer.content.contains("[SWARM]"),
            "swarm content must carry the unverified-source badge"
        );

        // Render: provenance bar must show "Unverified (swarm)", never the brand.
        let buf = buffer_of(&app);
        let line_text: String = (0..110).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(line_text.contains("Unverified (swarm)"), "bar must show unverified: {line_text}");
        assert!(!line_text.contains("PROVIDER: Anthropic"), "claimed brand must not be shown as verified");

        let _ = db::clear_all();
    }

    #[test]
    fn db_enforces_storage_ceiling_by_pruning_oldest() {
        let _guard = lock_db();
        let _ = db::reseed();
        let before = db::count_records();
        assert!(before >= 100);

        // Tiny ceiling forces pruning of the oldest records.
        // (65 KB sits above SQLite's structural minimum with tombstones table but below the seeded
        // footprint, so pruning must actually occur.)
        // `false` = Blind Swarm semantics: oldest-first eviction.
        let pruned = db::enforce_storage_limit_bytes(65_000, false);
        assert!(pruned > 0, "oldest records must be pruned under a tiny ceiling");
        assert!(db::count_records() < before);

        // Physical footprint (db + wal) must be back under the ceiling.
        let db_file = db::db_path();
        let size = std::fs::metadata(&db_file).map(|m| m.len()).unwrap_or(0)
            + std::fs::metadata(format!("{db_file}-wal")).map(|m| m.len()).unwrap_or(0);
        assert!(size <= 65_000, "db footprint must shrink under the ceiling (got {size})");

        let _ = db::reseed();
    }

    #[test]
    fn enforce_storage_ceiling_is_noop_below_cap() {
        let _guard = lock_db();
        let _ = db::reseed();
        let pruned = db::enforce_storage_limit_bytes(1_000_000_000, true); // 1 GB — far above current size
        assert_eq!(pruned, 0, "no pruning when well below the ceiling");
    }

    // ──────────────────────────────────────────────────────────────
    // Query Locality / storage semantics tests
    // ──────────────────────────────────────────────────────────────

    #[test]
    fn default_reserved_storage_is_1gb() {
        assert_eq!(crate::model::Settings::default().reserved_gb, 1);
        let _guard = lock_db();
        let app = App::for_screen(Screen::Memory);
        assert_eq!(app.settings.reserved_gb, 1, "fresh install defaults to 1 GB");
    }

    #[test]
    fn storage_keeps_unrelated_content_regardless_of_hit_rate() {
        let _guard = lock_db();
        let _ = db::clear_all();
        let before = db::count_records();

        // Store an answer to a question the user has never asked.
        let unrelated_q = "Quantum entanglement teleportation protocols";
        let h = crate::simhash::compute_simhash(unrelated_q);
        let saved = db::save_inference(unrelated_q, "Unrelated answer.", h, "OpenAI", "gpt-4o").expect("valid save");
        assert_eq!(db::count_records(), before + 1);

        // The Hit Rate Threshold gates DISPLAY, not storage: a very different
        // query finds nothing to show, yet the record is still stored.
        let probe = crate::simhash::compute_simhash("How to cook pasta al dente?");
        assert!(
            db::find_best_match_by_hash(probe, 95.0).is_none(),
            "unrelated record must not be surfaced as a direct hit"
        );
        let all = db::load_records();
        assert!(
            all.iter().any(|r| r.question == saved.question),
            "unrelated verified answers must remain stored in the shard"
        );

        let _ = db::clear_all();
    }

    #[test]
    fn locality_profile_ranks_memory_list_by_past_queries() {
        let _guard = lock_db();
        let _ = db::clear_all();
        db::clear_profile();

        // Store an unrelated record first (locality 0 against empty profile).
        let q1 = "Ancient roman aqueduct engineering";
        let h1 = crate::simhash::compute_simhash(q1);
        let _ = db::save_inference(q1, "Answer one.", h1, "OpenAI", "gpt-4o").expect("valid save");

        // Store a second record about black holes.
        let q2 = "What is a black hole?";
        let h2 = crate::simhash::compute_simhash(q2);
        let _ = db::save_inference(q2, "Answer two.", h2, "OpenAI", "gpt-4o").expect("valid save");

        // User asks a black-hole question → profile + re-ranking.
        db::record_profile_query(crate::simhash::compute_simhash("How do black holes form?"));

        let ordered = db::load_records_window(0, 10);
        assert!(!ordered.is_empty());
        // The black-hole record is now most similar to the user's past query.
        assert_eq!(ordered[0].question, q2, "most relevant record must be on top");
        assert!(ordered[0].locality > 0.0, "locality must be scored against the profile");
        assert!(ordered[0].locality > ordered[1].locality, "relevance ordering must hold");
    }

    #[test]
    fn blind_swarm_orders_newest_first() {
        let _guard = lock_db();
        let _ = db::clear_all();
        db::clear_profile();

        let old_q = "Old record question";
        let h_old = crate::simhash::compute_simhash(old_q);
        let _ = db::save_inference(old_q, "Old answer.", h_old, "OpenAI", "gpt-4o").expect("valid save");

        // Sleep a moment so the second record has a strictly newer timestamp.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let new_q = "New record question";
        let h_new = crate::simhash::compute_simhash(new_q);
        let _ = db::save_inference(new_q, "New answer.", h_new, "OpenAI", "gpt-4o").expect("valid save");

        let recent = db::load_records_window_recent(0, 10);
        assert_eq!(recent[0].question, new_q, "Blind Swarm lists newest first");

        let _ = db::clear_all();
    }

    #[test]
    fn locality_eviction_keeps_relevant_records_alive() {
        let _guard = lock_db();
        let _ = db::clear_all();
        db::clear_profile();

        let db_file = db::db_path();
        let now = chrono::Local::now().timestamp();

        // Reset the physical file to its structural minimum so size math is
        // deterministic, then bulk-insert 300 noise rows + 1 relevant row.
        {
            let conn = rusqlite::Connection::open(&db_file).unwrap();
            conn.execute_batch("VACUUM").unwrap();
            let mut stmt = conn
                .prepare(
                    "INSERT INTO inferences (timestamp, similarity, question, content, simhash, provider, model, locality)
                     VALUES (?1, 100.0, ?2, 'noise content', ?3, 'OpenAI', 'gpt-4o', ?4)",
                )
                .unwrap();
            for i in 0..300i64 {
                stmt.execute(rusqlite::params![
                    now - 100_000 - i,
                    format!("noise question {i}"),
                    i as i64,
                    0.0f64 // zero locality
                ])
                .unwrap();
            }
            // The one record the user cares about (high locality, newest).
            let relevant_q = "What is a black hole?";
            let h_r = crate::simhash::compute_simhash(relevant_q);
            stmt.execute(rusqlite::params![
                now,
                relevant_q,
                h_r as i64,
                95.0f64
            ])
            .unwrap();
        }

        let noise_before = db::count_records() - 1;
        assert_eq!(noise_before, 300);

        // Force eviction under a tiny ceiling with Query Locality semantics.
        let dbg_size = std::fs::metadata(&db_file).map(|m| m.len()).unwrap_or(0)
            + std::fs::metadata(format!("{db_file}-wal")).map(|m| m.len()).unwrap_or(0);
        eprintln!("DBG size_before={dbg_size} rows={}", db::count_records());
        let pruned = db::enforce_storage_limit_bytes(45_000, true);
        eprintln!("DBG pruned={pruned} rows_after={} size_after={}", db::count_records(),
            std::fs::metadata(&db_file).map(|m| m.len()).unwrap_or(0)
            + std::fs::metadata(format!("{db_file}-wal")).map(|m| m.len()).unwrap_or(0));
        assert!(pruned > 0, "eviction must prune under the tiny ceiling");

        let remaining = db::load_records();
        assert!(
            remaining.iter().any(|r| r.question == "What is a black hole?"),
            "relevant record must survive locality-aware eviction"
        );
        let noise_after = remaining
            .iter()
            .filter(|r| r.question.starts_with("noise question"))
            .count();
        assert!(
            noise_after < noise_before,
            "least relevant records must be evicted first ({} -> {} noise rows)",
            noise_before,
            noise_after
        );

        let _ = db::clear_all();
    }

    #[test]
    fn sharding_mode_switch_clears_query_profile() {
        let _guard = lock_db();
        let _ = db::clear_all();
        db::clear_profile();

        db::record_profile_query(crate::simhash::compute_simhash("What is a black hole?"));
        assert!(!db::load_profile_hashes().is_empty());

        let mut app = App::for_screen(Screen::Settings);
        while app.focus != SettingsField::ShardingMode {
            app.handle_event(key(KeyCode::Down));
        }
        app.handle_event(key(KeyCode::Right)); // opens purge confirm
        app.handle_event(key(KeyCode::Enter)); // confirm

        assert!(db::load_profile_hashes().is_empty(), "profile must not leak across mode switch");

        let _ = db::reseed();
    }

    #[test]
    fn l1_freshness_filter_skips_stale_records() {
        let _guard = lock_db();
        let _ = db::clear_all();

        // Insert a record with an old (stale) timestamp directly.
        let q = "Stale freshness question?";
        let h = crate::simhash::compute_simhash(q);
        let conn = rusqlite::Connection::open(db::db_path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS inferences (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                similarity REAL NOT NULL,
                question TEXT NOT NULL,
                content TEXT NOT NULL,
                simhash INTEGER NOT NULL DEFAULT 0,
                provider TEXT NOT NULL DEFAULT 'OpenAI',
                model TEXT NOT NULL DEFAULT 'gpt-4o'
            )",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO inferences (timestamp, similarity, question, content, simhash, provider, model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                1_500_000_000i64, // far in the past
                100.0,
                q,
                "Stale answer content.",
                h as i64,
                "OpenAI",
                "gpt-4o"
            ],
        )
        .unwrap();
        drop(conn);

        let now = 1_800_000_000i64;

        // Without freshness: hit.
        assert!(db::find_best_match_by_hash(h, 85.0).is_some());
        // With a freshness window that excludes the stale record: miss.
        assert!(
            db::find_best_match_by_hash_fresh(h, 85.0, Some(now - 86_400)).is_none(),
            "stale records must be filtered when freshness is configured"
        );
        // With `Any time` (None): hit again.
        assert!(db::find_best_match_by_hash_fresh(h, 85.0, None).is_some());

        let _ = db::clear_all();
    }

    #[test]
    fn headless_l1_cache_hit_returns_local_answer() {
        let _guard = lock_db();
        let _ = db::clear_all();
        let q = "How to write unit tests in Rust?";
        let ans = "Use #[test] annotation on test functions.";
        let sim = simhash::compute_simhash(q);
        let _ = db::save_inference(q, ans, sim, "TestProvider", "test-model").expect("valid save");

        let res = headless::execute_ask(q, None);
        assert!(res.is_ok());
        if let Ok(ipc::IpcResponse::Answer { question, content, source, is_swarm, .. }) = res {
            assert_eq!(question, q);
            assert_eq!(content, ans);
            assert!(source.contains("L1"));
            assert!(!is_swarm);
        } else {
            panic!("Expected Answer variant");
        }
        let _ = db::clear_all();
    }

    #[test]
    fn anti_poison_gate_rejects_empty_and_short_content() {
        let _guard = lock_db();
        let _ = db::clear_all();
        // Empty content
        assert!(db::save_inference("What is Rust?", "", 123, "OpenAI", "gpt-4o").is_none());
        // Whitespace only
        assert!(db::save_inference("What is Rust?", "   \n\t  ", 123, "OpenAI", "gpt-4o").is_none());
        // Too short (< 10 chars)
        assert!(db::save_inference("What is Rust?", "Short", 123, "OpenAI", "gpt-4o").is_none());
        // Short question (< 3 chars)
        assert!(db::save_inference("a?", "Valid content that has enough length", 123, "OpenAI", "gpt-4o").is_none());
        // Valid content saves properly
        let saved = db::save_inference("What is Rust?", "Rust is a systems programming language.", 123, "OpenAI", "gpt-4o");
        assert!(saved.is_some());
        let _ = db::clear_all();
    }

    #[test]
    fn tombstone_lifecycle_blocks_search_and_prevents_reinsertion() {
        let _guard = lock_db();
        let _ = db::clear_all();
        db::clear_tombstones();
        let q = "What is cryptographic negative signaling?";
        let c = "Negative signaling uses cryptographic tombstones to prune poisoned data permanently.";
        let sim = crate::simhash::compute_simhash(q);

        let saved = db::save_inference(q, c, sim, "TestProv", "test-model").expect("valid save");
        let hash = saved.content_hash.clone();
        assert!(!hash.is_empty());
        assert!(!db::is_tombstoned(&hash));

        // Delete and tombstone
        let (tomb_hash, _) = db::delete_and_tombstone_record(&saved, "test_deletion");
        assert_eq!(tomb_hash, hash);
        assert!(db::is_tombstoned(&hash));

        // Must not be found in cache
        assert!(db::find_best_match_by_hash(sim, 85.0).is_none());

        // Attempting to save the exact same content again must be rejected by tombstone guard
        let reinsert = db::save_inference(q, c, sim, "TestProv", "test-model");
        assert!(reinsert.is_none(), "tombstoned content must never be re-saved");

        let _ = db::clear_all();
        db::clear_tombstones();
    }

    #[test]
    fn truncated_and_large_inference_local_storage_and_cache_isolation() {
        let _guard = lock_db();
        let _ = db::clear_all();
        db::clear_tombstones();

        let q = "How to make authentic baklava?";
        let c_trunc = "Layer the phyllo dough with melted butter and...\n\n[⚠️ RESPONSE INCOMPLETE: Stream timeout]";
        let sim = crate::simhash::compute_simhash(q);

        // 1. Truncated record saves locally so the user can inspect partial output
        let trunc_saved = db::save_inference_with_truncated(q, c_trunc, sim, "OpenRouter", "deepseek-v4", true);
        assert!(trunc_saved.is_some());
        let record = trunc_saved.unwrap();
        assert!(record.is_truncated);

        // 2. Cache matching MUST ignore truncated records so re-asking fetches a fresh, complete answer
        assert!(db::find_best_match_by_hash(sim, 85.0).is_none(), "truncated record must not hit L1 cache");
        assert!(db::find_best_match_by_hash_fresh(sim, 85.0, Some(0)).is_none(), "truncated record must not hit fresh L1 cache");
        assert!(db::find_best_match_query_fresh(q, 85.0, Some(0)).is_none(), "truncated record must not hit fresh query cache");

        // 3. Swarm integrity check strictly rejects truncated inference payloads
        let mut swarm_trunc = valid_swarm_message(q, c_trunc, "OpenRouter", "deepseek-v4");
        swarm_trunc.is_truncated = true;
        assert!(!swarm_trunc.passes_integrity_checks(swarm_trunc.timestamp), "swarm must reject is_truncated = true");

        // 4. Local storage has NO 128 KB cap — huge answers (e.g. 150 KB) save fine locally
        let q_large = "Give me a 150KB technical manual on distributed database architecture.";
        let sim_large = crate::simhash::compute_simhash(q_large);
        let large_content = format!("Technical Manual\n{}", "Paragraph of architecture details.\n".repeat(4500));
        assert!(large_content.len() > crate::p2p::MAX_GOSSIP_PAYLOAD, "large content must exceed 128 KB");

        let large_saved = db::save_inference(q_large, &large_content, sim_large, "OpenRouter", "deepseek-v4");
        assert!(large_saved.is_some(), "local storage must have no arbitrary 128KB ceiling");
        assert_eq!(large_saved.unwrap().content.len(), large_content.len());

        // L1 cache matches large complete records locally
        let l1_match = db::find_best_match_by_hash(sim_large, 90.0);
        assert!(l1_match.is_some());

        // 5. But P2P Swarm integrity strictly enforces the 128 KB wire ceiling
        let swarm_large = valid_swarm_message(q_large, &large_content, "OpenRouter", "deepseek-v4");
        assert!(!swarm_large.passes_integrity_checks(swarm_large.timestamp), "swarm wire must enforce 128 KB ceiling");

        let _ = db::clear_all();
        db::clear_tombstones();
    }

    #[test]
    fn first_run_terms_modal_and_acceptance() {
        let _guard = lock_db();
        db::set_meta("terms_accepted", "false");

        // 1. Initial launch without terms accepted has terms_modal active and p2p deferred
        let mut app = App::new();
        assert!(app.terms_modal, "First run must show terms modal");
        assert!(app.p2p.is_none(), "P2P network must not connect before terms are accepted");

        // 2. Declining terms with Esc or 'q' flags quit
        app.handle_event(Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        )));
        assert!(app.quit, "Declining terms must quit the application");

        // 3. Accepting terms via Enter or 'y' saves acceptance and initializes P2P
        let mut app2 = App::new();
        assert!(app2.terms_modal);
        app2.handle_event(Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        )));
        assert!(!app2.terms_modal, "Terms modal should dismiss after acceptance");
        assert_eq!(db::get_meta("terms_accepted"), Some("true".to_string()));
        assert!(app2.p2p.is_some(), "P2P network should be running after terms are accepted");

        // 4. Subsequent launch starts directly with terms_modal = false
        let app3 = App::new();
        assert!(!app3.terms_modal, "Subsequent runs must not re-prompt for terms");
        assert!(app3.p2p.is_some());

        // Restore accepted state
        db::set_meta("terms_accepted", "true");
    }

    #[test]
    #[cfg(feature = "publisher")]
    fn web_sync_status_indicator_updates_and_auto_hides() {
        let _guard = lock_db();
        let _cms_guard = crate::cms::CMS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let dir = std::env::temp_dir().join(format!("mbhub_tui_sync_test_{}", std::process::id()));
        let scripts = dir.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("sync.js"), "console.log('FAKE-SYNC-DONE');").unwrap();
        let log = std::env::temp_dir().join(format!("mbhub_tui_sync_{}.log", std::process::id()));
        unsafe {
            std::env::set_var("MBHUB_CMS_DIR", &dir);
            std::env::set_var("MBHUB_CMS_LOG", &log);
        }

        let mut app = App::for_screen(Screen::Memory);
        app.viewer = None;
        app.start_web_sync();

        // The header indicator immediately reports the running pipeline —
        // the screen itself is NOT taken over by a viewer.
        assert_eq!(app.sync_status, Some(app::SyncStatus::Running));
        assert!(app.viewer.is_none(), "sync must not open a viewer");

        // The tick loop flips the indicator to Done once the pipeline exits.
        let mut tries = 0;
        while app.pending_sync.is_some() && tries < 200 {
            app.tick();
            std::thread::sleep(std::time::Duration::from_millis(50));
            tries += 1;
        }
        app.tick();
        match app.sync_status {
            Some(app::SyncStatus::Done { success, .. }) => assert!(success),
            other => panic!("expected Done status, got {other:?}"),
        }

        // After the TTL the indicator clears itself automatically.
        app.sync_status = Some(app::SyncStatus::Done {
            success: true,
            shown_at: std::time::Instant::now() - app::SYNC_STATUS_TTL - std::time::Duration::from_millis(1),
        });
        app.tick();
        assert_eq!(app.sync_status, None, "indicator must auto-hide after the TTL");

        unsafe {
            std::env::remove_var("MBHUB_CMS_DIR");
            std::env::remove_var("MBHUB_CMS_LOG");
        }
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(feature = "publisher")]
    fn blog_export_pipeline_filters_swarm_and_dlp() {
        let _guard = lock_db();
        let _ = db::clear_all();

        // 1. Clean local record -> should be candidate
        let q1 = "How does Raft consensus elect a leader?";
        let c1 = "In Raft, candidate nodes request votes from peers when heartbeat election timeout expires.";
        let sim1 = simhash::compute_simhash(q1);
        let _rec1 = db::save_inference(q1, c1, sim1, "DeepSeek", "deepseek-chat").expect("valid save");

        // 2. Swarm record -> must NOT be candidate (Phase 0 privacy gate)
        let q2 = "How to configure WireGuard VPN?";
        let c2 = "Generate private and public keys using wg genkey and configure interface wg0.";
        let sim2 = simhash::compute_simhash(q2);
        let hash2 = content_hash::compute_content_hash(q2, c2, "SwarmPeer", "model");
        let _rec2 = db::save_swarm_inference(q2, c2, sim2, "SwarmPeer", "model", &hash2).expect("valid save");

        // 3. Local record containing sensitive API key -> should be candidate in DB, but blocked by second-pass DLP in export
        let q3 = "What is my API key?";
        let c3 = "Here is the key: sk-abcdefghijklmnopqrstuvwxyz1234567890.";
        let sim3 = simhash::compute_simhash(q3);
        let _rec3 = db::save_inference(q3, c3, sim3, "DeepSeek", "deepseek-chat").expect("valid save");

        // Verify candidates from DB: only local records (rec1 and rec3), NOT swarm record (rec2)
        let candidates = db::fetch_blog_export_candidates(0, true);
        assert_eq!(candidates.len(), 2, "Only local records (is_swarm=0) should be candidates");
        assert!(candidates.iter().any(|c| c.question == q1));
        assert!(candidates.iter().any(|c| c.question == q3));
        assert!(!candidates.iter().any(|c| c.question == q2));

        // Test export command with a temporary directory
        let temp_dir = std::env::temp_dir().join(format!("mbhub_blog_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let args = vec!["--out".to_string(), temp_dir.to_string_lossy().to_string(), "--all".to_string()];

        let res = handle_cli_export_blog(&args);
        assert!(res.is_ok());

        // Verify exported files:
        // rec1 must be exported
        // rec3 must be skipped due to DLP
        let entries: Vec<_> = std::fs::read_dir(&temp_dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(entries.len(), 1, "Only rec1 should be exported; rec3 blocked by DLP");

        let file_content = std::fs::read_to_string(entries[0].path()).unwrap();
        assert!(file_content.contains(q1));
        assert!(file_content.contains("source: \"L3\""));

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = db::clear_all();
    }
}
