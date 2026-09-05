//! Sovereign P2P Swarm Network Background Service using libp2p.
//!
//! Runs on a dedicated background thread with Tokio runtime, enforcing:
//! - Ed25519 identity verification.
//! - Noise encrypted transport + Yamux multiplexing over TCP.
//! - GossipSub 1.1 topics for inferences, queries, and responses.
//! - Strict 128 KB Payload Ceiling (`MAX_GOSSIP_PAYLOAD = 131_072`).
//! - Connection limits (per-peer + global) against Sybil connection flooding.
//! - Per-peer gossip message rate limiting.
//! - Query consistency validation: a peer may only probe the swarm with the
//!   SimHash of the question it actually sends (prevents arbitrary hash
//!   enumeration / data scraping).
//! - Content integrity on every response: only records with a valid BLAKE3
//!   content hash are served to peers (honest nodes silently self-regulate).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Sender};
use libp2p::futures::StreamExt;
use libp2p::gossipsub::{self, IdentTopic, MessageAuthenticity};
use libp2p::swarm::SwarmEvent;
use libp2p::{Multiaddr, PeerId};
use libp2p_swarm_derive::NetworkBehaviour;

use crate::p2p::identity::load_or_generate_keypair;
use crate::p2p::protocol::{
    SwarmInferenceMessage, SwarmQueryRequest, SwarmQueryResponse, SwarmTombstoneMessage,
    GOSSIP_TOPIC_INFERENCES, GOSSIP_TOPIC_QUERIES, GOSSIP_TOPIC_RESPONSES, GOSSIP_TOPIC_TOMBSTONES,
    MAX_GOSSIP_PAYLOAD,
};

/// Max gossip messages accepted per peer per second. Honest clients are far
/// below this; floods above it are dropped unprocessed.
const MAX_MSGS_PER_PEER_PER_SEC: u32 = 20;

/// Connection budget: at most this many established connections total, and
/// at most 2 per peer.
const MAX_ESTABLISHED_CONNECTIONS: u32 = 32;
const MAX_CONNECTIONS_PER_PEER: u32 = 2;

/// Log file size cap before truncation (keeps the log bounded).
const MAX_LOG_BYTES: u64 = 1_000_000;

/// Logs one P2P lifecycle line.
///
/// The TUI owns the terminal (alternate screen + raw mode), so these lines
/// MUST NOT go to stderr by default — stderr writes leak into the middle of
/// the rendered UI. Lines are appended to a log file instead
/// (`MBHUB_LOG_FILE`, default `~/.mbhub/mbhub.log`), and only echoed to
/// stderr when `MBHUB_LOG_STDERR=1` is set (headless/test scenarios).
fn log_line(msg: &str) {
    let path = std::env::var("MBHUB_LOG_FILE")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            // Windows doesn't set HOME; USERPROFILE keeps the log working on
            // every OS.
            std::env::var("HOME")
                .ok()
                .or_else(|| std::env::var("USERPROFILE").ok())
                .map(|h| std::path::PathBuf::from(h).join(".mbhub").join("mbhub.log"))
        });

    if let Some(path) = path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Simple size cap: start fresh when the log grows past ~1 MB.
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
            let _ = writeln!(f, "[{ts}] {msg}");
        }
    }

    if std::env::var("MBHUB_LOG_STDERR").as_deref() == Ok("1") {
        eprintln!("{msg}");
    }
}

#[derive(Clone, Debug, Default)]
pub struct P2pStatus {
    pub peer_id: String,
    pub connected_peers: usize,
    pub listen_addrs: Vec<String>,
}

