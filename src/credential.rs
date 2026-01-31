//! API credential types.
//!
//! Defines the structure for credentials used in 17track API requests.

/// API credentials extracted/generated for 17track requests.
#[derive(Debug, Clone)]
pub struct ApiCredentials {
    pub sign: String,
    pub last_event_id: String,
    pub yq_bid: String,
    /// The configs.md5 value from the page (needed for Last-Event-ID generation).
    pub configs_md5: String,
}
