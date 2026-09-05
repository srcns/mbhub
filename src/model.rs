//! Data structures: Inferences, Dates, Settings, Routing, and Providers.

/// Structured timestamp for display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ts {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

/// Supported date presentation formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateFormat {
    DotDmy,   // DD.MM.YYYY HH:MM
    IsoDash,  // YYYY-MM-DD HH:MM
    SlashMdy, // MM/DD/YYYY HH:MM
    SlashYmd, // YYYY/MM/DD HH:MM
}

impl DateFormat {
    pub const ALL: [DateFormat; 4] = [
        DateFormat::DotDmy,
        DateFormat::IsoDash,
        DateFormat::SlashMdy,
        DateFormat::SlashYmd,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DateFormat::DotDmy => "DD.MM.YYYY HH:MM",
            DateFormat::IsoDash => "YYYY-MM-DD HH:MM",
            DateFormat::SlashMdy => "MM/DD/YYYY HH:MM",
            DateFormat::SlashYmd => "YYYY/MM/DD HH:MM",
        }
    }

    pub fn format(self, t: &Ts) -> String {
        match self {
            DateFormat::DotDmy => {
                format!("{:02}.{:02}.{} {:02}:{:02}", t.day, t.month, t.year, t.hour, t.minute)
            }
            DateFormat::IsoDash => {
                format!("{}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute)
            }
            DateFormat::SlashMdy => {
                format!("{:02}/{:02}/{} {:02}:{:02}", t.month, t.day, t.year, t.hour, t.minute)
            }
            DateFormat::SlashYmd => {
                format!("{}/{:02}/{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute)
            }
        }
    }
}

/// A stored inference response.
#[derive(Clone, Debug, PartialEq)]
pub struct InferenceRecord {
    pub ts: Ts,
    pub similarity: f32, // 1.0 .. 99.99
    pub question: String,
    pub content: String,
    pub simhash: u64,
    pub provider: String,
    pub model: String,
    /// BLAKE3 content hash over (question, content, provider, model).
    /// Empty for legacy records predating content-addressing.
    pub content_hash: String,
    /// True when this record arrived from the P2P swarm (unverified source)
    /// rather than being produced locally by a live provider call.
    pub is_swarm: bool,
    /// Similarity of this record's question to the user's own past questions
    /// (Query Locality score, 0.0 .. 100.0). Drives Memory ordering and
    /// locality-aware eviction. 0.0 when no query profile exists yet.
    pub locality: f32,
    /// True if the inference was truncated, timed out, or interrupted mid-stream.
    /// Truncated records are preserved locally but NEVER broadcast to the P2P swarm.
    pub is_truncated: bool,
    /// True if marked as a candidate for publication to the web collective archive.
    pub publish_candidate: bool,
}

impl InferenceRecord {
    /// Formatted similarity with 2 decimals (e.g. "99.69").
    #[allow(dead_code)]
    pub fn similarity_string(&self) -> String {
        format!("{:.2}", self.similarity)
    }

    /// Formatted Query Locality score with 2 decimals (e.g. "87.42").
    pub fn locality_string(&self) -> String {
        format!("{:.2}", self.locality)
    }

    /// Single-line question preview for the Memory list.
    pub fn preview(&self) -> &str {
        self.question.trim()
    }
}

/// A known AI provider whose endpoint is shipped in the client.
#[derive(Clone, Copy, Debug)]
pub struct Provider {
    pub name: &'static str,
    pub endpoint: &'static str,
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        name: "OpenAI",
        endpoint: "https://api.openai.com/v1",
    },
    Provider {
        name: "Anthropic",
        endpoint: "https://api.anthropic.com/v1",
    },
    Provider {
        name: "Google Gemini",
        endpoint: "https://generativelanguage.googleapis.com/v1beta/openai",
    },
    Provider {
        name: "DeepSeek",
        endpoint: "https://api.deepseek.com/v1",
    },
    Provider {
        name: "xAI (Grok)",
        endpoint: "https://api.x.ai/v1",
    },
    Provider {
        name: "OpenRouter",
        endpoint: "https://openrouter.ai/api/v1",
    },
    Provider {
        name: "Groq",
        endpoint: "https://api.groq.com/openai/v1",
    },
    Provider {
        name: "Perplexity",
        endpoint: "https://api.perplexity.ai",
    },
    Provider {
        name: "Mistral AI",
        endpoint: "https://api.mistral.ai/v1",
    },
    Provider {
        name: "Cohere",
        endpoint: "https://api.cohere.com/v2",
    },
    Provider {
        name: "Together AI",
        endpoint: "https://api.together.xyz/v1",
    },
];

