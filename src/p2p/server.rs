//! `mbhub bootstrap` — dedicated rendezvous node for the MBHub network.
//!
//! A minimal, hardened long-running node intended for cheap VPS instances
//! (Oracle Always Free). It carries NO user content and NO database:
//!
//! - Kademlia DHT in **server mode** (`/mbhub/kad/1.0.0`): answers routing
//!   queries so clients can discover each other (every client then acts as
//!   an introducer too — the bootstrap node only starts the chain).
//! - Circuit relay v2 **server**: provides reservations for hard-NAT'd
//!   clients with strict per-peer/per-IP rate limits and tiny circuit
//!   budgets (traffic is coordinated here, but content never transits).
//! - identify + AutoNAT: address discovery and verified reachability.
//!
//! Exploitation caps (all structural, not just rate-limited): bounded
//! reservations, bounded circuits, bounded circuit bytes, bounded
//! connections, per-peer and per-IP rate limiters on both reservations and
//! circuit sources. A hostile client can cost the operator bandwidth
//! budgets, not data or identity.

use std::collections::HashSet;
use std::time::Duration;

use libp2p::futures::StreamExt;
use libp2p::swarm::SwarmEvent;
use libp2p::Multiaddr;
use libp2p_swarm_derive::NetworkBehaviour;

use crate::p2p::bootstrap;
use crate::p2p::identity::load_or_generate_keypair;
use crate::p2p::service::{DEFAULT_LISTEN_PORT, MAX_LOG_BYTES};

/// Connection budget for the public rendezvous node. Generous but bounded:
/// hundreds of concurrent clients, a handful of links per peer.
const SERVER_MAX_CONNECTIONS: u32 = 512;
const SERVER_MAX_CONNECTIONS_PER_PEER: u32 = 4;

/// Circuit relay server configuration: reservations are cheap and
/// long-lived; circuits (relayed data streams) are tiny and short.
///
/// The libp2p default rate limiters (30/min per peer, 60/min per IP for both
/// reservations and circuit sources) are kept — they are the strict limits;
/// the structural caps below are additionally tightened.
fn relay_server_config() -> libp2p::relay::Config {
    let mut config = libp2p::relay::Config::default();
    config.max_reservations = 256;
    config.max_reservations_per_peer = 1;
    config.max_circuits = 64;
    config.max_circuits_per_peer = 4;
    // Gossip payloads are capped at 128 KB; a circuit never needs more than
    // a few multiples of that per hole-punch coordination.
    config.max_circuit_bytes = 1 << 20; // 1 MiB per circuit
    config
}

/// Behaviour composition of the bootstrap node: no gossipsub, no content.
#[derive(NetworkBehaviour)]
#[behaviour(prelude = "libp2p::swarm::derive_prelude")]
struct BootstrapBehaviour {
    kad: libp2p::kad::Behaviour<libp2p::kad::store::MemoryStore>,
    identify: libp2p::identify::Behaviour,
    autonat: libp2p::autonat::Behaviour,
    relay: libp2p::relay::Behaviour,
    limits: libp2p::connection_limits::Behaviour,
}

fn build_bootstrap_swarm(
    keypair: &libp2p::identity::Keypair,
) -> Result<
    libp2p::Swarm<BootstrapBehaviour>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    let peer_id = keypair.public().to_peer_id();

    let mut kad_config = libp2p::kad::Config::new(libp2p::StreamProtocol::new(
        crate::p2p::behaviour::KAD_PROTOCOL_NAME,
    ));
    kad_config.set_query_timeout(Duration::from_secs(10));

    let identify = libp2p::identify::Behaviour::new(
        libp2p::identify::Config::new(
            crate::p2p::behaviour::IDENTIFY_PROTOCOL_VERSION.to_string(),
            keypair.public(),
        )
        .with_agent_version(format!(
            "mbhub-bootstrap/{}",
            env!("CARGO_PKG_VERSION")
        )),
    );

    let limits = libp2p::connection_limits::ConnectionLimits::default()
        .with_max_established(Some(SERVER_MAX_CONNECTIONS))
        .with_max_established_per_peer(Some(SERVER_MAX_CONNECTIONS_PER_PEER))
        .with_max_pending_incoming(Some(SERVER_MAX_CONNECTIONS))
        .with_max_pending_outgoing(Some(SERVER_MAX_CONNECTIONS));

    Ok(libp2p::SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )?
        .with_behaviour(move |_keypair| BootstrapBehaviour {
            kad: libp2p::kad::Behaviour::with_config(
                peer_id,
                libp2p::kad::store::MemoryStore::new(peer_id),
                kad_config,
            ),
            identify,
            autonat: libp2p::autonat::Behaviour::new(
                peer_id,
                libp2p::autonat::Config::default(),
            ),
            relay: libp2p::relay::Behaviour::new(peer_id, relay_server_config()),
            limits: libp2p::connection_limits::Behaviour::new(limits),
        })?
        .build())
}

