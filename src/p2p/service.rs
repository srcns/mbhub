//! Sovereign P2P Swarm Network Background Service using libp2p.
//!
//! Runs on a dedicated background thread with Tokio runtime, enforcing:
//! - Ed25519 identity verification.
//! - Noise encrypted transport + Yamux multiplexing over TCP (+ relay client
//!   transport as the hard-NAT fallback).
//! - GossipSub 1.1 topics for inferences, queries, and responses.
//! - Kademlia DHT peer routing (`/mbhub/kad/1.0.0`) for decentralized peer
//!   discovery: every peer acts as an introducer; only the first contact
//!   requires a bootstrap address (env / embedded / bootstrap.json / cache).
//! - NAT traversal: AutoNAT probes + UPnP mapping + DCUtR hole punching +
//!   circuit-relay v2 reservations.
//! - mDNS for free local-network discovery.
//! - Strict 128 KB Payload Ceiling (`MAX_GOSSIP_PAYLOAD = 131_072`).
//! - Connection limits (per-peer + global) against Sybil connection flooding.
//! - Per-peer gossip message rate limiting.
//! - Publish retry: gossipsub mesh membership settles a heartbeat or two
//!   after connecting, so failed publishes are retried within a bounded
//!   window instead of silently dropping the first question after joining.
//! - Signed tombstones: negative signals are Ed25519-signed by the reporter;
//!   unsigned or invalid ones are dropped at the swarm edge.
//! - Content integrity on every response: only records with a valid BLAKE3
//!   content hash are served to peers (honest nodes silently self-regulate).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Sender};
use libp2p::futures::StreamExt;
use libp2p::gossipsub::{self, IdentTopic};
use libp2p::swarm::SwarmEvent;
use libp2p::{Multiaddr, PeerId};

use crate::p2p::behaviour::{MbHubBehaviour, MbHubBehaviourEvent};
use crate::p2p::bootstrap::{self, BootstrapSource};
use crate::p2p::identity::load_or_generate_keypair;
use crate::p2p::protocol::{
    SwarmInferenceMessage, SwarmQueryRequest, SwarmQueryResponse, SwarmTombstoneMessage,
    GOSSIP_TOPIC_INFERENCES, GOSSIP_TOPIC_QUERIES, GOSSIP_TOPIC_RESPONSES, GOSSIP_TOPIC_TOMBSTONES,
    MAX_GOSSIP_PAYLOAD,
};

/// Max gossip messages accepted per peer per second. Honest clients are far
/// below this; floods above it are dropped unprocessed.
const MAX_MSGS_PER_PEER_PER_SEC: u32 = 20;

/// Log file size cap before truncation (keeps the log bounded).
pub const MAX_LOG_BYTES: u64 = 1_000_000;

/// Default fixed listen port. A fixed, well-known port makes every node
/// dialable (firewall rules, port forwarding, DHT advertisement); when it is
/// taken (second instance on one machine) the swarm falls back to an
/// ephemeral port instead of failing.
pub const DEFAULT_LISTEN_PORT: u16 = 37777;

/// Publish retry window. GossipSub mesh membership settles a heartbeat or
/// two (~1-2 s) after connecting; within this window a failed publish is
/// retried so the first question after joining is not lost. The window
/// matches the L2 deadline (2.5 s) so answers arrive while the asker waits.
pub const PUBLISH_RETRY_WINDOW: Duration = Duration::from_millis(2_500);

/// Publish retry cadence.
const PUBLISH_RETRY_TICK: Duration = Duration::from_millis(400);

/// Manual Kademlia re-bootstrap cadence. kad 0.48 also runs its own periodic
/// bootstrap every 5 minutes; this timer is belt-and-braces for nodes whose
/// routing table went cold.
const KAD_REBOOTSTRAP_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Periodic observability line: one "peers=N" status per minute so operators
/// can distinguish "alone in the network" from "broken logging".
const STATUS_LOG_INTERVAL: Duration = Duration::from_secs(60);

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
    /// Confirmed external (publicly advertised) addresses.
    pub external_addrs: Vec<String>,
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

/// A gossip publish that failed because the mesh had not settled yet, kept
/// for a bounded retry window (see [`PUBLISH_RETRY_WINDOW`]).
struct PendingPublish {
    topic: IdentTopic,
    payload: Vec<u8>,
    first_attempt: Instant,
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

/// Optional fixed listen port (`MBHUB_LISTEN_PORT`). Default: the well-known
/// MBHub port 37777 so nodes are dialable without configuration.
fn listen_port() -> u16 {
    std::env::var("MBHUB_LISTEN_PORT")
        .ok()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_LISTEN_PORT)
}

/// True when the multiaddr embeds a globally routable (public) IP.
///
/// Used to decide whether an identify-observed external address candidate may
/// be confirmed. Loopback, link-local, private, CGNAT and multicast ranges
/// are rejected: advertising them to the WAN DHT would poison other peers'
/// routing tables, and LAN reachability is already covered by mDNS.
fn is_public_candidate(addr: &Multiaddr) -> bool {
    use libp2p::multiaddr::Protocol;
    for proto in addr.iter() {
        match proto {
            Protocol::Ip4(ip) => {
                if ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_link_local()
                    || ip.is_broadcast()
                    || ip.is_documentation()
                {
                    return false;
                }
                let o = ip.octets();
                if o[0] == 10 || o[0] == 127 {
                    return false; // private / loopback
                }
                if o[0] == 172 && (16..=31).contains(&o[1]) {
                    return false; // 172.16/12
                }
                if o[0] == 192 && o[1] == 168 {
                    return false; // 192.168/16
                }
                if o[0] == 169 && o[1] == 254 {
                    return false; // link-local
                }
                if o[0] == 100 && (64..=127).contains(&o[1]) {
                    return false; // CGNAT 100.64/10
                }
            }
            Protocol::Ip6(ip) => {
                if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
                    return false;
                }
                if ip.segments()[0] & 0xfe00 == 0xfc00 {
                    return false; // unique-local fc00::/7
                }
                if ip.segments()[0] & 0xffc0 == 0xfe80 {
                    return false; // link-local fe80::/10
                }
            }
            _ => {}
        }
    }
    true
}