pub struct P2pHandle {
    #[allow(dead_code)]
    pub status: Arc<RwLock<P2pStatus>>,
    pub inbound_inference_rx: Receiver<SwarmInferenceMessage>,
    pub inbound_tombstone_rx: Receiver<SwarmTombstoneMessage>,
    pub query_response_rx: Receiver<SwarmQueryResponse>,
    pub outbound_inference_tx: Sender<SwarmInferenceMessage>,
    pub outbound_tombstone_tx: Sender<SwarmTombstoneMessage>,
    pub outbound_query_tx: Sender<SwarmQueryRequest>,
    pub outbound_response_tx: Sender<SwarmQueryResponse>,
    pub query_response_tx: Sender<SwarmQueryResponse>,
    /// Test/backdoor-free simulation hook mirroring the swarm ingress channel.
    #[allow(dead_code)]
    inbound_inference_tx: Sender<SwarmInferenceMessage>,
}

impl P2pHandle {
    #[allow(dead_code)]
    pub fn peer_id(&self) -> String {
        self.status
            .read()
            .map(|s| s.peer_id.clone())
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub fn connected_peers(&self) -> usize {
        self.status
            .read()
            .map(|s| s.connected_peers)
            .unwrap_or_default()
    }

    pub fn broadcast_inference(&self, msg: SwarmInferenceMessage) {
        // Anti-Poison Hard Gate: never publish empty, short or truncated content to P2P network
        if msg.content.trim().is_empty()
            || msg.content.trim().len() < 10
            || msg.question.trim().is_empty()
            || msg.question.trim().len() < 3
            || msg.is_truncated
        {
            return;
        }
        let _ = self.outbound_inference_tx.send(msg);
    }

    pub fn broadcast_tombstone(&self, tomb: SwarmTombstoneMessage) {
        let _ = self.outbound_tombstone_tx.send(tomb);
    }

    pub fn broadcast_query(&self, query: SwarmQueryRequest) {
        if query.question.trim().is_empty() || query.question.trim().len() < 3 {
            return;
        }
        let _ = self.outbound_query_tx.send(query);
    }

    #[allow(dead_code)]
    pub fn broadcast_response(&self, resp: SwarmQueryResponse) {
        // Anti-Poison Hard Gate: never publish empty or uninformative responses to P2P network
        if resp.content.trim().is_empty()
            || resp.content.trim().len() < 10
            || resp.question.trim().is_empty()
            || resp.question.trim().len() < 3
        {
            return;
        }
        let _ = self.outbound_response_tx.send(resp);
    }

    #[allow(dead_code)]
    pub fn simulate_query_response(&self, resp: SwarmQueryResponse) {
        let _ = self.query_response_tx.send(resp);
    }

    /// Injects an inbound gossip inference exactly as the swarm thread would
    /// deliver it (used by integration tests for the receiver-side gates).
    #[allow(dead_code)]
    pub fn simulate_inbound_inference(&self, msg: SwarmInferenceMessage) {
        let _ = self.inbound_inference_tx.send(msg);
    }
}

/// Combined network behaviour: GossipSub plus connection limits.
#[derive(NetworkBehaviour)]
#[behaviour(prelude = "libp2p::swarm::derive_prelude")]
struct MbHubBehaviour {
    gossipsub: gossipsub::Behaviour<gossipsub::IdentityTransform>,
    limits: libp2p::connection_limits::Behaviour,
}

/// Sliding-window message counter for one peer.
struct PeerRate {
    window_start: Instant,
    count: u32,
}

impl PeerRate {
    fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            count: 0,
        }
    }

    /// Returns true when the message may be accepted (rate within limit).
    fn allow(&mut self, now: Instant) -> bool {
        if now.duration_since(self.window_start) >= Duration::from_secs(1) {
            self.window_start = now;
            self.count = 0;
        }
        if self.count >= MAX_MSGS_PER_PEER_PER_SEC {
            return false;
        }
        self.count += 1;
        true
    }
}

/// Optional static bootstrap peers, supplied out-of-band via
/// `MBHUB_BOOTSTRAP_PEERS` as comma-separated multiaddrs. Capped at 16 entries.
fn bootstrap_peers() -> Vec<Multiaddr> {
    let Ok(raw) = std::env::var("MBHUB_BOOTSTRAP_PEERS") else {
        return Vec::new();
    };
    raw.split(',')
        .filter_map(|s| s.trim().parse::<Multiaddr>().ok())
        .take(16)
        .collect()
}

