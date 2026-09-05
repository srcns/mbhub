//! Background daemon service for MBHub.
//!
//! Runs 24/7 as a system service without a TUI:
//! - Keeps libp2p swarm connection alive across the mesh.
//! - Serves incoming peer queries (L2 provider).
//! - Accepts incoming gossip inferences and stores verified knowledge.
//! - Listens on local IPC socket (`~/.mbhub/mbhub.sock`) for local CLI/MCP requests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::ipc::{IpcRequest, IpcResponse, ServerListener};
use crate::model::{Settings, ShardingMode};

/// Runs the 24/7 background daemon loop.
pub fn run_daemon(accept_terms: bool) -> std::io::Result<()> {
    if accept_terms {
        crate::db::set_meta("terms_accepted", "true");
        eprintln!("[daemon] MBHub Terms of Service accepted via CLI flag.");
    }

    if crate::db::get_meta("terms_accepted") != Some("true".to_string()) {
        eprintln!("Error: MBHub Terms of Service have not been accepted yet.");
        eprintln!("Please launch `mbhub` once in your terminal to review and accept the Terms of Service, or run `mbhub daemon --accept-terms`.");
        std::process::exit(1);
    }

    eprintln!("MBHub daemon starting in background...");

    // Enforce storage quota at startup
    let settings = Settings::load();
    let locality_first = settings.sharding_mode == ShardingMode::QueryLocality;
    let _ = crate::db::enforce_storage_limit_gb(settings.reserved_gb, locality_first);

    // Initialize P2P Swarm Network
    let p2p = Arc::new(crate::p2p::start_p2p_service());
    let listener = ServerListener::bind()?;

    let running = Arc::new(AtomicBool::new(true));

    eprintln!("MBHub daemon active and listening on IPC socket.");

    // Worker thread for P2P inbound gossip digestion
    let p2p_bg = p2p.clone();
    let r = running.clone();
    let inbound_thread = thread::spawn(move || {
        while r.load(Ordering::Relaxed) {
            let now_epoch = chrono::Local::now().timestamp();

            // 1. Drain inbound inferences
            while let Ok(inf) = p2p_bg.inbound_inference_rx.try_recv() {
                // Anti-Poison Hard Gate: drop answerless, empty, short or truncated inferences
                if inf.content.trim().is_empty()
                    || inf.content.trim().len() < 10
                    || inf.question.trim().is_empty()
                    || inf.question.trim().len() < 3
                    || inf.is_truncated
                {
                    continue;
                }
                if inf.passes_integrity_checks(now_epoch)
                    && !crate::dlp::scan_text(&inf.content).is_sensitive
                    && crate::content_safety::screen_text(&inf.content).is_allowed()
                    && !crate::db::is_tombstoned(&inf.content_hash)
                {
                    crate::db::save_swarm_inference(
                        &inf.question,
                        &inf.content,
                        inf.simhash,
                        &inf.provider,
                        &inf.model,
                        &inf.content_hash,
                    );
                }
            }

            // 2. Drain inbound tombstones (negative signals)
            while let Ok(tomb) = p2p_bg.inbound_tombstone_rx.try_recv() {
                if tomb.passes_integrity_checks(now_epoch) {
                    crate::db::add_tombstone(&tomb.content_hash, tomb.simhash, &tomb.reason);
                }
            }

            thread::sleep(Duration::from_millis(50));
        }
    });

    // Main thread: IPC Connection Acceptance Loop
    while running.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok(mut stream) => {
                let p2p_clone = p2p.clone();
                thread::spawn(move || {
                    if let Ok(req) = stream.read_request() {
                        let resp = match req {
                            IpcRequest::Ping => IpcResponse::Pong,
                            IpcRequest::Status => {
                                let s = Settings::load();
                                IpcResponse::Status {
                                    running: true,
                                    peers: p2p_clone.connected_peers(),
                                    reserved_gb: s.reserved_gb,
                                    records: crate::db::count_records(),
                                }
                            }
                            IpcRequest::Ask { query } => {
                                match crate::headless::execute_ask(&query, Some(&p2p_clone)) {
                                    Ok(resp) => resp,
                                    Err(err) => IpcResponse::Error(err),
                                }
                            }
                        };
                        let _ = stream.write_response(&resp);
                    }
                });
            }
            Err(_) => {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }

    let _ = inbound_thread.join();
    eprintln!("MBHub daemon stopped.");
    Ok(())
}