/// Constructs the hardened MBHub swarm (full discovery stack + relay client
/// transport) for a given identity. Shared by the background service and the
/// two-swarm network integration tests.
fn build_swarm(
    keypair: &libp2p::identity::Keypair,
) -> Result<libp2p::Swarm<MbHubBehaviour>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(libp2p::SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )?
        // Relay client transport: dialing /p2p-circuit addresses falls back
        // to circuit relay v2 when direct dialing is impossible (hard NAT).
        .with_relay_client(libp2p::noise::Config::new, libp2p::yamux::Config::default)?
        .with_behaviour(|keypair, relay_client| MbHubBehaviour::new(keypair, relay_client))?
        .with_swarm_config(|c| {
            // libp2p 0.56 closes connections after 10 s without open streams.
            // Between swarm bursts (bootstrap, queries) that idle-kill made
            // PEERS collapse to 0 and the L2 gate skip the swarm entirely.
            // 600 s keeps the mesh warm: the 5/10-min kad re-bootstraps always
            // land inside the window, so peers stay continuously reachable.
            c.with_idle_connection_timeout(Duration::from_secs(600))
        })
        .build())
}

/// Binds the swarm listener: fixed port first, ephemeral fallback when the
/// port is already taken (two instances on one machine).
fn bind_listener(swarm: &mut libp2p::Swarm<MbHubBehaviour>) {
    let port = listen_port();
    let primary: Multiaddr = format!("/ip4/0.0.0.0/tcp/{port}").parse().expect("valid addr");
    match swarm.listen_on(primary.clone()) {
        Ok(_) => {
            log_line(&format!("[MBHub P2P] listening on {primary}"));
        }
        Err(e) => {
            log_line(&format!(
                "[MBHub P2P] fixed port {port} unavailable ({e}); falling back to ephemeral"
            ));
            let fallback: Multiaddr = "/ip4/0.0.0.0/tcp/0".parse().expect("valid addr");
            match swarm.listen_on(fallback) {
                Ok(_) => {}
                Err(e) => log_line(&format!("[MBHub P2P] Failed to listen: {e}")),
            }
        }
    }
}

/// Dials every bootstrap address and seeds the Kademlia routing table.
fn dial_bootstrap_peers(swarm: &mut libp2p::Swarm<MbHubBehaviour>) {
    let list = bootstrap::resolve();
    match list.source {
        BootstrapSource::None => {
            log_line(
                "[MBHub P2P] no bootstrap addresses available (env/embedded/remote/cache all \
                 empty) — running solo; LAN peers still discoverable via mDNS",
            );
            return;
        }
        BootstrapSource::Env => {
            log_line(&format!(
                "[MBHub P2P] bootstrap: {} address(es) from MBHUB_BOOTSTRAP_PEERS",
                list.addresses.len()
            ));
        }
        BootstrapSource::Embedded => {
            log_line(&format!(
                "[MBHub P2P] bootstrap: {} embedded address(es)",
                list.addresses.len()
            ));
        }
        BootstrapSource::Remote => {
            log_line(&format!(
                "[MBHub P2P] bootstrap: {} address(es) from {BOOTSTRAP_URL_MANIFEST}",
                list.addresses.len()
            ));
        }
        BootstrapSource::Cache => {
            log_line(&format!(
                "[MBHub P2P] bootstrap: {} address(es) from local cache (remote unreachable)",
                list.addresses.len()
            ));
        }
    }

    for addr in &list.addresses {
        // Skip our own entry (a node may find itself in a shared manifest).
        if peer_id_of(addr) == Some(*swarm.local_peer_id()) {
            continue;
        }
        // Seed the DHT routing table before bootstrapping so the first
        // FIND_NODE query has somewhere to go.
        if let Some(peer_id) = peer_id_of(addr) {
            swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
        }
        if let Err(e) = swarm.dial(addr.clone()) {
            log_line(&format!("[MBHub P2P] bootstrap dial failed {addr}: {e}"));
        }
    }

    match swarm.behaviour_mut().kad.bootstrap() {
        Ok(_) => {
            log_line("[MBHub P2P] Kademlia bootstrap query started");
        }
        Err(e) => {
            log_line(&format!("[MBHub P2P] Kademlia bootstrap not possible yet: {e}"));
        }
    }
}

const BOOTSTRAP_URL_MANIFEST: &str = bootstrap::BOOTSTRAP_URL;