/// Selection for answer freshness filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    AnyTime,
    Hours24,
    Days7,
    Days30,
    Days90,
    Year1,
}

impl Freshness {
    pub const ALL: [Freshness; 6] = [
        Freshness::AnyTime,
        Freshness::Hours24,
        Freshness::Days7,
        Freshness::Days30,
        Freshness::Days90,
        Freshness::Year1,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Freshness::AnyTime => "Any time",
            Freshness::Hours24 => "24 hours",
            Freshness::Days7 => "7 days",
            Freshness::Days30 => "30 days",
            Freshness::Days90 => "90 days",
            Freshness::Year1 => "1 year",
        }
    }

    /// Time window duration in seconds. Returns `None` for `AnyTime`.
    #[allow(dead_code)]
    pub fn duration_seconds(self) -> Option<i64> {
        match self {
            Freshness::AnyTime => None,
            Freshness::Hours24 => Some(24 * 3600),
            Freshness::Days7 => Some(7 * 86400),
            Freshness::Days30 => Some(30 * 86400),
            Freshness::Days90 => Some(90 * 86400),
            Freshness::Year1 => Some(365 * 86400),
        }
    }

    /// Calculates the minimum unix epoch timestamp for this freshness setting.
    #[allow(dead_code)]
    pub fn min_timestamp(self, current_epoch: i64) -> Option<i64> {
        self.duration_seconds().map(|dur| current_epoch.saturating_sub(dur))
    }
}

/// Selection for storage personalization vs blind swarm sharding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShardingMode {
    QueryLocality, // Personalized based on past query history
    BlindSwarm,    // Random/blind swarm shards, zero query tracking
}

impl ShardingMode {
    pub const ALL: [ShardingMode; 2] = [
        ShardingMode::QueryLocality,
        ShardingMode::BlindSwarm,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ShardingMode::QueryLocality => "Query locality",
            ShardingMode::BlindSwarm => "Blind swarm",
        }
    }
}

/// Minimum similarity threshold required to consider a cache/swarm result a "hit".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitRate {
    Percent70,
    Percent75,
    Percent80,
    Percent85,
    Percent90,
    Percent95,
    Percent99,
}

impl HitRate {
    pub const ALL: [HitRate; 7] = [
        HitRate::Percent70,
        HitRate::Percent75,
        HitRate::Percent80,
        HitRate::Percent85,
        HitRate::Percent90,
        HitRate::Percent95,
        HitRate::Percent99,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HitRate::Percent70 => "70%",
            HitRate::Percent75 => "75%",
            HitRate::Percent80 => "80%",
            HitRate::Percent85 => "85%",
            HitRate::Percent90 => "90%",
            HitRate::Percent95 => "95%",
            HitRate::Percent99 => "99%",
        }
    }

    pub fn percentage(self) -> f32 {
        match self {
            HitRate::Percent70 => 70.0,
            HitRate::Percent75 => 75.0,
            HitRate::Percent80 => 80.0,
            HitRate::Percent85 => 85.0,
            HitRate::Percent90 => 90.0,
            HitRate::Percent95 => 95.0,
            HitRate::Percent99 => 99.0,
        }
    }

    #[allow(dead_code)]
    pub fn value(self) -> f32 {
        match self {
            HitRate::Percent70 => 70.0,
            HitRate::Percent75 => 75.0,
            HitRate::Percent80 => 80.0,
            HitRate::Percent85 => 85.0,
            HitRate::Percent90 => 90.0,
            HitRate::Percent95 => 95.0,
            HitRate::Percent99 => 99.0,
        }
    }
}