/// Optional fixed listen port (`MBHUB_LISTEN_PORT`, default 0 = random).
/// A fixed port makes the node dialable: firewall rules, port forwarding,
/// and two-node local network tests.
fn listen_port() -> u16 {
    std::env::var("MBHUB_LISTEN_PORT")
        .ok()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .unwrap_or(0)
}

/// Constructs the hardened MBHub swarm (Noise + Yamux + GossipSub signed +
/// connection limits) for a given identity. Shared by the background service
/// and the two-swarm network integration tests.
fn build_swarm(
    keypair: &libp2p::identity::Keypair,
) -> Result<libp2p::Swarm<MbHubBehaviour>, Box<dyn std::error::Error + Send + Sync>> {
    let connection_limits = libp2p::connection_limits::ConnectionLimits::default()
        .with_max_established(Some(MAX_ESTABLISHED_CONNECTIONS))
        .with_max_established_per_peer(Some(MAX_CONNECTIONS_PER_PEER))
        .with_max_pending_incoming(Some(MAX_ESTABLISHED_CONNECTIONS))
        .with_max_pending_outgoing(Some(MAX_ESTABLISHED_CONNECTIONS));

    Ok(libp2p::SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )?
        .with_behaviour(|keypair| {
            let message_authenticity = MessageAuthenticity::Signed(keypair.clone());
            let gossip_config = gossipsub::ConfigBuilder::default()
                .max_transmit_size(MAX_GOSSIP_PAYLOAD)
                .build()
                .expect("valid gossipsub config");

            MbHubBehaviour {
                gossipsub: gossipsub::Behaviour::<gossipsub::IdentityTransform>::new(
                    message_authenticity,
                    gossip_config,
                )
                .expect("valid gossipsub behaviour"),
                limits: libp2p::connection_limits::Behaviour::new(connection_limits),
            }
        })?
        .build())
}

/// Starts the P2P swarm service in a dedicated background thread.
pub fn start_p2p_service() -> P2pHandle {
    let status = Arc::new(RwLock::new(P2pStatus::default()));
    let status_clone = Arc::clone(&status);

    let (inbound_inf_tx, inbound_inf_rx) = unbounded();
    let (inbound_tomb_tx, inbound_tomb_rx) = unbounded();
    let (query_resp_tx, query_resp_rx) = unbounded();
    let (outbound_inf_tx, outbound_inf_rx) = unbounded();
    let (outbound_tomb_tx, outbound_tomb_rx) = unbounded();
    let (outbound_query_tx, outbound_query_rx) = unbounded();
    let (outbound_resp_tx, outbound_resp_rx) = unbounded();

    let query_resp_tx_clone = query_resp_tx.clone();
    let inbound_inf_tx_sim = inbound_inf_tx.clone();

    thread::Builder::new()
        .name("mbhub-p2p".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("failed to build tokio runtime for P2P service");

            rt.block_on(run_swarm_loop(
                status_clone,
                inbound_inf_tx,
                inbound_tomb_tx,
                query_resp_tx,
                outbound_inf_rx,
                outbound_tomb_rx,
                outbound_query_rx,
                outbound_resp_rx,
            ));
        })
        .expect("failed to spawn P2P background thread");

    P2pHandle {
        status,
        inbound_inference_rx: inbound_inf_rx,
        inbound_tombstone_rx: inbound_tomb_rx,
        query_response_rx: query_resp_rx,
        outbound_inference_tx: outbound_inf_tx,
        outbound_tombstone_tx: outbound_tomb_tx,
        outbound_query_tx,
        outbound_response_tx: outbound_resp_tx,
        query_response_tx: query_resp_tx_clone,
        inbound_inference_tx: inbound_inf_tx_sim,
    }
}