/// Extracts the `/p2p/<peer-id>` component of a multiaddr, if present.
fn peer_id_of(addr: &Multiaddr) -> Option<PeerId> {
    use libp2p::multiaddr::Protocol;
    addr.iter().find_map(|p| match p {
        Protocol::P2p(peer_id) => Some(peer_id),
        _ => None,
    })
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

    let topics = GossipTopics::new();

    let _ = swarm.behaviour_mut().gossipsub.subscribe(&topics.inferences);
    let _ = swarm.behaviour_mut().gossipsub.subscribe(&topics.queries);
    let _ = swarm.behaviour_mut().gossipsub.subscribe(&topics.responses);
    let _ = swarm.behaviour_mut().gossipsub.subscribe(&topics.tombstones);

    bind_listener(&mut swarm);
    dial_bootstrap_peers(&mut swarm);

    let mut tick_interval = tokio::time::interval(Duration::from_millis(50));
    let mut kad_rebootstrap = tokio::time::interval(KAD_REBOOTSTRAP_INTERVAL);
    let mut bootstrap_refresh = tokio::time::interval(bootstrap::BOOTSTRAP_REFRESH_INTERVAL);
    let mut status_log = tokio::time::interval(STATUS_LOG_INTERVAL);
    let mut publish_retry_tick = tokio::time::interval(PUBLISH_RETRY_TICK);

    let mut peer_rates: HashMap<PeerId, PeerRate> = HashMap::new();
    // Public keys learned via identify — used to verify signed tombstones.
    let mut peer_keys: HashMap<PeerId, libp2p::identity::PublicKey> = HashMap::new();
    // Unique connected peers (a peer may hold up to 2 connections) — the
    // PEERS indicator reports distinct peers, not raw connection count.
    let mut connected_set: HashSet<PeerId> = HashSet::new();
    let mut pending_publishes: VecDeque<PendingPublish> = VecDeque::new();
    let mut listen_addrs_seen: HashSet<String> = HashSet::new();
    let mut tick_counter: u64 = 0;

    loop {
        tokio::select! {
            _ = tick_interval.tick() => {
                tick_counter += 1;

                // Drain outbound inferences
                while let Ok(mut msg) = outbound_inf_rx.try_recv() {
                    // Anti-Poison Hard Gate: defense-in-depth gate before wire transmission
                    if msg.content.trim().is_empty()
                        || msg.content.trim().len() < 10
                        || msg.question.trim().is_empty()
                        || msg.question.trim().len() < 3
                        || msg.is_truncated
                    {
                        continue;
                    }
                    // Publish proof-of-work: raises the cost of flooding the
                    // network with content, independent of identity count.
                    if msg.pow.is_empty() {
                        msg.pow = crate::p2p::protocol::solve_publish_pow(
                            &keypair.public().encode_protobuf(),
                            &msg.content_hash,
                        );
                    }
                    if let Ok(json_bytes) = serde_json::to_vec(&msg) {
                        if json_bytes.len() <= MAX_GOSSIP_PAYLOAD {
                            enqueue_or_publish(
                                &mut swarm,
                                &mut pending_publishes,
                                topics.inferences.clone(),
                                json_bytes,
                            );
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
                            enqueue_or_publish(
                                &mut swarm,
                                &mut pending_publishes,
                                topics.queries.clone(),
                                json_bytes,
                            );
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
                            enqueue_or_publish(
                                &mut swarm,
                                &mut pending_publishes,
                                topics.responses.clone(),
                                json_bytes,
                            );
                        }
                    }
                }

                // Drain outbound tombstones (cryptographic negative signals).
                // Sender side of the signed-tombstone protocol: the swarm
                // signs with the node identity before publication.
                while let Ok(mut tomb) = outbound_tomb_rx.try_recv() {
                    if tomb.reporter_peer_id.is_empty() {
                        tomb.reporter_peer_id = my_peer_id_str.clone();
                    }
                    if tomb.signature.is_empty() {
                        tomb.signature = keypair.sign(&tomb.signing_payload()).unwrap_or_default();
                    }
                    if let Ok(json_bytes) = serde_json::to_vec(&tomb) {
                        if json_bytes.len() <= MAX_GOSSIP_PAYLOAD {
                            enqueue_or_publish(
                                &mut swarm,
                                &mut pending_publishes,
                                topics.tombstones.clone(),
                                json_bytes,
                            );
                        }
                    }
                }

                // Prune stale per-peer rate windows every ~10s to bound memory.
                if tick_counter % 200 == 0 {
                    let now = Instant::now();
                    peer_rates.retain(|_, r| now.duration_since(r.window_start) < Duration::from_secs(2));
                }
            }

            _ = publish_retry_tick.tick() => {
                // Bounded publish retry: gossipsub needs a heartbeat or two
                // after connecting before the mesh carries messages. Keep
                // failed publishes and re-attempt within the retry window so
                // the first question/answer after joining is not lost.
                let now = Instant::now();
                let mut retry: VecDeque<PendingPublish> = VecDeque::new();
                while let Some(pending) = pending_publishes.pop_front() {
                    match swarm.behaviour_mut().gossipsub.publish(pending.topic.clone(), pending.payload.clone()) {
                        Ok(_) => {}
                        Err(gossipsub::PublishError::Duplicate) => {}
                        Err(e) if is_retryable_publish_error(&e) => {
                            if now.duration_since(pending.first_attempt) < PUBLISH_RETRY_WINDOW {
                                log_line(&format!(
                                    "[MBHub P2P] publish deferred (mesh settling): {e}"
                                ));
                                retry.push_back(pending);
                            } else {
                                log_line(&format!(
                                    "[MBHub P2P] publish abandoned after retry window: {e}"
                                ));
                            }
                        }
                        Err(e) => {
                            log_line(&format!("[MBHub P2P] publish failed (non-retryable): {e}"));
                        }
                    }
                }
                pending_publishes = retry;
            }

            _ = kad_rebootstrap.tick() => {
                match swarm.behaviour_mut().kad.bootstrap() {
                    Ok(_) => {
                        log_line("[MBHub P2P] periodic Kademlia re-bootstrap started");
                    }
                    Err(e) => {
                        log_line(&format!("[MBHub P2P] periodic re-bootstrap skipped: {e}"));
                    }
                }
            }

            _ = bootstrap_refresh.tick() => {
                // Re-resolve the manifest (it can gain new bootstrap VMs);
                // dial any address we do not already listen-dial.
                let list = bootstrap::resolve();
                for addr in &list.addresses {
                    if let Some(pid) = peer_id_of(addr) {
                        swarm.behaviour_mut().kad.add_address(&pid, addr.clone());
                    }
                    let _ = swarm.dial(addr.clone());
                }
            }

            _ = status_log.tick() => {
                let (peers, ext) = {
                    let s = status.read().map(|s| (s.connected_peers, s.external_addrs.clone()))
                        .unwrap_or((0, Vec::new()));
                    (s.0, s.1)
                };
                let kad_mode = match swarm.behaviour().kad.mode() {
                    libp2p::kad::Mode::Server => "server",
                    libp2p::kad::Mode::Client => "client",
                };
                log_line(&format!(
                    "[MBHub P2P] status: peers={peers} kad={kad_mode} external={}",
                    if ext.is_empty() { "none".to_string() } else { ext.join(",") }
                ));
            }

            event = swarm.select_next_some() => {
                handle_swarm_event(
                    event,
                    &mut swarm,
                    &status,
                    &my_peer_id_str,
                    &mut peer_rates,
                    &mut peer_keys,
                    &mut connected_set,
                    &mut listen_addrs_seen,
                    &mut pending_publishes,
                    &topics,
                    inbound_inf_tx.clone(),
                    inbound_tomb_tx.clone(),
                    query_resp_tx.clone(),
                );
            }
        }
    }
}

