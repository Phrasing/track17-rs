use serde::{Deserialize, Serialize};
use std::fmt;

/// Package tracking state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingState {
    LabelCreated,
    InTransit,
    OutForDelivery,
    Delivered,
    DeliveredSigned,
    Exception,
    ExceptionDelayed,
    ExceptionHeld,
    ExceptionReturned,
    ExceptionDamaged,
    AvailableForPickup,
    Expired,
    Unknown,
}

impl TrackingState {
    /// Parse from 17track's stage or sub_status field
    pub fn from_stage(stage: &str) -> Self {
        match stage {
            // Exact matches first
            "InfoReceived" => Self::LabelCreated,
            "InTransit" => Self::InTransit,
            "OutForDelivery" => Self::OutForDelivery,
            "Delivered" => Self::Delivered,
            "Delivered_Signed" => Self::DeliveredSigned,
            "Delivered_Other" => Self::Delivered,
            "Exception" => Self::Exception,
            "Exception_Delayed" => Self::ExceptionDelayed,
            "Exception_Held" => Self::ExceptionHeld,
            "Exception_Returned" | "Exception_RTS" => Self::ExceptionReturned,
            "Exception_Damaged" => Self::ExceptionDamaged,
            "AvailableForPickup" => Self::AvailableForPickup,
            "Expired" => Self::Expired,
            "Undelivered" => Self::Exception,
            // Prefix matches for other variants
            s if s.starts_with("InTransit_") => Self::InTransit,
            s if s.starts_with("Delivered_") => Self::Delivered,
            s if s.starts_with("Exception_") => Self::Exception,
            _ => Self::Unknown,
        }
    }
}

impl fmt::Display for TrackingState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LabelCreated => write!(f, "LABEL_CREATED"),
            Self::InTransit => write!(f, "IN_TRANSIT"),
            Self::OutForDelivery => write!(f, "OUT_FOR_DELIVERY"),
            Self::Delivered => write!(f, "DELIVERED"),
            Self::DeliveredSigned => write!(f, "DELIVERED_SIGNED"),
            Self::Exception => write!(f, "EXCEPTION"),
            Self::ExceptionDelayed => write!(f, "EXCEPTION_DELAYED"),
            Self::ExceptionHeld => write!(f, "EXCEPTION_HELD"),
            Self::ExceptionReturned => write!(f, "EXCEPTION_RETURNED"),
            Self::ExceptionDamaged => write!(f, "EXCEPTION_DAMAGED"),
            Self::AvailableForPickup => write!(f, "AVAILABLE_FOR_PICKUP"),
            Self::Expired => write!(f, "EXPIRED"),
            Self::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

/// Tracking request to be base64-encoded and sent to the API
#[derive(Debug, Serialize)]
pub struct TrackingRequest {
    pub data: Vec<TrackingItem>,
    pub guid: String,
    #[serde(rename = "timeZoneOffset")]
    pub time_zone_offset: i32,
    pub sign: String,
}

/// Individual tracking item in the request
#[derive(Debug, Clone, Serialize)]
pub struct TrackingItem {
    pub num: String,
    pub fc: u32,
    pub sc: u32,
}

/// Response from the tracking API
#[derive(Debug, Deserialize)]
pub struct TrackingResponse {
    pub id: u32,
    #[serde(default)]
    pub guid: String,
    pub shipments: Vec<Shipment>,
    pub meta: Meta,
}

/// Extra field for code 400 responses with carrier suggestions
#[derive(Debug, Clone, Deserialize)]
pub struct ShipmentExtra {
    /// Available carrier codes when auto-detect fails
    #[serde(default)]
    pub multi: Vec<u32>,
}

