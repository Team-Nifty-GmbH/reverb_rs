use async_trait::async_trait;

/// Event handler trait for receiving WebSocket events
#[async_trait]
pub trait EventHandler: Send + Sync {
    /// Called when connection is established with the server
    async fn on_connection_established(&self, socket_id: &str);

    /// Called when channel subscription succeeds
    async fn on_channel_subscription_succeeded(&self, channel: &str);

    /// Called when a channel event is received
    async fn on_channel_event(&self, channel: &str, event: &str, data: &str);

    /// Called when an error occurs
    async fn on_error(&self, code: u32, message: &str);
}
