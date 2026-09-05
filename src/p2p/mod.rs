pub mod identity;
pub mod protocol;
pub mod service;

#[allow(unused_imports)]
pub use service::{start_p2p_service, P2pHandle, P2pStatus};
#[allow(unused_imports)]
pub use protocol::{
    SwarmInferenceMessage, SwarmQueryRequest, SwarmQueryResponse, SwarmTombstoneMessage,
    GOSSIP_TOPIC_INFERENCES, GOSSIP_TOPIC_QUERIES, GOSSIP_TOPIC_RESPONSES, GOSSIP_TOPIC_TOMBSTONES,
    MAX_GOSSIP_PAYLOAD, MAX_HOP_TTL,
};