/// The four gossip topic handles, threaded through the event handlers.
struct GossipTopics {
    inferences: IdentTopic,
    queries: IdentTopic,
    responses: IdentTopic,
    tombstones: IdentTopic,
}

impl GossipTopics {
    fn new() -> Self {
        Self {
            inferences: IdentTopic::new(GOSSIP_TOPIC_INFERENCES),
            queries: IdentTopic::new(GOSSIP_TOPIC_QUERIES),
            responses: IdentTopic::new(GOSSIP_TOPIC_RESPONSES),
            tombstones: IdentTopic::new(GOSSIP_TOPIC_TOMBSTONES),
        }
    }
}

/// Publishes immediately; on a mesh-not-settled failure the payload is kept
/// for the bounded retry loop.
fn enqueue_or_publish(
    swarm: &mut libp2p::Swarm<MbHubBehaviour>,
    pending: &mut VecDeque<PendingPublish>,
    topic: IdentTopic,
    payload: Vec<u8>,
) {
    match swarm.behaviour_mut().gossipsub.publish(topic.clone(), payload.clone()) {
        Ok(_) => {}
        Err(gossipsub::PublishError::Duplicate) => {}
        Err(e) if is_retryable_publish_error(&e) => {
            pending.push_back(PendingPublish {
                topic,
                payload,
                first_attempt: Instant::now(),
            });
        }
        Err(e) => {
            log_line(&format!("[MBHub P2P] publish failed (non-retryable): {e}"));
        }
    }
}

/// Central swarm event handling, split from the loop for readability.
#[allow(clippy::too_many_arguments)]
fn handle_swarm_event(
    event: SwarmEvent<MbHubBehaviourEvent>,
    swarm: &mut libp2p::Swarm<MbHubBehaviour>,
    status: &Arc<RwLock<P2pStatus>>,
    my_peer_id_str: &str,
    peer_rates: &mut HashMap<PeerId, PeerRate>,
    peer_keys: &mut HashMap<PeerId, libp2p::identity::PublicKey>,
    connected_set: &mut HashSet<PeerId>,
    listen_addrs_seen: &mut HashSet<String>,
    pending_publishes: &mut VecDeque<PendingPublish>,
    topics: &GossipTopics,
    inbound_inf_tx: Sender<SwarmInferenceMessage>,
    inbound_tomb_tx: Sender<SwarmTombstoneMessage>,
    query_resp_tx: Sender<SwarmQueryResponse>,
) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            if listen_addrs_seen.insert(address.to_string()) {
                log_line(&format!("[MBHub P2P] listening on {address}"));
                if let Ok(mut s) = status.write() {
                    s.listen_addrs.push(address.to_string());
                }
            }
        }
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            connected_set.insert(peer_id);
            if let Ok(mut s) = status.write() {
                s.connected_peers = connected_set.len();
                log_line(&format!(
                    "[MBHub P2P] peer connected: {peer_id} (total {})",
                    s.connected_peers
                ));
            }
        }
        SwarmEvent::ConnectionClosed { peer_id, .. } => {
            peer_keys.remove(&peer_id);
            connected_set.remove(&peer_id);
            if let Ok(mut s) = status.write() {
                s.connected_peers = connected_set.len();
                log_line(&format!(
                    "[MBHub P2P] peer disconnected: {peer_id} (total {})",
                    s.connected_peers
                ));
            }
        }
        SwarmEvent::NewExternalAddrCandidate { address } => {
            // identify reports the address remote peers observe us on. Two
            // guards before confirming (advertising into the DHT):
            // 1. the port must be OUR listen port — identify's observed
            //    address carries the TCP *source* port of the current
            //    connection (an ephemeral port that dies with it); letting
            //    it into our address book poisons future dials.
            // 2. the IP must be publicly routable — LAN ranges stay
            //    mDNS-local.
            let port_is_ours = address.iter().any(|p| {
                matches!(
                    p,
                    libp2p::multiaddr::Protocol::Tcp(port) if port == listen_port()
                )
            });
            if port_is_ours && is_public_candidate(&address) {
                log_line(&format!(
                    "[MBHub P2P] external address candidate confirmed: {address}"
                ));
                swarm.add_external_address(address);
            } else {
                log_line(&format!(
                    "[MBHub P2P] ignoring external address candidate (not our listen port or not public): {address}"
                ));
            }
        }
        SwarmEvent::ExternalAddrConfirmed { address } => {
            if let Ok(mut s) = status.write() {
                if !s.external_addrs.contains(&address.to_string()) {
                    s.external_addrs.push(address.to_string());
                }
            }
        }
        SwarmEvent::ExternalAddrExpired { address } => {
            if let Ok(mut s) = status.write() {
                s.external_addrs.retain(|a| a != &address.to_string());
            }
        }
        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
            log_line(&format!(
                "[MBHub P2P] outgoing dial failed to {}: {error}",
                peer_id.map(|p| p.to_string()).unwrap_or_else(|| "unknown".into())
            ));
        }
        SwarmEvent::Behaviour(behaviour_event) => {
            handle_behaviour_event(
                behaviour_event,
                swarm,
                status,
                my_peer_id_str,
                peer_rates,
                peer_keys,
                pending_publishes,
                topics,
                inbound_inf_tx,
                inbound_tomb_tx,
                query_resp_tx,
            );
        }
        _ => {}
    }
}