/// Individual shipment in the response
#[derive(Debug, Clone, Deserialize)]
pub struct Shipment {
    pub code: i32,
    pub number: String,
    pub carrier: u32,
    pub carrier_final: Option<u32>,
    pub param: Option<serde_json::Value>,
    pub params: Option<serde_json::Value>,
    pub params_v2: Option<Vec<ParamV2>>,
    pub extra: Option<Vec<ShipmentExtra>>,
    pub shipment: Option<ShipmentDetails>,
    #[serde(default)]
    pub pre_status: Option<i32>,
    #[serde(default)]
    pub prior_status: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    pub state_final: Option<String>,
    pub service_type: Option<String>,
    pub service_type_final: Option<String>,
    #[serde(default)]
    pub key: Option<i32>,
    #[serde(default)]
    pub show_more: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ParamV2 {
    pub key: String,
    pub input_type: String,
    pub example: String,
    pub regex: String,
    pub options: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShipmentDetails {
    pub tracking: Option<TrackingDetails>,
    pub latest_event: Option<TrackingEvent>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrackingDetails {
    pub providers: Option<Vec<Provider>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    pub events: Vec<TrackingEvent>,
}

/// Location can be either a string or a structured object
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum LocationData {
    String(String),
    Structured(LocationDetails),
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocationDetails {
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub postal_code: Option<String>,
    pub zip_code: Option<String>,
    pub address: Option<String>,
    // Common variations
    #[serde(alias = "countryCode")]
    pub country_code: Option<String>,
    #[serde(alias = "postalCode")]
    pub postal_code_alt: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrackingEvent {
    pub time: Option<String>,
    pub time_iso: Option<String>,
    pub time_utc: Option<String>,
    pub description: Option<String>,
    pub location: Option<LocationData>,
    pub stage: Option<String>,
    pub sub_status: Option<String>,
}

impl TrackingEvent {
    /// Get the tracking state from this event's stage or sub_status
    pub fn tracking_state(&self) -> TrackingState {
        // Try stage first, then fall back to sub_status
        self.stage
            .as_deref()
            .or(self.sub_status.as_deref())
            .map(TrackingState::from_stage)
            .unwrap_or(TrackingState::Unknown)
    }

    /// Get the raw location string
    pub fn raw_location(&self) -> Option<String> {
        match &self.location {
            Some(LocationData::String(s)) if !s.is_empty() => Some(s.clone()),
            Some(LocationData::Structured(loc)) => {
                let city = loc.city.as_deref().filter(|s| !s.is_empty());
                let state = loc.state.as_deref().filter(|s| !s.is_empty());
                let country = loc
                    .country
                    .as_deref()
                    .or(loc.country_code.as_deref())
                    .filter(|s| !s.is_empty());
                let postal = loc
                    .postal_code
                    .as_deref()
                    .or(loc.postal_code_alt.as_deref())
                    .or(loc.zip_code.as_deref())
                    .filter(|s| !s.is_empty());

                match (city, state, postal) {
                    (Some(c), Some(s), _) => Some(format!("{}, {}", c, s)),
                    (Some(c), None, Some(p)) => Some(format!("{} {}", c, p)),
                    (Some(c), None, None) => Some(c.to_string()),
                    (None, Some(s), Some(p)) => Some(format!("{} {}", s, p)),
                    (None, Some(s), None) => Some(s.to_string()),
                    (None, None, Some(p)) => match country {
                        Some(co) => Some(format!("{} {}", co, p)),
                        None => Some(p.to_string()),
                    },
                    _ => loc.address.clone(),
                }
            }
            _ => None,
        }
    }

    /// Parse country and zip from raw location like "US 60455"
    pub fn parse_location_parts(&self) -> Option<(String, String)> {
        let raw = self.raw_location()?;
        let parts: Vec<&str> = raw.split_whitespace().collect();
        if parts.len() == 2 {
            Some((parts[0].to_string(), parts[1].to_string()))
        } else {
            None
        }
    }
}

/// Metadata in the response
#[derive(Debug, Clone, Deserialize)]
pub struct Meta {
    pub code: i32,
    pub message: String,
}

/// Carrier codes
pub mod carriers {
    pub const AUTO: u32 = 0; // Auto-detect carrier
    pub const FEDEX: u32 = 100003;
    pub const UPS: u32 = 100001;
    pub const USPS: u32 = 100002;
    pub const DHL: u32 = 100005;
}
