//! Combined libp2p network behaviour for MBHub.
//!
//! Composes the full peer-discovery stack specified for Phase 1:
//!
//! - `gossipsub` — content topics (inferences, queries, responses, tombstones).
//! - `kad` — Kademlia DHT peer routing (`/mbhub/kad/1.0.0`): every peer acts
//!   as an introducer; only the first contact requires a bootstrap address.
//! - `identify` — protocol/agent exchange; feeds observed external addresses
//!   and peers' listen addresses into the routing table.
//! - `autonat` — NAT detection via dial-back probes (verified reachability).
//! - `dcutr` — direct connection upgrade through relayed connections.
//! - `relay` (client) — circuit relay v2 client; fallback transport for
//!   hard-NAT'd nodes (reservation confirmed by the bootstrap/relay node).
//! - `upnp` — automatic gateway port mapping where supported.
//! - `mdns` — free peer discovery on the local network.
//! - `limits` — connection budget against Sybil flooding.

use libp2p::gossipsub::{self, MessageAuthenticity};
use libp2p::identity::Keypair;
use libp2p::swarm::NetworkBehaviour;
use libp2p::StreamProtocol;

use crate::p2p::protocol::MAX_GOSSIP_PAYLOAD;

/// Connection budget: at most this many established connections total, and
/// at most 2 per peer.
pub const MAX_ESTABLISHED_CONNECTIONS: u32 = 32;
pub const MAX_CONNECTIONS_PER_PEER: u32 = 2;

/// MBHub Kademlia protocol name. Bumping the version isolates incompatible
/// networks (same pattern as the gossipsub topic versioning).
pub const KAD_PROTOCOL_NAME: &str = "/mbhub/kad/1.0.0";

/// identify protocol version string.
pub const IDENTIFY_PROTOCOL_VERSION: &str = "mbhub/1.0.0";

/// Combined network behaviour: full discovery stack plus connection limits.
///
/// The relay **client** behaviour is injected by the swarm builder
/// (`SwarmBuilder::with_relay_client`) because libp2p couples it with the
/// relayed transport — see `service::build_swarm`.
#[derive(NetworkBehaviour)]
#[behaviour(prelude = "libp2p::swarm::derive_prelude")]
pub struct MbHubBehaviour {
    pub gossipsub: gossipsub::Behaviour<gossipsub::IdentityTransform>,
    pub limits: libp2p::connection_limits::Behaviour,
    pub identify: libp2p::identify::Behaviour,
    pub kad: libp2p::kad::Behaviour<libp2p::kad::store::MemoryStore>,
    pub autonat: libp2p::autonat::Behaviour,
    pub dcutr: libp2p::dcutr::Behaviour,
    pub relay_client: libp2p::relay::client::Behaviour,
    pub upnp: libp2p::upnp::tokio::Behaviour,
    pub mdns: libp2p::mdns::tokio::Behaviour,
}

impl MbHubBehaviour {
    /// Builds the composed behaviour. `relay_client` arrives pre-built from
    /// the swarm builder and is passed through.
    pub fn new(keypair: &Keypair, relay_client: libp2p::relay::client::Behaviour) -> Self {
        let peer_id = keypair.public().to_peer_id();

        let connection_limits = libp2p::connection_limits::ConnectionLimits::default()
            .with_max_established(Some(MAX_ESTABLISHED_CONNECTIONS))
            .with_max_established_per_peer(Some(MAX_CONNECTIONS_PER_PEER))
            .with_max_pending_incoming(Some(MAX_ESTABLISHED_CONNECTIONS))
            .with_max_pending_outgoing(Some(MAX_ESTABLISHED_CONNECTIONS));

        let message_authenticity = MessageAuthenticity::Signed(keypair.clone());
        let gossip_config = gossipsub::ConfigBuilder::default()
            .max_transmit_size(MAX_GOSSIP_PAYLOAD)
            .build()
            .expect("valid gossipsub config");
        let gossipsub = gossipsub::Behaviour::<gossipsub::IdentityTransform>::new(
            message_authenticity,
            gossip_config,
        )
        .expect("valid gossipsub behaviour");

        let identify = libp2p::identify::Behaviour::new(
            libp2p::identify::Config::new(
                IDENTIFY_PROTOCOL_VERSION.to_string(),
                keypair.public(),
            )
            .with_agent_version(format!("mbhub/{}", env!("CARGO_PKG_VERSION"))),
        );

        let mut kad_config = libp2p::kad::Config::new(StreamProtocol::new(KAD_PROTOCOL_NAME));
        // Discovery lookups must fail fast so the L2 gate never stalls the
        // pipeline; record storage is unused (peer routing only).
        kad_config.set_query_timeout(std::time::Duration::from_secs(10));
        // Keep the automatic mode: start as DHT client, flip to server mode
        // as soon as a verified external address is confirmed — every client
        // becomes an introducer as soon as it is reachable.
        let kad = libp2p::kad::Behaviour::with_config(
            peer_id,
            libp2p::kad::store::MemoryStore::new(peer_id),
            kad_config,
        );

        let autonat = libp2p::autonat::Behaviour::new(peer_id, libp2p::autonat::Config::default());
        let dcutr = libp2p::dcutr::Behaviour::new(peer_id);
        let upnp = libp2p::upnp::tokio::Behaviour::default();
        let mdns = libp2p::mdns::tokio::Behaviour::new(
            libp2p::mdns::Config::default(),
            peer_id,
        )
        .expect("valid mdns behaviour");

        Self {
            gossipsub,
            limits: libp2p::connection_limits::Behaviour::new(connection_limits),
            identify,
            kad,
            autonat,
            dcutr,
            relay_client,
            upnp,
            mdns,
        }
    }
}