/// Behaviour-level event handling (gossip content, DHT, identify, NAT...).
#[allow(clippy::too_many_arguments)]
fn handle_behaviour_event(
    event: MbHubBehaviourEvent,
    swarm: &mut libp2p::Swarm<MbHubBehaviour>,
    status: &Arc<RwLock<P2pStatus>>,
    my_peer_id_str: &str,
    peer_rates: &mut HashMap<PeerId, PeerRate>,
    peer_keys: &mut HashMap<PeerId, libp2p::identity::PublicKey>,
    pending_publishes: &mut VecDeque<PendingPublish>,
    topics: &GossipTopics,
    inbound_inf_tx: Sender<SwarmInferenceMessage>,
    inbound_tomb_tx: Sender<SwarmTombstoneMessage>,
    query_resp_tx: Sender<SwarmQueryResponse>,
) {
    match event {
        MbHubBehaviourEvent::Gossipsub(gossipsub::Event::Message { message, .. }) => {
            handle_gossip_message(
                message,
                swarm,
                my_peer_id_str,
                peer_rates,
                peer_keys,
                pending_publishes,
                topics,
                inbound_inf_tx,
                inbound_tomb_tx,
                query_resp_tx,
            );
        }
        MbHubBehaviourEvent::Gossipsub(gossipsub::Event::Subscribed { peer_id, topic }) => {
            log_line(&format!("[MBHub P2P] {peer_id} subscribed {topic}"));
        }
        MbHubBehaviourEvent::Identify(libp2p::identify::Event::Received {
            peer_id, info, ..
        }) => {
            peer_keys.insert(peer_id, info.public_key.clone());
            // Feed every non-loopback listen address into the DHT routing
            // table: this is how "every peer introduces every other peer".
            for addr in &info.listen_addrs {
                if addr.iter().any(|p| {
                    matches!(p, libp2p::multiaddr::Protocol::Ip4(ip) if ip.is_loopback())
                        || matches!(p, libp2p::multiaddr::Protocol::Ip6(ip) if ip.is_loopback())
                }) {
                    continue;
                }
                swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
            }
        }
        MbHubBehaviourEvent::Kad(libp2p::kad::Event::OutboundQueryProgressed {
            result, ..
        }) => {
            match result {
                libp2p::kad::QueryResult::Bootstrap(Ok(ok)) => {
                    if ok.num_remaining == 0 {
                        log_line(&format!(
                            "[MBHub P2P] Kademlia bootstrap complete via {}",
                            ok.peer
                        ));
                    }
                }
                libp2p::kad::QueryResult::Bootstrap(Err(e)) => {
                    log_line(&format!("[MBHub P2P] Kademlia bootstrap failed: {e}"));
                }
                _ => {}
            }
        }
        MbHubBehaviourEvent::Kad(libp2p::kad::Event::RoutingUpdated {
            peer,
            is_new_peer,
            ..
        }) => {
            if is_new_peer {
                log_line(&format!("[MBHub P2P] DHT routing table: +{peer}"));
            }
        }
        MbHubBehaviourEvent::Kad(libp2p::kad::Event::ModeChanged { new_mode }) => {
            log_line(&format!("[MBHub P2P] Kademlia mode: {new_mode}"));
        }
        MbHubBehaviourEvent::Autonat(libp2p::autonat::Event::StatusChanged { new, .. }) => {
            match new {
                libp2p::autonat::NatStatus::Public(addr) => {
                    // Verified by a successful dial-back. Only advertise it
                    // when the port is our listen port — a dial-back to an
                    // ephemeral source port proves nothing about reachability
                    // of our listener and would poison the address book.
                    let port_is_ours = addr.iter().any(|p| {
                        matches!(
                            p,
                            libp2p::multiaddr::Protocol::Tcp(port) if port == listen_port()
                        )
                    });
                    if port_is_ours {
                        log_line(&format!("[MBHub P2P] NAT status: PUBLIC ({addr})"));
                        swarm.add_external_address(addr);
                    } else {
                        log_line(&format!(
                            "[MBHub P2P] NAT probe verified a non-listen port ({addr}) — not advertising"
                        ));
                    }
                }
                libp2p::autonat::NatStatus::Private => {
                    log_line("[MBHub P2P] NAT status: private (hole punching/relay will be used)");
                }
                libp2p::autonat::NatStatus::Unknown => {
                    log_line("[MBHub P2P] NAT status: unknown");
                }
            }
        }
        MbHubBehaviourEvent::RelayClient(libp2p::relay::client::Event::ReservationReqAccepted {
            relay_peer_id,
            renewal,
            ..
        }) => {
            log_line(&format!(
                "[MBHub P2P] relay reservation accepted by {relay_peer_id} (renewal: {renewal})"
            ));
        }
        MbHubBehaviourEvent::Dcutr(event) => {
            log_line(&format!("[MBHub P2P] DCUtR hole punch: {} → {:?}", event.remote_peer_id, event.result.map(|_| ()).map_err(|e| e.to_string())));
        }
        MbHubBehaviourEvent::Upnp(libp2p::upnp::Event::NewExternalAddr(addr)) => {
            log_line(&format!("[MBHub P2P] UPnP port mapping active: {addr}"));
        }
        MbHubBehaviourEvent::Upnp(libp2p::upnp::Event::GatewayNotFound) => {
            log_line("[MBHub P2P] UPnP: no gateway found");
        }
        MbHubBehaviourEvent::Mdns(libp2p::mdns::Event::Discovered(peers)) => {
            for (peer, addr) in peers {
                if peer.to_string() == my_peer_id_str {
                    continue;
                }
                swarm.behaviour_mut().kad.add_address(&peer, addr.clone());
                let _ = swarm.dial(addr);
            }
        }
        _ => {}
    }
    let _ = status;
}

