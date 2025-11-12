use thiserror::Error;

/// Error types for the reverb-rs library
#[derive(Error, Debug)]
pub enum ReverbError {
    #[error("WebSocket error: {0}")]
    WebSocketError(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("URL parse error: {0}")]
    UrlParseError(#[from] url::ParseError),
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("HTTP request error: {0}")]
    HttpError(#[from] reqwest::Error),
    #[error("Channel authentication failed: {0}")]
    AuthError(String),
    #[error("Connection error: {0}")]
    ConnectionError(String),
    #[error("Subscription error: {0}")]
    SubscriptionError(String),
    #[error("Send error: {0}")]
    SendError(String),
}