/// Provenance / Source tracking for generated inferences.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum InferenceSource {
    CloudProvider {
        provider: String,
        model: String,
    },
    SwarmPeer {
        peer_id: String,
    },
}

impl InferenceSource {
    #[allow(dead_code)]
    pub fn can_gossip_to_swarm(&self) -> bool {
        match self {
            InferenceSource::CloudProvider { .. } => true,
            InferenceSource::SwarmPeer { .. } => false,
        }
    }
}

/// User settings. Local-only; nothing here leaves the machine.
#[derive(Clone, Debug)]
pub struct Settings {
    pub date_format: DateFormat,
    pub reserved_gb: u64,
    pub sharding_mode: ShardingMode,
    pub hit_rate: HitRate,
    pub freshness: Freshness,
    pub provider_idx: usize,
    pub provider_model: String,
    pub api_key: String,
    pub provider_keys: std::collections::HashMap<String, String>,
    pub provider_selected_models: std::collections::HashMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            date_format: DateFormat::DotDmy,
            // Default shard budget: 1 GB. Even the lowest-budget peer holds
            // the whole early-stage network locally; users raise it as needed.
            reserved_gb: 1,
            sharding_mode: ShardingMode::QueryLocality,
            hit_rate: HitRate::Percent85,
            freshness: Freshness::AnyTime,
            provider_idx: 0,
            provider_model: "gpt-4o".to_string(),
            api_key: String::new(),
            provider_keys: std::collections::HashMap::new(),
            provider_selected_models: std::collections::HashMap::new(),
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Self::default();
        // 1. Load keys and models from SQLite meta
        s.provider_keys = crate::db::load_provider_keys();
        s.provider_selected_models = crate::db::load_provider_models();

        // 2. Load keys and models from .env file / environment variables (highest priority)
        for provider in PROVIDERS {
            let key = crate::env::get_api_key_for_provider(provider.name);
            if !key.is_empty() {
                s.provider_keys.insert(provider.name.to_string(), key);
            }
            if let Some(model) = crate::env::get_model_for_provider(provider.name) {
                if !model.is_empty() {
                    s.provider_selected_models.insert(provider.name.to_string(), model);
                }
            }
        }

        // Active provider resolution: env -> SQLite -> default
        let env_file = crate::env::load_env_file();
        if let Some(active_name) = env_file.get("ACTIVE_PROVIDER") {
            if let Some(pos) = PROVIDERS
                .iter()
                .position(|p| p.name.eq_ignore_ascii_case(active_name))
            {
                s.provider_idx = pos;
            }
        } else if let Some(idx_str) = crate::db::get_meta("active_provider_idx") {
            if let Ok(idx) = idx_str.parse::<usize>() {
                if idx < PROVIDERS.len() {
                    s.provider_idx = idx;
                }
            }
        }

        let mut provider_name = PROVIDERS[s.provider_idx].name;
        if let Some(key) = s.provider_keys.get(provider_name) {
            s.api_key = key.clone();
        }

        // If the selected provider has no key, auto-fallback to any configured provider
        if s.api_key.trim().is_empty() {
            for (idx, p) in PROVIDERS.iter().enumerate() {
                if let Some(key) = s.provider_keys.get(p.name) {
                    if !key.trim().is_empty() {
                        s.provider_idx = idx;
                        provider_name = p.name;
                        s.api_key = key.clone();
                        break;
                    }
                }
            }
        }

        if let Some(model) = s.provider_selected_models.get(provider_name) {
            s.provider_model = model.clone();
        } else {
            s.provider_model = crate::api::client::default_model_for_provider(provider_name);
        }

        s
    }
}