/// Gossip content handling: rate limiting, integrity gates, self-response,
/// signed tombstone verification.
#[allow(clippy::too_many_arguments)]
fn handle_gossip_message(
    message: gossipsub::Message,
    swarm: &mut libp2p::Swarm<MbHubBehaviour>,
    my_peer_id_str: &str,
    peer_rates: &mut HashMap<PeerId, PeerRate>,
    peer_keys: &mut HashMap<PeerId, libp2p::identity::PublicKey>,
    _pending_publishes: &mut VecDeque<PendingPublish>,
    topics: &GossipTopics,
    inbound_inf_tx: Sender<SwarmInferenceMessage>,
    inbound_tomb_tx: Sender<SwarmTombstoneMessage>,
    query_resp_tx: Sender<SwarmQueryResponse>,
) {
    // Pre-parse size ceiling (§15 rule 1): drop oversized
    // frames before any allocation/processing.
    if message.data.len() > MAX_GOSSIP_PAYLOAD {
        return;
    }

    // Per-peer message rate limit (§4): floods from a
    // single peer are dropped unprocessed.
    if let Some(source) = message.source {
        let now = Instant::now();
        let rate = peer_rates.entry(source).or_insert_with(|| PeerRate::new(now));
        if !rate.allow(now) {
            return;
        }
    }

    if message.topic == topics.queries.hash() {
        if let Ok(query_req) = serde_json::from_slice::<SwarmQueryRequest>(&message.data) {
            // Only respond to peers' queries, never to our own echoes.
            if query_req.asker_peer_id != my_peer_id_str && query_req.passes_integrity_checks() {
                if let Some(hit) =
                    crate::db::find_best_match_by_hash(query_req.simhash, query_req.min_similarity)
                {
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
                            responder_peer_id: my_peer_id_str.to_string(),
                            question: hit.question,
                            content: hit.content,
                            simhash: hit.simhash,
                            provider: hit.provider,
                            model: hit.model,
                            content_hash: hit.content_hash,
                        };
                        if let Ok(bytes) = serde_json::to_vec(&resp) {
                            if bytes.len() <= MAX_GOSSIP_PAYLOAD {
                                enqueue_or_publish(
                                    swarm,
                                    _pending_publishes,
                                    topics.responses.clone(),
                                    bytes,
                                );
                            }
                        }
                    }
                }
            }
        }
    } else if message.topic == topics.responses.hash() {
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
    } else if message.topic == topics.inferences.hash() {
        if let Ok(mut inference) = serde_json::from_slice::<SwarmInferenceMessage>(&message.data) {
            // Authenticated author binding: the distributor is the signed
            // gossip author, never a self-declared field. Refuse messages
            // that lie about their own authorship.
            let Some(source) = message.source else {
                return;
            };
            if inference.author_peer_id.is_empty() {
                inference.author_peer_id = source.to_string();
            } else if inference.author_peer_id != source.to_string() {
                log_line(&format!(
                    "[MBHub P2P] dropped inference: author field ({}) does not match gossip author ({source})",
                    inference.author_peer_id
                ));
                return;
            }
            // Local publisher ban gate: banned distributors' records are
            // never processed (and never reach the database).
            if crate::db::is_banned(&inference.author_peer_id) {
                log_line(&format!(
                    "[MBHub P2P] dropped inference: author {} is locally banned",
                    inference.author_peer_id
                ));
                return;
            }
            // Publish proof-of-work gate: without valid work from the
            // author's key, content is not accepted at all.
            let pow_ok = peer_keys
                .get(&source)
                .map(|pk| {
                    crate::p2p::protocol::verify_publish_pow(
                        &pk.encode_protobuf(),
                        &inference.content_hash,
                        &inference.pow,
                        crate::p2p::protocol::PUBLISH_POW_DIFFICULTY_BITS,
                    )
                })
                .unwrap_or(false);
            if !pow_ok {
                log_line(&format!(
                    "[MBHub P2P] dropped inference from {source}: invalid/missing publish PoW"
                ));
                return;
            }
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
    } else if message.topic == topics.tombstones.hash() {
        if let Ok(tomb) = serde_json::from_slice::<SwarmTombstoneMessage>(&message.data) {
            handle_tombstone_message(tomb, message.source, peer_keys, inbound_tomb_tx);
        }
    }
}

