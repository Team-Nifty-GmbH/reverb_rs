use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use reqwest::Client as HttpClient;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::protocol::Message,
};
use tracing::{debug, error, info, trace, warn};
use url::Url;

use crate::channel::Channel;
use crate::connection::WebSocketConnection;
use crate::error::ReverbError;
use crate::event::EventHandler;
use crate::message::{
    ClientEventMessage, ConnectionData, ErrorData, PusherMessage, SubscribeData, SubscribeMessage,
};

/// Main Reverb client for connecting to Laravel Reverb WebSocket server
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

impl ReverbClient {
    /// Create a new Reverb client
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
            auth_endpoint: Some(auth_endpoint.to_string()),
            app_secret: Some(app_secret.to_string()),
            socket_id: Arc::new(Mutex::new(None)),
            connection: Arc::new(Mutex::new(None)),
            event_handlers: Arc::new(Mutex::new(Vec::new())),
            http_client: HttpClient::new(),
            csrf_token: Arc::new(Mutex::new(None)),
        }
    }

    /// Set authentication endpoint
    pub fn with_auth_endpoint(mut self, endpoint: &str) -> Self {
        self.auth_endpoint = Some(endpoint.to_string());
        self
    }

    /// Set application secret for server-side authentication
    pub fn with_app_secret(mut self, secret: &str) -> Self {
        self.app_secret = Some(secret.to_string());
        self
    }

    /// Add an event handler for receiving WebSocket events
    pub async fn add_event_handler<H: EventHandler + 'static>(&self, handler: H) {
        let mut handlers = self.event_handlers.lock().await;
        handlers.push(Box::new(handler));
    }

    /// Connect to the Reverb WebSocket server
    pub async fn connect(&self) -> Result<(), ReverbError> {
        let scheme = if self.secure { "wss" } else { "ws" };
        let url = format!(
            "{}://{}:{}/app/{}",
            scheme, self.host, self.port, self.app_key
        );
        let url = Url::parse(&url)?;

        info!("Connecting to Laravel Reverb at {}", url);

        let (ws_stream, response) = connect_async(url).await.map_err(|e| {
            error!("Failed to connect to WebSocket server: {}", e);
            e
        })?;

        debug!("Connected to WebSocket server. Response: {:?}", response);

        let (sink, stream) = ws_stream.split();
        let (tx, rx) = mpsc::channel::<Message>(100);

        let connection = Arc::new(WebSocketConnection {
            socket: tx,
            _task_handle: self.spawn_ws_tasks(sink, stream, rx),
        });

        // Start ping interval for keepalive
        self.start_ping_interval();

        let mut conn_guard = self.connection.lock().await;
        *conn_guard = Some(connection);

        Ok(())
    }

    /// Spawn WebSocket send and receive tasks
    fn spawn_ws_tasks(
        &self,
        sink: futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
        mut stream: futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        mut rx: mpsc::Receiver<Message>,
    ) -> tokio::task::JoinHandle<()> {
        let event_handlers = Arc::clone(&self.event_handlers);
        let socket_id = Arc::clone(&self.socket_id);
        let sink = Arc::new(Mutex::new(sink));

        tokio::spawn(async move {
            // Task for sending messages
            let sink_clone = Arc::clone(&sink);
            let send_task = tokio::spawn(async move {
                while let Some(message) = rx.recv().await {
                    if let Err(e) = sink_clone.lock().await.send(message).await {
                        error!("Error sending message: {}", e);
                        break;
                    }
                }
            });

            // Task for receiving messages
            let sink_clone = Arc::clone(&sink);
            let receive_task = tokio::spawn(async move {
                while let Some(Ok(message)) = stream.next().await {
                    if let Message::Text(text) = message {
                        trace!("Received message: {}", text);

                        if let Ok(pusher_msg) = serde_json::from_str::<PusherMessage>(&text) {
                            match pusher_msg.event.as_str() {
                                "pusher:connection_established" => {
                                    if let serde_json::Value::String(data_str) = &pusher_msg.data
                                        && let Ok(conn_data) =
                                            serde_json::from_str::<ConnectionData>(data_str)
                                        {
                                            debug!(
                                                "Connection established with socket ID: {}",
                                                conn_data.socket_id
                                            );

                                            *socket_id.lock().await =
                                                Some(conn_data.socket_id.clone());

                                            for handler in event_handlers.lock().await.iter() {
                                                handler
                                                    .on_connection_established(&conn_data.socket_id)
                                                    .await;
                                            }
                                        }
                                }
                                "pusher_internal:subscription_succeeded" => {
                                    if let Some(channel) = &pusher_msg.channel {
                                        debug!("Subscription succeeded for channel: {}", channel);

                                        for handler in event_handlers.lock().await.iter() {
                                            handler
                                                .on_channel_subscription_succeeded(channel)
                                                .await;
                                        }
                                    }
                                }
                                "pusher:error" => {
                                    if let Ok(error_data) =
                                        serde_json::from_value::<ErrorData>(pusher_msg.data)
                                    {
                                        error!(
                                            "Reverb error: {} (code: {})",
                                            error_data.message, error_data.code
                                        );

                                        for handler in event_handlers.lock().await.iter() {
                                            handler
                                                .on_error(error_data.code, &error_data.message)
                                                .await;
                                        }
                                    }
                                }
                                _ => {
                                    if !pusher_msg.event.starts_with("pusher:")
                                        && !pusher_msg.event.starts_with("pusher_internal:")
                                        && let Some(channel) = &pusher_msg.channel {
                                            let data_str = match &pusher_msg.data {
                                                serde_json::Value::String(s) => s.clone(),
                                                _ => serde_json::to_string(&pusher_msg.data)
                                                    .unwrap_or_default(),
                                            };

                                            debug!(
                                                "Channel event: {} on {}",
                                                pusher_msg.event, channel
                                            );

                                            for handler in event_handlers.lock().await.iter() {
                                                handler
                                                    .on_channel_event(
                                                        channel,
                                                        &pusher_msg.event,
                                                        &data_str,
                                                    )
                                                    .await;
                                            }
                                        }
                                }
                            }
                        }
                    } else if let Message::Ping(data) = message
                        && let Err(e) = sink_clone.lock().await.send(Message::Pong(data)).await {
                            error!("Failed to send pong: {}", e);
                        }
                }
            });

            tokio::select! {
                _ = send_task => warn!("Send task completed"),
                _ = receive_task => warn!("Receive task completed")
            }
        })
    }

    /// Subscribe to a channel
    pub async fn subscribe<C: Channel>(&self, channel: C) -> Result<(), ReverbError> {
        let socket_id = self
            .socket_id
            .lock()
            .await
            .clone()
            .ok_or_else(|| ReverbError::ConnectionError("Not connected".to_string()))?;

        let auth = if channel.requires_auth() {
            channel.get_auth(&socket_id, self).await?
        } else {
            None
        };

        let channel_data = channel.get_channel_data();

        let subscribe_message = SubscribeMessage {
            event: "pusher:subscribe".to_string(),
            data: SubscribeData {
                channel: channel.name().to_string(),
                auth,
                channel_data,
            },
        };

        let json = serde_json::to_string(&subscribe_message)?;

        match &*self.connection.lock().await {
            Some(connection) => {
                connection.send(Message::Text(json)).await?;
                Ok(())
            }
            None => Err(ReverbError::ConnectionError("Not connected".to_string())),
        }
    }

    /// Unsubscribe from a channel
    pub async fn unsubscribe(&self, channel_name: &str) -> Result<(), ReverbError> {
        let unsubscribe_message = serde_json::json!({
            "event": "pusher:unsubscribe",
            "data": {
                "channel": channel_name
            }
        });

        let json = serde_json::to_string(&unsubscribe_message)?;

        match &*self.connection.lock().await {
            Some(connection) => {
                connection.send(Message::Text(json)).await?;
                Ok(())
            }
            None => Err(ReverbError::ConnectionError("Not connected".to_string())),
        }
    }

    /// Trigger a client event on a channel
    pub async fn trigger_event(
        &self,
        channel: &str,
        event: &str,
        data: serde_json::Value,
    ) -> Result<(), ReverbError> {
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

        match &*self.connection.lock().await {
            Some(connection) => {
                connection.send(Message::Text(json)).await?;
                Ok(())
            }
            None => Err(ReverbError::ConnectionError("Not connected".to_string())),
        }
    }

    /// Authenticate a private channel (internal method)
    pub(crate) async fn authenticate_channel(
        &self,
        socket_id: &str,
        channel: &str,
    ) -> Result<Option<String>, ReverbError> {
        if let Some(secret) = &self.app_secret {
            // Server-side auth using HMAC
            // Format: socket_id:channel_name
            let signature = format!("{}:{}", socket_id, channel);
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .map_err(|_| ReverbError::AuthError("HMAC creation failed".to_string()))?;

            mac.update(signature.as_bytes());
            let result = mac.finalize().into_bytes();
            // Auth format: app_key:hex_encoded_hmac
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

    /// Authenticate a presence channel (internal method)
    pub(crate) async fn authenticate_presence_channel(
        &self,
        socket_id: &str,
        channel: &str,
        user_data: &str,
    ) -> Result<Option<String>, ReverbError> {
        if let Some(secret) = &self.app_secret {
            // Server-side auth for presence channel
            // Format: socket_id:channel_name:channel_data
            let signature = format!("{}:{}:{}", socket_id, channel, user_data);
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .map_err(|_| ReverbError::AuthError("HMAC creation failed".to_string()))?;

            mac.update(signature.as_bytes());
            let result = mac.finalize().into_bytes();
            // Auth format: app_key:hex_encoded_hmac
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

    /// Get CSRF token from server
    async fn get_csrf_token(&self) -> Result<String, ReverbError> {
        if let Some(token) = self.csrf_token.lock().await.clone() {
            return Ok(token);
        }

        let endpoint = if self.secure {
            format!("https://{}:{}/broadcasting/auth", self.host, self.port)
        } else {
            format!("http://{}:{}/broadcasting/auth", self.host, self.port)
        };

        let response = self.http_client.get(&endpoint).send().await?;

        if let Some(cookie_header) = response.headers().get("set-cookie")
            && let Ok(cookie_str) = cookie_header.to_str()
                && let Some(start) = cookie_str.find("XSRF-TOKEN=") {
                    let start = start + "XSRF-TOKEN=".len();
                    if let Some(end) = cookie_str[start..].find(';') {
                        let token = urlencoding::decode(&cookie_str[start..start + end])
                            .map_err(|_| {
                                ReverbError::AuthError("Failed to decode CSRF token".to_string())
                            })?
                            .to_string();

                        *self.csrf_token.lock().await = Some(token.clone());
                        return Ok(token);
                    }
                }

        Err(ReverbError::AuthError(
            "Failed to get CSRF token".to_string(),
        ))
    }

    /// Fetch authentication from endpoint
    async fn fetch_auth_from_endpoint(
        &self,
        endpoint: &str,
        socket_id: &str,
        channel: &str,
        channel_data: Option<&str>,
    ) -> Result<Option<String>, ReverbError> {
        let mut form = HashMap::new();
        form.insert("socket_id", socket_id);
        form.insert("channel_name", channel);

        if let Some(data) = channel_data {
            form.insert("channel_data", data);
        }

        let csrf_token = self.get_csrf_token().await?;

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

    /// Disconnect from the server
    pub async fn disconnect(&self) -> Result<(), ReverbError> {
        if let Some(connection) = self.connection.lock().await.take()
            && let Err(e) = connection.send(Message::Close(None)).await {
                warn!("Error sending close frame: {}", e);
            }

        Ok(())
    }

    /// Wait for the WebSocket connection to disconnect
    /// Returns when the connection is closed (either by server or network failure)
    pub async fn wait_for_disconnect(&self) {
        // Get the connection and wait for its task to complete
        let connection = {
            let guard = self.connection.lock().await;
            guard.as_ref().map(Arc::clone)
        };

        if let Some(conn) = connection {
            // Wait for the task handle to complete
            // We need to take ownership, so we'll poll until disconnect is detected
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

                // Check if connection is still alive by trying to get it
                let guard = self.connection.lock().await;
                if guard.is_none() {
                    break;
                }

                // Check if the sender is closed (indicates connection died)
                if conn.socket.is_closed() {
                    info!("WebSocket connection closed");
                    break;
                }
            }
        }
    }

    /// Start ping interval for keepalive
    pub fn start_ping_interval(&self) {
        let connection = Arc::clone(&self.connection);

        tokio::spawn(async move {
            let ping_interval = tokio::time::Duration::from_secs(30);

            loop {
                tokio::time::sleep(ping_interval).await;

                let conn_guard = connection.lock().await;
                if let Some(conn) = &*conn_guard {
                    let ping_message = serde_json::json!({
                        "event": "pusher:ping",
                        "data": {}
                    });

                    if let Err(e) = conn.send(Message::Text(ping_message.to_string())).await {
                        error!("Failed to send ping message: {}", e);
                        break;
                    }

                    debug!("Ping message sent");
                } else {
                    debug!("No active connection to send ping to");
                    break;
                }
            }
        });
    }
}
