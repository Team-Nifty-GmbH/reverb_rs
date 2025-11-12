use serde::{Deserialize, Serialize};

/// Pusher protocol message structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PusherMessage {
    pub event: String,
    #[serde(default)]
    pub data: serde_json::Value,
    #[serde(default)]
    pub channel: Option<String>,
}

/// Connection data received after establishing connection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionData {
    pub socket_id: String,
    pub activity_timeout: u32,
}

/// Error data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorData {
    pub code: u32,
    pub message: String,
}

/// Subscribe message structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SubscribeMessage {
    pub event: String,
    pub data: SubscribeData,
}

/// Subscribe data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SubscribeData {
    pub channel: String,
    pub auth: Option<String>,
    pub channel_data: Option<String>,
}

/// Client event message structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ClientEventMessage {
    pub event: String,
    pub channel: String,
    pub data: serde_json::Value,
}