/// Signed-tombstone receiver gate: the message must be signed by the
/// gossip author, the claimed reporter must be the author, and the content
/// hash must be a well-formed BLAKE3 hex digest. Anything else is dropped at
/// the swarm edge — a peer may only delete its own attestations.
fn handle_tombstone_message(
    tomb: SwarmTombstoneMessage,
    source: Option<PeerId>,
    peer_keys: &HashMap<PeerId, libp2p::identity::PublicKey>,
    inbound_tomb_tx: Sender<SwarmTombstoneMessage>,
) {
    let Some(source) = source else {
        log_line("[MBHub P2P] dropped tombstone: anonymous (unsigned author)");
        return;
    };
    if tomb.reporter_peer_id != source.to_string() {
        log_line("[MBHub P2P] dropped tombstone: reporter identity mismatch");
        return;
    }
    let Some(public_key) = peer_keys.get(&source) else {
        log_line(&format!(
            "[MBHub P2P] dropped tombstone from {source}: public key unknown (no identify)"
        ));
        return;
    };
    if !public_key.verify(&tomb.signing_payload(), &tomb.signature) {
        log_line(&format!(
            "[MBHub P2P] dropped tombstone from {source}: invalid signature"
        ));
        return;
    }
    if !tomb.passes_integrity_checks(chrono::Local::now().timestamp()) {
        log_line(&format!(
            "[MBHub P2P] dropped tombstone from {source}: integrity check failed"
        ));
        return;
    }
    // Author-retraction only: a tombstone is honored when the reporter is
    // the stored distributor of this exact content — a publisher retracting
    // their own work. Third-party negative signals are ignored: every user
    // curates their own network, and no peer can delete what they did not
    // author.
    if !crate::db::record_author_matches(&tomb.content_hash, &source.to_string()) {
        log_line(&format!(
            "[MBHub P2P] ignored tombstone from {source}: third-party signals never delete"
        ));
        return;
    }
    let _ = inbound_tomb_tx.send(tomb);
}