/// Logs one lifecycle line (same channel/format as the client service).
fn log_line(msg: &str) {
    let path = std::env::var("MBHUB_LOG_FILE")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .or_else(|| std::env::var("USERPROFILE").ok())
                .map(|h| std::path::PathBuf::from(h).join(".mbhub").join("mbhub.log"))
        });

    if let Some(path) = path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_LOG_BYTES {
            let _ = std::fs::write(&path, b"");
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = writeln!(f, "[MBHub Bootstrap] {ts} {msg}");
        }
    }

    if std::env::var("MBHUB_LOG_STDERR").as_deref() == Ok("1") {
        eprintln!("[MBHub Bootstrap] {msg}");
    }
}

/// Runs the bootstrap node in the foreground until Ctrl+C.
///
/// Used by `mbhub bootstrap` on the rendezvous VMs; nothing here touches the
/// local database or the user's memory store.
pub fn run_bootstrap_server() -> Result<(), String> {
    let keypair = load_or_generate_keypair();
    let peer_id = libp2p::PeerId::from(keypair.public());

    log_line(&format!(
        "bootstrap node starting — peer id: {peer_id} (advertise this in bootstrap.json)"
    ));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;

    rt.block_on(async move {
        let mut swarm = build_bootstrap_swarm(&keypair)
            .map_err(|e| format!("swarm construction: {e}"))?;

        swarm
            .behaviour_mut()
            .kad
            .set_mode(Some(libp2p::kad::Mode::Server));

        // Fixed port with ephemeral fallback (same contract as clients).
        let port = std::env::var("MBHUB_LISTEN_PORT")
            .ok()
            .and_then(|p| p.trim().parse::<u16>().ok())
            .unwrap_or(DEFAULT_LISTEN_PORT);
        let primary: Multiaddr = format!("/ip4/0.0.0.0/tcp/{port}").parse().unwrap();
        match swarm.listen_on(primary.clone()) {
            Ok(_) => log_line(&format!("listening on {primary}")),
            Err(e) => {
                log_line(&format!("fixed port {port} unavailable ({e}); using ephemeral"));
                let fallback: Multiaddr = "/ip4/0.0.0.0/tcp/0".parse().unwrap();
                swarm
                    .listen_on(fallback)
                    .map_err(|e| format!("listen: {e}"))?;
            }
        }

        // Interconnect with sibling bootstrap nodes (multi-VM topology).
        let list = bootstrap::resolve();
        log_line(&format!(
            "bootstrap source: {:?} with {} address(es)",
            list.source,
            list.addresses.len()
        ));
        // Confirm our own public address from the manifest: a rendezvous
        // node is public by definition — its address is literally published.
        // Advertising it immediately (a) lets AutoNAT skip the unreliable
        // observed-ephemeral-port probe path and (b) makes this node a valid
        // AutoNAT probe server for clients from the very first minute.
        for addr in &list.addresses {
            let Some(pid) = addr
                .iter()
                .find_map(|p| match p {
                    libp2p::multiaddr::Protocol::P2p(pid) => Some(pid),
                    _ => None,
                })
            else {
                continue;
            };
            if pid == peer_id {
                let external: Multiaddr = addr
                    .iter()
                    .filter(|p| !matches!(p, libp2p::multiaddr::Protocol::P2p(_)))
                    .collect();
                swarm.add_external_address(external.clone());
                log_line(&format!("self external address confirmed: {external}"));
            }
        }

        for addr in &list.addresses {
            // Skip our own entry (nodes find themselves in shared manifests).
            let Some(pid) = addr
                .iter()
                .find_map(|p| match p {
                    libp2p::multiaddr::Protocol::P2p(pid) => Some(pid),
                    _ => None,
                })
            else {
                continue;
            };
            if pid == peer_id {
                continue;
            }
            swarm.behaviour_mut().kad.add_address(&pid, addr.clone());
            if let Err(e) = swarm.dial(addr.clone()) {
                log_line(&format!("sibling dial failed {addr}: {e}"));
            }
        }
        match swarm.behaviour_mut().kad.bootstrap() {
            Ok(_) => log_line("Kademlia bootstrap query started"),
            Err(e) => log_line(&format!("Kademlia bootstrap deferred: {e}")),
        }

        let mut kad_rebootstrap = tokio::time::interval(Duration::from_secs(10 * 60));
        let mut status_log = tokio::time::interval(Duration::from_secs(60));
        let mut known_peers: HashSet<libp2p::PeerId> = HashSet::new();

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    log_line("shutdown signal received");
                    return Ok(());
                }
                _ = kad_rebootstrap.tick() => {
                    let _ = swarm.behaviour_mut().kad.bootstrap();
                }
                _ = status_log.tick() => {
                    log_line(&format!(
                        "status: connections={} known_dht_peers={}",
                        swarm.network_info().num_peers(),
                        known_peers.len()
                    ));
                }
                event = swarm.select_next_some() => {
                    match event {
                        SwarmEvent::NewListenAddr { address, .. } => {
                            log_line(&format!("listening on {address}"));
                        }
                        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                            log_line(&format!("peer connected: {peer_id}"));
                        }
                        SwarmEvent::ConnectionClosed { peer_id, .. } => {
                            known_peers.remove(&peer_id);
                            log_line(&format!("peer disconnected: {peer_id}"));
                        }
                        SwarmEvent::OutgoingConnectionError { error, .. } => {
                            log_line(&format!("dial failed: {error}"));
                        }
                        SwarmEvent::Behaviour(BootstrapBehaviourEvent::Identify(
                            libp2p::identify::Event::Received { peer_id, info, .. },
                        )) => {
                            for addr in &info.listen_addrs {
                                swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
                            }
                        }
                        SwarmEvent::Behaviour(BootstrapBehaviourEvent::Kad(
                            libp2p::kad::Event::RoutingUpdated { peer, is_new_peer, .. },
                        )) => {
                            if is_new_peer {
                                known_peers.insert(peer);
                                log_line(&format!("DHT routing table: +{peer}"));
                            }
                        }
                        SwarmEvent::Behaviour(BootstrapBehaviourEvent::Relay(
                            libp2p::relay::Event::ReservationReqAccepted { src_peer_id, renewed },
                        )) => {
                            log_line(&format!(
                                "relay reservation: {src_peer_id} (renewal: {renewed})"
                            ));
                        }
                        SwarmEvent::Behaviour(BootstrapBehaviourEvent::Autonat(
                            libp2p::autonat::Event::StatusChanged { new, .. },
                        )) => {
                            if let libp2p::autonat::NatStatus::Public(addr) = new {
                                // Only advertise dial-backs that verified our
                                // actual listen port; ephemeral-port verdicts
                                // prove nothing about listener reachability.
                                let port_is_ours = addr.iter().any(|p| {
                                    matches!(
                                        p,
                                        libp2p::multiaddr::Protocol::Tcp(port)
                                            if port == DEFAULT_LISTEN_PORT
                                    )
                                });
                                if port_is_ours {
                                    log_line(&format!("NAT status: PUBLIC ({addr})"));
                                    swarm.add_external_address(addr);
                                } else {
                                    log_line(&format!(
                                        "NAT probe verified a non-listen port ({addr}) — not advertising"
                                    ));
                                }
                            } else {
                                log_line(&format!("NAT status: {new:?}"));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    })
}