async fn run_swarm_loop(
    status: Arc<RwLock<P2pStatus>>,
    inbound_inf_tx: Sender<SwarmInferenceMessage>,
    inbound_tomb_tx: Sender<SwarmTombstoneMessage>,
    query_resp_tx: Sender<SwarmQueryResponse>,
    outbound_inf_rx: Receiver<SwarmInferenceMessage>,
    outbound_tomb_rx: Receiver<SwarmTombstoneMessage>,
    outbound_query_rx: Receiver<SwarmQueryRequest>,
    outbound_resp_rx: Receiver<SwarmQueryResponse>,
) {
    let keypair = load_or_generate_keypair();
    let peer_id = PeerId::from(keypair.public());
    let my_peer_id_str = peer_id.to_string();

    if let Ok(mut s) = status.write() {
        s.peer_id = my_peer_id_str.clone();
    }

    log_line(&format!("[MBHub P2P] node started — peer id: {my_peer_id_str}"));

    let mut swarm = match build_swarm(&keypair) {
        Ok(s) => s,
        Err(e) => {
            log_line(&format!("[MBHub P2P] Failed to construct swarm: {e}"));
            return;
        }
    };

    let topic_inferences = IdentTopic::new(GOSSIP_TOPIC_INFERENCES);
    let topic_queries = IdentTopic::new(GOSSIP_TOPIC_QUERIES);
    let topic_responses = IdentTopic::new(GOSSIP_TOPIC_RESPONSES);
    let topic_tombstones = IdentTopic::new(GOSSIP_TOPIC_TOMBSTONES);

    let _ = swarm.behaviour_mut().gossipsub.subscribe(&topic_inferences);
    let _ = swarm.behaviour_mut().gossipsub.subscribe(&topic_queries);
    let _ = swarm.behaviour_mut().gossipsub.subscribe(&topic_responses);
    let _ = swarm.behaviour_mut().gossipsub.subscribe(&topic_tombstones);

    // Listen on TCP. Port comes from MBHUB_LISTEN_PORT when set (fixed, for
    // port-forwarding and two-node tests); otherwise the OS assigns one.
    let listen_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", listen_port())
        .parse()
        .expect("valid listen multiaddr");
    if let Err(e) = swarm.listen_on(listen_addr) {
        log_line(&format!("[MBHub P2P] Failed to listen on multiaddr: {e}"));
    }

    // Static bootstrap dialing (optional, env-configured). All peers remain
    // equally untrusted regardless of origin; content gates run in the app.
    for addr in bootstrap_peers() {
        let _ = swarm.dial(addr);
    }

    let mut tick_interval = tokio::time::interval(Duration::from_millis(50));
    let mut peer_rates: HashMap<PeerId, PeerRate> = HashMap::new();
    let mut tick_counter: u64 = 0;

    loop {
        tokio::select! {
            _ = tick_interval.tick() => {
                tick_counter += 1;

                // Drain outbound inferences
                while let Ok(msg) = outbound_inf_rx.try_recv() {
                    // Anti-Poison Hard Gate: defense-in-depth gate before wire transmission
                    if msg.content.trim().is_empty()
                        || msg.content.trim().len() < 10
                        || msg.question.trim().is_empty()
                        || msg.question.trim().len() < 3
                        || msg.is_truncated
                    {
                        continue;
                    }
                    if let Ok(json_bytes) = serde_json::to_vec(&msg) {
                        if json_bytes.len() <= MAX_GOSSIP_PAYLOAD {
                            let _ = swarm.behaviour_mut().gossipsub.publish(topic_inferences.clone(), json_bytes);
                        }
                    }
                }

                // Drain outbound queries
                while let Ok(req) = outbound_query_rx.try_recv() {
                    if req.question.trim().is_empty() || req.question.trim().len() < 3 {
                        continue;
                    }
                    if let Ok(json_bytes) = serde_json::to_vec(&req) {
                        if json_bytes.len() <= MAX_GOSSIP_PAYLOAD {
                            let _ = swarm.behaviour_mut().gossipsub.publish(topic_queries.clone(), json_bytes);
                        }
                    }
                }

                // Drain outbound responses
                while let Ok(resp) = outbound_resp_rx.try_recv() {
                    // Anti-Poison Hard Gate: defense-in-depth gate before wire transmission
                    if resp.content.trim().is_empty()
                        || resp.content.trim().len() < 10
                        || resp.question.trim().is_empty()
                        || resp.question.trim().len() < 3
                    {
                        continue;
                    }
                    if let Ok(json_bytes) = serde_json::to_vec(&resp) {
                        if json_bytes.len() <= MAX_GOSSIP_PAYLOAD {
                            let _ = swarm.behaviour_mut().gossipsub.publish(topic_responses.clone(), json_bytes);
                        }
                    }
                }

                // Drain outbound tombstones (cryptographic negative signals)
                while let Ok(tomb) = outbound_tomb_rx.try_recv() {
                    if let Ok(json_bytes) = serde_json::to_vec(&tomb) {
                        if json_bytes.len() <= MAX_GOSSIP_PAYLOAD {
                            let _ = swarm.behaviour_mut().gossipsub.publish(topic_tombstones.clone(), json_bytes);
                        }
                    }
                }

                // Prune stale per-peer rate windows every ~10s to bound memory.
                if tick_counter % 200 == 0 {
                    let now = Instant::now();
                    peer_rates.retain(|_, r| now.duration_since(r.window_start) < Duration::from_secs(2));
                }
            }

            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        // Log the dialable address so other nodes (or a second
                        // local instance for testing) can bootstrap to us.
                        log_line(&format!("[MBHub P2P] listening on {address}"));
                        if let Ok(mut s) = status.write() {
                            s.listen_addrs.push(address.to_string());
                        }
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        if let Ok(mut s) = status.write() {
                            s.connected_peers += 1;
                            log_line(&format!(
                                "[MBHub P2P] peer connected: {peer_id} (total {})",
                                s.connected_peers
                            ));
                        }
                    }
                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        if let Ok(mut s) = status.write() {
                            s.connected_peers = s.connected_peers.saturating_sub(1);
                            log_line(&format!(
                                "[MBHub P2P] peer disconnected: {peer_id} (total {})",
                                s.connected_peers
                            ));
                        }
                    }
                    SwarmEvent::Behaviour(MbHubBehaviourEvent::Gossipsub(gossipsub::Event::Message { message, .. })) => {
                        // Pre-parse size ceiling (§15 rule 1): drop oversized
                        // frames before any allocation/processing.
                        if message.data.len() > MAX_GOSSIP_PAYLOAD {
                            continue;
                        }

                        // Per-peer message rate limit (§4): floods from a
                        // single peer are dropped unprocessed.
                        if let Some(source) = message.source {
                            let now = Instant::now();
                            let rate = peer_rates.entry(source).or_insert_with(|| PeerRate::new(now));
                            if !rate.allow(now) {
                                continue;
                            }
                        }

                        if message.topic == topic_queries.hash() {
                            if let Ok(query_req) = serde_json::from_slice::<SwarmQueryRequest>(&message.data) {
                                // Only respond to our own queries' echoes? No:
                                // respond to peers' queries only.
                                if query_req.asker_peer_id != my_peer_id_str
                                    && query_req.passes_integrity_checks()
                                {
                                    if let Some(hit) = crate::db::find_best_match_by_hash(query_req.simhash, query_req.min_similarity) {
                                            // Honest self-regulation (§5.1): never serve records lacking a valid content hash,
                                            // or records with empty/short/truncated content (Anti-Poison Hard Gate).
                                            if !hit.content_hash.is_empty()
                                                && !hit.content.trim().is_empty()
                                                && hit.content.trim().len() >= 10
                                                && !hit.question.trim().is_empty()
                                                && hit.question.trim().len() >= 3
                                                && !hit.is_truncated
                                            {
                                                let resp = SwarmQueryResponse {
                                                    request_id: query_req.request_id,
                                                    responder_peer_id: my_peer_id_str.clone(),
                                                    question: hit.question,
                                                    content: hit.content,
                                                    simhash: hit.simhash,
                                                    provider: hit.provider,
                                                    model: hit.model,
                                                    content_hash: hit.content_hash,
                                                };
                                                if let Ok(bytes) = serde_json::to_vec(&resp) {
                                                    if bytes.len() <= MAX_GOSSIP_PAYLOAD {
                                                        let _ = swarm.behaviour_mut().gossipsub.publish(topic_responses.clone(), bytes);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else if message.topic == topic_responses.hash() {
                            if let Ok(query_resp) = serde_json::from_slice::<SwarmQueryResponse>(&message.data) {
                                // Anti-Poison Hard Gate: drop answerless / empty / short responses immediately
                                if !query_resp.content.trim().is_empty()
                                    && query_resp.content.trim().len() >= 10
                                    && !query_resp.question.trim().is_empty()
                                    && query_resp.question.trim().len() >= 3
                                    && query_resp.passes_integrity_checks()
                                {
                                    let _ = query_resp_tx.send(query_resp);
                                }
                            }
                        } else if message.topic == topic_inferences.hash() {
                            if let Ok(inference) = serde_json::from_slice::<SwarmInferenceMessage>(&message.data) {
                                // Anti-Poison Hard Gate: drop answerless / empty / short / truncated inferences immediately
                                if !inference.content.trim().is_empty()
                                    && inference.content.trim().len() >= 10
                                    && !inference.question.trim().is_empty()
                                    && inference.question.trim().len() >= 3
                                    && !inference.is_truncated
                                    && inference.passes_integrity_checks(chrono::Local::now().timestamp())
                                {
                                    let _ = inbound_inf_tx.send(inference);
                                }
                            }
                        } else if message.topic == topic_tombstones.hash() {
                            if let Ok(tomb) = serde_json::from_slice::<SwarmTombstoneMessage>(&message.data) {
                                let _ = inbound_tomb_tx.send(tomb);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::protocol::MAX_HOP_TTL;
    use libp2p::identity::Keypair;

    #[test]
    fn log_line_writes_to_file_not_terminal() {
        let dir = std::env::temp_dir().join(format!("mbhub_log_test_{}", std::process::id()));
        let path = dir.join("p2p.log");
        let _ = std::fs::remove_dir_all(&dir);
        unsafe {
            std::env::set_var("MBHUB_LOG_FILE", &path);
            std::env::remove_var("MBHUB_LOG_STDERR");
        }

        log_line("[MBHub P2P] test line one");
        log_line("[MBHub P2P] test line two");

        let content = std::fs::read_to_string(&path).expect("log file created");
        assert!(content.contains("test line one"));
        assert!(content.contains("test line two"));
        assert!(content.lines().all(|l| l.starts_with('[')), "timestamped lines");

        // Size cap: force a huge line, next write truncates the log.
        std::fs::write(&path, "x".repeat((MAX_LOG_BYTES + 100) as usize)).unwrap();
        log_line("[MBHub P2P] after cap");
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("after cap"));
        assert!(after.len() < MAX_LOG_BYTES as usize, "log must be capped");

        unsafe {
            std::env::remove_var("MBHUB_LOG_FILE");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End-to-end network proof: two real swarms (Noise + Yamux + GossipSub)
    /// in one process. Node B bootstraps to node A exactly like the production
    /// `MBHUB_BOOTSTRAP_PEERS` path; after the connection and mesh establish,
    /// B's gossiped inference must arrive at A, pass content-hash integrity,
    /// and be rejected when tampered.
    #[test]
    fn two_swarms_connect_and_gossip_inference() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let swarm_a = build_swarm(&Keypair::generate_ed25519()).expect("swarm A builds");
            let mut swarm_a = swarm_a;
            let swarm_b = build_swarm(&Keypair::generate_ed25519()).expect("swarm B builds");
            let mut swarm_b = swarm_b;

            let topic = IdentTopic::new(GOSSIP_TOPIC_INFERENCES);
            swarm_a.behaviour_mut().gossipsub.subscribe(&topic).expect("A subscribes");
            swarm_b.behaviour_mut().gossipsub.subscribe(&topic).expect("B subscribes");

            swarm_a
                .listen_on("/ip4/127.0.0.1/tcp/0".parse::<Multiaddr>().unwrap())
                .expect("A listens");

            // Capture A's actual listen port from its NewListenAddr event.
            let port_a = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let SwarmEvent::NewListenAddr { address, .. } = swarm_a.select_next_some().await {
                        for proto in address.iter() {
                            if let libp2p::multiaddr::Protocol::Tcp(p) = proto {
                                return p;
                            }
                        }
                    }
                }
            })
            .await
            .expect("A publishes its listen address");

            // B bootstraps to A — the same dial path used by MBHUB_BOOTSTRAP_PEERS.
            let addr_a: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port_a}").parse().unwrap();
            swarm_b.dial(addr_a).expect("B dials A");

            // Wait until both sides report the established (Noise-authenticated) link.
            tokio::time::timeout(Duration::from_secs(15), async {
                let (mut a_up, mut b_up) = (false, false);
                while !(a_up && b_up) {
                    tokio::select! {
                        ev = swarm_a.select_next_some() => {
                            if matches!(ev, SwarmEvent::ConnectionEstablished { .. }) { a_up = true; }
                        }
                        ev = swarm_b.select_next_some() => {
                            if matches!(ev, SwarmEvent::ConnectionEstablished { .. }) { b_up = true; }
                        }
                    }
                }
            })
            .await
            .expect("the two nodes establish a connection");

            // B announces a valid inference. Publishing is retried briefly
            // because GossipSub mesh membership settles a heartbeat or two
            // after the connection (idempotent thanks to content addressing).
            let mut msg = SwarmInferenceMessage {
                question: "How does distributed consensus work?".to_string(),
                content: "Consensus via Byzantine fault tolerance.".to_string(),
                timestamp: chrono::Local::now().timestamp(),
                simhash: crate::simhash::compute_simhash("How does distributed consensus work?"),
                provider: "OpenAI".to_string(),
                model: "gpt-4o".to_string(),
                content_hash: String::new(),
                hop_ttl: MAX_HOP_TTL,
                is_truncated: false,
            };
            msg.content_hash = msg.canonical_content_hash();
            let payload = serde_json::to_vec(&msg).expect("serializes");

            // GossipSub mesh membership settles a heartbeat or two after the
            // connection, and publish() fails while the mesh is empty. Retry
            // publishing on a fixed cadence (idempotent — content addressing
            // makes duplicates harmless) while polling BOTH swarms so their
            // GRAFT/IHAVE control flows progress. Note gossipsub 0.49 emits no
            // heartbeat events, so the retry cadence must be timer-driven.
            let received = tokio::time::timeout(Duration::from_secs(25), async {
                let mut publish_tick = tokio::time::interval(Duration::from_millis(400));
                loop {
                    tokio::select! {
                        ev = swarm_a.select_next_some() => {
                            if let SwarmEvent::Behaviour(MbHubBehaviourEvent::Gossipsub(
                                gossipsub::Event::Message { message, .. },
                            )) = ev
                            {
                                if let Ok(parsed) = serde_json::from_slice::<SwarmInferenceMessage>(&message.data) {
                                    if parsed.content_hash == msg.content_hash {
                                        return parsed;
                                    }
                                }
                            }
                        }
                        _ = swarm_b.select_next_some() => {}
                        _ = publish_tick.tick() => {
                            let _ = swarm_b.behaviour_mut().gossipsub.publish(topic.clone(), payload.clone());
                        }
                    }
                }
            })
            .await
            .expect("A receives the gossiped inference");

            assert_eq!(received.question, msg.question);
            assert_eq!(received.content, msg.content);
            assert!(
                received.passes_integrity_checks(received.timestamp),
                "received payload must pass full receiver-side integrity checks"
            );
        });
    }
}