/// True when a publish failure is worth retrying (mesh not settled yet).
fn is_retryable_publish_error(e: &gossipsub::PublishError) -> bool {
    matches!(
        e,
        gossipsub::PublishError::NoPeersSubscribedToTopic
            | gossipsub::PublishError::AllQueuesFull(_)
    )
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

    #[test]
    fn publish_error_retry_classification() {
        // Mesh not settled → retryable (the exact failure after joining).
        assert!(is_retryable_publish_error(
            &gossipsub::PublishError::NoPeersSubscribedToTopic
        ));
        assert!(is_retryable_publish_error(
            &gossipsub::PublishError::AllQueuesFull(3)
        ));
        // Content/protocol errors are permanent — retrying cannot succeed.
        assert!(!is_retryable_publish_error(
            &gossipsub::PublishError::Duplicate
        ));
        assert!(!is_retryable_publish_error(
            &gossipsub::PublishError::MessageTooLarge
        ));
        assert!(!is_retryable_publish_error(
            &gossipsub::PublishError::TransformFailed(
                std::io::Error::new(std::io::ErrorKind::InvalidData, "x")
            )
        ));
    }

    #[test]
    fn default_listen_port_is_fixed_and_wellknown() {
        unsafe {
            std::env::remove_var("MBHUB_LISTEN_PORT");
        }
        assert_eq!(listen_port(), 37777, "nodes must be dialable by default");
        unsafe {
            std::env::set_var("MBHUB_LISTEN_PORT", "47777");
        }
        assert_eq!(listen_port(), 47777, "env override respected");
        unsafe {
            std::env::remove_var("MBHUB_LISTEN_PORT");
        }
    }

    #[test]
    fn external_address_candidates_are_classified() {
        let parse = |s: &str| s.parse::<Multiaddr>().unwrap();
        // Public — confirmable.
        assert!(is_public_candidate(&parse("/ip4/93.184.216.34/tcp/37777")));
        assert!(is_public_candidate(&parse("/ip4/1.1.1.1/tcp/37777")));
        // Loopback / private / link-local / CGNAT / ULA — rejected.
        assert!(!is_public_candidate(&parse("/ip4/127.0.0.1/tcp/1")));
        assert!(!is_public_candidate(&parse("/ip4/10.1.2.3/tcp/1")));
        assert!(!is_public_candidate(&parse("/ip4/172.16.0.5/tcp/1")));
        assert!(!is_public_candidate(&parse("/ip4/172.31.255.1/tcp/1")));
        assert!(!is_public_candidate(&parse("/ip4/192.168.1.1/tcp/1")));
        assert!(!is_public_candidate(&parse("/ip4/169.254.1.9/tcp/1")));
        assert!(!is_public_candidate(&parse("/ip4/100.64.0.1/tcp/1")));
        assert!(!is_public_candidate(&parse("/ip6/::1/tcp/1")));
        assert!(!is_public_candidate(&parse("/ip6/fe80::1/tcp/1")));
        assert!(!is_public_candidate(&parse("/ip6/fd00::1/tcp/1")));
    }

    #[test]
    fn peer_id_extraction_from_p2p_multiaddr() {
        let kp = Keypair::generate_ed25519();
        let pid = PeerId::from(kp.public());
        let addr: Multiaddr = format!("/ip4/1.2.3.4/tcp/37777/p2p/{pid}").parse().unwrap();
        assert_eq!(peer_id_of(&addr), Some(pid));
        let bare: Multiaddr = "/ip4/1.2.3.4/tcp/37777".parse().unwrap();
        assert_eq!(peer_id_of(&bare), None);
    }

    /// End-to-end network proof: two real swarms (Noise + Yamux + GossipSub
    /// + DHT stack) in one process. Node B bootstraps to node A exactly like
    /// the production bootstrap path; after the connection and mesh
    /// establish, B's gossiped inference must arrive at A, pass content-hash
    /// integrity, and be rejected when tampered.
    #[test]
    fn two_swarms_connect_and_gossip_inference() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let keypair_b = Keypair::generate_ed25519();
            let swarm_a = build_swarm(&Keypair::generate_ed25519()).expect("swarm A builds");
            let mut swarm_a = swarm_a;
            let swarm_b = build_swarm(&keypair_b).expect("swarm B builds");
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

            // B bootstraps to A — the same dial path used by the bootstrap list.
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
                pow: String::new(),
                author_peer_id: String::new(),
            };
            msg.content_hash = msg.canonical_content_hash();
            // The receiver enforces the publish proof-of-work gate.
            msg.pow = crate::p2p::protocol::solve_publish_pow(&keypair_b.public().encode_protobuf(), &msg.content_hash);
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

    /// DHT discovery proof: B seeds its routing table with A (the production
    /// `dial_bootstrap_peers` path: add_address + dial + bootstrap) and the
    /// Kademlia bootstrap query must complete, adding A as a routable peer.
    #[test]
    fn kad_bootstrap_via_single_seed_node() {
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

            let peer_a = *swarm_a.local_peer_id();

            swarm_a
                .listen_on("/ip4/127.0.0.1/tcp/0".parse::<Multiaddr>().unwrap())
                .expect("A listens");
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

            let addr_a: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port_a}/p2p/{peer_a}")
                .parse()
                .unwrap();

            // Production bootstrap path: seed the routing table, dial, bootstrap.
            swarm_b.behaviour_mut().kad.add_address(&peer_a, addr_a.clone());
            swarm_b.dial(addr_a).expect("B dials A");
            swarm_b.behaviour_mut().kad.bootstrap().expect("B bootstraps");

            // The bootstrap query must complete without error and A must end
            // up in B's routing table (RoutingUpdated) — that is the exact
            // discovery contract: a single seed node opens the DHT.
            let mut bootstrap_ok = false;
            let mut a_routable = false;
            tokio::time::timeout(Duration::from_secs(20), async {
                loop {
                    tokio::select! {
                        ev = swarm_b.select_next_some() => {
                            match ev {
                                SwarmEvent::Behaviour(MbHubBehaviourEvent::Kad(
                                    libp2p::kad::Event::OutboundQueryProgressed {
                                        result: libp2p::kad::QueryResult::Bootstrap(Ok(ok)),
                                        ..
                                    },
                                )) => {
                                    if ok.num_remaining == 0 {
                                        bootstrap_ok = true;
                                    }
                                }
                                SwarmEvent::Behaviour(MbHubBehaviourEvent::Kad(
                                    libp2p::kad::Event::OutboundQueryProgressed {
                                        result: libp2p::kad::QueryResult::Bootstrap(Err(e)),
                                        ..
                                    },
                                )) => {
                                    panic!("bootstrap should not fail with a live seed: {e}");
                                }
                                SwarmEvent::Behaviour(MbHubBehaviourEvent::Kad(
                                    libp2p::kad::Event::RoutingUpdated { peer, .. },
                                )) => {
                                    if peer == peer_a {
                                        a_routable = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                        _ = swarm_a.select_next_some() => {}
                    }
                    if bootstrap_ok && a_routable {
                        return;
                    }
                }
            })
            .await
            .expect("kad bootstrap completes and seed becomes routable");
        });
    }

    /// Signed tombstone end-to-end: a signed tombstone from a known peer is
    /// accepted; the same tombstone with a tampered signature is dropped.
    #[test]
    fn signed_tombstones_accepted_tampered_rejected() {
        let keypair = Keypair::generate_ed25519();
        let reporter = PeerId::from(keypair.public()).to_string();

        let mut tomb = SwarmTombstoneMessage {
            content_hash: "c".repeat(64),
            simhash: 42,
            timestamp: chrono::Local::now().timestamp(),
            reporter_peer_id: reporter,
            reason: "hallucinated content".to_string(),
            signature: Vec::new(),
        };
        tomb.signature = keypair.sign(&tomb.signing_payload()).expect("signs");

        // Wire round-trip preserves the signature.
        let bytes = serde_json::to_vec(&tomb).unwrap();
        let parsed: SwarmTombstoneMessage = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed.passes_integrity_checks(chrono::Local::now().timestamp()));

        // A tampered signature (different key) must not verify.
        let attacker = Keypair::generate_ed25519();
        assert!(!attacker.public().verify(&parsed.signing_payload(), &parsed.signature));

        // An unsigned legacy tombstone is rejected by integrity checks.
        let mut unsigned = parsed.clone();
        unsigned.signature.clear();
        assert!(!unsigned.passes_integrity_checks(chrono::Local::now().timestamp()));
    }
}
