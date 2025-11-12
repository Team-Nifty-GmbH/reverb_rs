use async_trait::async_trait;
use crate::error::ReverbError;
use crate::client::ReverbClient;

/// Channel trait defining common channel behavior
#[async_trait]
pub trait Channel: Send + Sync {
    /// Get the channel name
    fn name(&self) -> &str;

    /// Check if the channel requires authentication
    fn requires_auth(&self) -> bool;

    /// Get authentication token for the channel
    async fn get_auth(
        &self,
        socket_id: &str,
        client: &ReverbClient,
    ) -> Result<Option<String>, ReverbError>;

    /// Get channel data (for presence channels)
    fn get_channel_data(&self) -> Option<String>;
}

/// Private channel implementation
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

    fn get_channel_data(&self) -> Option<String> {
        None
    }
}

/// Presence channel implementation
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

    fn get_channel_data(&self) -> Option<String> {
        Some(self.user_data.clone())
    }
}

/// Public channel implementation
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

    fn get_channel_data(&self) -> Option<String> {
        None
    }
}

/// Helper function to create a private channel
pub fn private_channel(name: &str) -> PrivateChannel {
    let name = if !name.starts_with("private-") {
        format!("private-{}", name)
    } else {
        name.to_string()
    };

    PrivateChannel::new(name)
}

/// Helper function to create a presence channel
pub fn presence_channel(name: &str, user_data: &str) -> PresenceChannel {
    let name = if !name.starts_with("presence-") {
        format!("presence-{}", name)
    } else {
        name.to_string()
    };

    PresenceChannel::new(name, user_data.to_string())
}

/// Helper function to create a public channel
pub fn public_channel(name: &str) -> PublicChannel {
    PublicChannel::new(name.to_string())
}
