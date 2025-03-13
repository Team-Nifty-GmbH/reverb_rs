// src/lib.rs

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::{
    mpsc::{self, Receiver},
    Mutex,
};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};
use tracing::{debug, error, info, trace, warn};
use url::Url;

// Error types for the library
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

// Event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReverbEvent {
    ConnectionEstablished {
        event: String,
        data: ConnectionData,
    },
    SubscriptionSucceeded {
        event: String,
        data: String,
        channel: String,
    },
    ChannelEvent {
        event: String,
        data: String,
        channel: String,
    },
    Error {
        event: String,
        data: ErrorData,
    },
    // Other internal event types
    Unknown(serde_json::Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionData {
    socket_id: String,
    activity_timeout: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorData {
    code: u32,
    message: String,
}

// Message types for subscription and events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeMessage {
    event: String,
    data: SubscribeData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeData {
    channel: String,
    auth: Option<String>,
    channel_data: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientEventMessage {
    event: String,
    channel: String,
    data: serde_json::Value,
}

// Channel trait for defining common channel behavior
#[async_trait]
pub trait Channel: Send + Sync {
    fn name(&self) -> &str;
    fn requires_auth(&self) -> bool;
    async fn get_auth(
        &self,
        socket_id: &str,
        client: &ReverbClient,
    ) -> Result<Option<String>, ReverbError>;
}

// Private channel implementation
#[derive(Debug, Clone)]
pub struct PrivateChannel {
    name: String,
}

impl PrivateChannel {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

#[async_trait]
impl Channel for PrivateChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn requires_auth(&self) -> bool {
        true
    }

    async fn get_auth(
        &self,
        socket_id: &str,
        client: &ReverbClient,
    ) -> Result<Option<String>, ReverbError> {
        client.authenticate_channel(socket_id, self.name()).await
    }
}

// Presence channel implementation
#[derive(Debug, Clone)]
pub struct PresenceChannel {
    name: String,
    user_data: String,
}

impl PresenceChannel {
    pub fn new(name: String, user_data: String) -> Self {
        Self { name, user_data }
    }
}

#[async_trait]
impl Channel for PresenceChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn requires_auth(&self) -> bool {
        true
    }

    async fn get_auth(
        &self,
        socket_id: &str,
        client: &ReverbClient,
    ) -> Result<Option<String>, ReverbError> {
        client
            .authenticate_presence_channel(socket_id, self.name(), &self.user_data)
            .await
    }
}

// Public channel implementation
#[derive(Debug, Clone)]
pub struct PublicChannel {
    name: String,
}

impl PublicChannel {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

#[async_trait]
impl Channel for PublicChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn requires_auth(&self) -> bool {
        false
    }

    async fn get_auth(
        &self,
        _socket_id: &str,
        _client: &ReverbClient,
    ) -> Result<Option<String>, ReverbError> {
        Ok(None)
    }
}

// Event handler trait that users can implement
#[async_trait]
pub trait EventHandler: Send + Sync {
    async fn on_connection_established(&self, socket_id: &str);
    async fn on_channel_subscription_succeeded(&self, channel: &str);
    async fn on_channel_event(&self, channel: &str, event: &str, data: &str);
    async fn on_error(&self, code: u32, message: &str);
}

// The main ReverbClient struct
pub struct ReverbClient {
    app_key: String,
    host: String,
    port: u16,
    secure: bool,
    auth_endpoint: Option<String>,
    app_secret: Option<String>,
    socket_id: Arc<Mutex<Option<String>>>,
    connection: Arc<Mutex<Option<Arc<WebSocketConnection>>>>,
    event_handlers: Arc<Mutex<Vec<Box<dyn EventHandler>>>>,
    http_client: HttpClient,
    csrf_token: Arc<Mutex<Option<String>>>,
}

// WebSocket connection handler
struct WebSocketConnection {
    socket: mpsc::Sender<Message>,
    _task_handle: tokio::task::JoinHandle<()>,
}

impl WebSocketConnection {
    async fn send(&self, message: Message) -> Result<(), ReverbError> {
        self.socket
            .send(message)
            .await
            .map_err(|e| ReverbError::SendError(e.to_string()))
    }
}

impl ReverbClient {
    pub fn new(
        app_key: &str,
        app_secret: &str,
        auth_endpoint: &str,
        host: &str,
        secure: bool,
    ) -> Self {
        Self {
            app_key: app_key.to_string(),
            host: host.to_string(),
            port: if secure { 443 } else { 80 },
            secure,
            auth_endpoint: Option::from(auth_endpoint.to_string()),
            app_secret: Option::from(app_secret.to_string()),
            socket_id: Arc::new(Mutex::new(None)),
            connection: Arc::new(Mutex::new(None)),
            event_handlers: Arc::new(Mutex::new(Vec::new())),
            http_client: HttpClient::new(),
            csrf_token: Arc::new(Mutex::new(None)),
        }
    }

    // Set authentication method options
    pub fn with_auth_endpoint(mut self, endpoint: &str) -> Self {
        self.auth_endpoint = Some(endpoint.to_string());
        self
    }

    pub fn with_app_secret(mut self, secret: &str) -> Self {
        self.app_secret = Some(secret.to_string());
        self
    }

    // Register an event handler
    pub async fn add_event_handler<H: EventHandler + 'static>(&self, handler: H) {
        let mut handlers = self.event_handlers.lock().await;
        handlers.push(Box::new(handler));
    }

    // Connect to the WebSocket server
    pub async fn connect(&self) -> Result<(), ReverbError> {
        let scheme = if self.secure { "wss" } else { "ws" };
        let url = format!(
            "{}://{}:{}/app/{}",
            scheme, self.host, self.port, self.app_key
        );
        let url = Url::parse(&url)?;

        info!("Connecting to Laravel Reverb at {}", url);

        let connection_result = connect_async(url).await;
        let ws_stream = match connection_result {
            Ok((stream, response)) => {
                debug!("Connected to WebSocket server. Response: {:?}", response);
                stream
            },
            Err(err) => {
                error!("Failed to connect to WebSocket server: {}", err);
                return Err(err.into());
            }
        };

        debug!("WebSocket connection established: {:?}", ws_stream);

        let (sink, stream) = ws_stream.split();

        let (tx, rx) = mpsc::channel::<Message>(100);

        let connection = Arc::new(WebSocketConnection {
            socket: tx,
            _task_handle: self.spawn_ws_tasks(sink, stream, rx),
        });

        let mut conn_guard = self.connection.lock().await;
        *conn_guard = Some(connection);

        Ok(())
    }

    // Spawn tasks to handle WebSocket communication
    fn spawn_ws_tasks(
        &self,
        sink: futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
        mut stream: futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        mut rx: Receiver<Message>,
    ) -> tokio::task::JoinHandle<()> {
        // Clone these for our tasks
        let event_handlers = Arc::clone(&self.event_handlers);
        let socket_id = Arc::clone(&self.socket_id);

        // Create thread-safe wrappers for the sink
        let sink = Arc::new(Mutex::new(sink));

        tokio::spawn(async move {
            // Task for sending messages to the WebSocket
            let sink_clone = Arc::clone(&sink);
            let send_task = tokio::spawn(async move {
                while let Some(message) = rx.recv().await {
                    let mut sink = sink_clone.lock().await;
                    if let Err(e) = sink.send(message).await {
                        error!("Error sending message: {}", e);
                        break;
                    }
                }
            });

            // Task for receiving messages from the WebSocket
            let sink_clone = Arc::clone(&sink);
            let receive_task = tokio::spawn(async move {
                while let Some(message_result) = stream.next().await {
                    match message_result {
                        Ok(message) => {
                            if let Message::Text(text) = message {
                                trace!("Received message: {}", text);

                                if let Ok(event) = serde_json::from_str::<ReverbEvent>(&text) {
                                    match event {
                                        ReverbEvent::ConnectionEstablished { data, .. } => {
                                            // Store socket ID
                                            {
                                                let mut sid = socket_id.lock().await;
                                                *sid = Some(data.socket_id.clone());
                                            }

                                            // Notify handlers
                                            let socket_id_str = data.socket_id.clone();

                                            // FIXED: Process handlers immediately while holding the lock
                                            let handlers = event_handlers.lock().await;
                                            for handler in handlers.iter() {
                                                handler
                                                    .on_connection_established(&socket_id_str)
                                                    .await;
                                            }
                                        }
                                        ReverbEvent::SubscriptionSucceeded { channel, .. } => {
                                            let channel_str = channel.clone();

                                            // FIXED: Process handlers immediately while holding the lock
                                            let handlers = event_handlers.lock().await;
                                            for handler in handlers.iter() {
                                                handler
                                                    .on_channel_subscription_succeeded(&channel_str)
                                                    .await;
                                            }
                                        }
                                        ReverbEvent::ChannelEvent {
                                            event,
                                            data,
                                            channel,
                                        } => {
                                            if !event.starts_with("pusher_") {
                                                let event_str = event.clone();
                                                let data_str = data.clone();
                                                let channel_str = channel.clone();

                                                // FIXED: Process handlers immediately while holding the lock
                                                let handlers = event_handlers.lock().await;
                                                for handler in handlers.iter() {
                                                    handler
                                                        .on_channel_event(
                                                            &channel_str,
                                                            &event_str,
                                                            &data_str,
                                                        )
                                                        .await;
                                                }
                                            }
                                        }
                                        ReverbEvent::Error { data, .. } => {
                                            error!(
                                                "Reverb error: {} (code: {})",
                                                data.message, data.code
                                            );

                                            let code = data.code;
                                            let message = data.message.clone();

                                            // FIXED: Process handlers immediately while holding the lock
                                            let handlers = event_handlers.lock().await;
                                            for handler in handlers.iter() {
                                                handler.on_error(code, &message).await;
                                            }
                                        }
                                        _ => {
                                            debug!("Unhandled event type: {}", text);
                                        }
                                    }
                                } else {
                                    error!("Failed to parse message: {}", text);
                                }
                            } else if let Message::Ping(data) = message {
                                debug!("Received ping");
                                // Respond with pong
                                let data_clone = data.clone();
                                let mut sink = sink_clone.lock().await;
                                if let Err(e) = sink.send(Message::Pong(data_clone)).await {
                                    error!("Failed to send pong: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            error!("WebSocket error: {}", e);
                            break;
                        }
                    }
                }
            });

            // Wait for either task to complete
            tokio::select! {
                _ = send_task => {
                    warn!("Send task completed");
                },
                _ = receive_task => {
                    warn!("Receive task completed");
                }
            }
        })
    }

    // Subscribe to a channel
    pub async fn subscribe<C: Channel>(&self, channel: C) -> Result<(), ReverbError> {
        let socket_id = {
            let guard = self.socket_id.lock().await;
            match &*guard {
                Some(id) => id.clone(),
                None => return Err(ReverbError::ConnectionError("Not connected".to_string())),
            }
        };

        let auth = if channel.requires_auth() {
            channel.get_auth(&socket_id, self).await?
        } else {
            None
        };

        let subscribe_message = SubscribeMessage {
            event: "pusher:subscribe".to_string(),
            data: SubscribeData {
                channel: channel.name().to_string(),
                auth,
                channel_data: None, // This would be set for presence channels
            },
        };

        let json = serde_json::to_string(&subscribe_message)?;

        let conn_guard = self.connection.lock().await;
        match &*conn_guard {
            Some(connection) => {
                connection.send(Message::Text(json)).await?;
                Ok(())
            }
            None => Err(ReverbError::ConnectionError("Not connected".to_string())),
        }
    }

    // Send a client event to a channel
    pub async fn trigger_event(
        &self,
        channel: &str,
        event: &str,
        data: serde_json::Value,
    ) -> Result<(), ReverbError> {
        // Ensure event name is prefixed with 'client-'
        let event_name = if !event.starts_with("client-") {
            format!("client-{}", event)
        } else {
            event.to_string()
        };

        let message = ClientEventMessage {
            event: event_name,
            channel: channel.to_string(),
            data,
        };

        let json = serde_json::to_string(&message)?;

        let conn_guard = self.connection.lock().await;
        match &*conn_guard {
            Some(connection) => {
                connection.send(Message::Text(json)).await?;
                Ok(())
            }
            None => Err(ReverbError::ConnectionError("Not connected".to_string())),
        }
    }

    // Authenticate a channel using app secret (server-side auth)
    async fn authenticate_channel(
        &self,
        socket_id: &str,
        channel: &str,
    ) -> Result<Option<String>, ReverbError> {
        if let Some(secret) = &self.app_secret {
            let signature = format!("{}:{}", socket_id, channel);
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .map_err(|_| ReverbError::AuthError("HMAC creation failed".to_string()))?;
            mac.update(signature.as_bytes());
            let result = mac.finalize().into_bytes();
            let auth = format!("{}:{}", self.app_key, hex::encode(result));

            Ok(Some(auth))
        } else if let Some(endpoint) = &self.auth_endpoint {
            self.fetch_auth_from_endpoint(endpoint, socket_id, channel, None)
                .await
        } else {
            Err(ReverbError::AuthError(
                "No authentication method available".to_string(),
            ))
        }
    }

    // Authenticate a presence channel
    async fn authenticate_presence_channel(
        &self,
        socket_id: &str,
        channel: &str,
        user_data: &str,
    ) -> Result<Option<String>, ReverbError> {
        if let Some(secret) = &self.app_secret {
            let signature = format!("{}:{}:{}", socket_id, channel, user_data);
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .map_err(|_| ReverbError::AuthError("HMAC creation failed".to_string()))?;
            mac.update(signature.as_bytes());
            let result = mac.finalize().into_bytes();
            let auth = format!("{}:{}", self.app_key, hex::encode(result));

            Ok(Some(auth))
        } else if let Some(endpoint) = &self.auth_endpoint {
            self.fetch_auth_from_endpoint(endpoint, socket_id, channel, Some(user_data))
                .await
        } else {
            Err(ReverbError::AuthError(
                "No authentication method available".to_string(),
            ))
        }
    }

    // Get CSRF token for Laravel authentication
    async fn get_csrf_token(&self) -> Result<String, ReverbError> {
        // First check if we already have a token
        {
            let token = self.csrf_token.lock().await;
            if let Some(token) = token.as_ref() {
                return Ok(token.clone());
            }
        }

        // Fetch the token
        let endpoint = if self.secure {
            format!("https://{}:{}/sanctum/csrf-cookie", self.host, self.port)
        } else {
            format!("http://{}:{}/sanctum/csrf-cookie", self.host, self.port)
        };

        let response = self.http_client.get(&endpoint).send().await?;

        // Extract the XSRF-TOKEN from cookies
        if let Some(cookie_header) = response.headers().get("set-cookie") {
            if let Ok(cookie_str) = cookie_header.to_str() {
                if let Some(start) = cookie_str.find("XSRF-TOKEN=") {
                    let start = start + "XSRF-TOKEN=".len();
                    if let Some(end) = cookie_str[start..].find(';') {
                        let token = urlencoding::decode(&cookie_str[start..start + end])
                            .map_err(|_| {
                                ReverbError::AuthError("Failed to decode CSRF token".to_string())
                            })?
                            .to_string();

                        // Save the token
                        let mut token_guard = self.csrf_token.lock().await;
                        *token_guard = Some(token.clone());

                        return Ok(token);
                    }
                }
            }
        }

        Err(ReverbError::AuthError(
            "Failed to get CSRF token".to_string(),
        ))
    }

    // Fetch authentication token from the auth endpoint
    async fn fetch_auth_from_endpoint(
        &self,
        endpoint: &str,
        socket_id: &str,
        channel: &str,
        channel_data: Option<&str>,
    ) -> Result<Option<String>, ReverbError> {
        // Prepare the request payload
        let mut form = HashMap::new();
        form.insert("socket_id", socket_id);
        form.insert("channel_name", channel);

        if let Some(data) = channel_data {
            form.insert("channel_data", data);
        }

        // Get CSRF token if needed
        let csrf_token = self.get_csrf_token().await?;

        // Send the request
        let response = self
            .http_client
            .post(endpoint)
            .header("X-CSRF-TOKEN", csrf_token)
            .form(&form)
            .send()
            .await?;

        let status = response.status();

        if status.is_success() {
            let auth_data: serde_json::Value = response.json().await?;

            if let Some(auth) = auth_data["auth"].as_str() {
                return Ok(Some(auth.to_string()));
            }
        }

        Err(ReverbError::AuthError(format!(
            "Authentication failed: {}",
            status
        )))
    }

    // Close the WebSocket connection
    pub async fn disconnect(&self) -> Result<(), ReverbError> {
        let mut conn_guard = self.connection.lock().await;

        if let Some(connection) = conn_guard.take() {
            // Send a close frame
            if let Err(e) = connection.send(Message::Close(None)).await {
                warn!("Error sending close frame: {}", e);
            }
        }

        Ok(())
    }
}

// Helper functions to create channel instances
pub fn private_channel(name: &str) -> PrivateChannel {
    let name = if !name.starts_with("private-") {
        format!("private-{}", name)
    } else {
        name.to_string()
    };

    PrivateChannel::new(name)
}

pub fn presence_channel(name: &str, user_data: &str) -> PresenceChannel {
    let name = if !name.starts_with("presence-") {
        format!("presence-{}", name)
    } else {
        name.to_string()
    };

    PresenceChannel::new(name, user_data.to_string())
}

pub fn public_channel(name: &str) -> PublicChannel {
    PublicChannel::new(name.to_string())
}
