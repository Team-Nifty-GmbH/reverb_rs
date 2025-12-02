//! reverb-rs - A Rust client library for Laravel Reverb WebSocket server
//!
//! This library provides a client for connecting to Laravel Reverb, which implements
//! the Pusher protocol for WebSocket communication.
//!
//! # Features
//! - Public, private, and presence channel support
//! - Server-side and client-side authentication
//! - Event handling with async traits
//! - Automatic reconnection and keepalive
//!
//! # Example
//! ```no_run
//! use reverb_rs::{ReverbClient, EventHandler, public_channel};
//! use async_trait::async_trait;
//!
//! struct MyHandler;
//!
//! #[async_trait]
//! impl EventHandler for MyHandler {
//!     async fn on_connection_established(&self, socket_id: &str) {
//!         println!("Connected with socket ID: {}", socket_id);
//!     }
//!
//!     async fn on_channel_subscription_succeeded(&self, channel: &str) {
//!         println!("Subscribed to channel: {}", channel);
//!     }
//!
//!     async fn on_channel_event(&self, channel: &str, event: &str, data: &str) {
//!         println!("Event {} on channel {}: {}", event, channel, data);
//!     }
//!
//!     async fn on_error(&self, code: u32, message: &str) {
//!         eprintln!("Error {}: {}", code, message);
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     let client = ReverbClient::new(
//!         "app-key",
//!         "app-secret",
//!         "http://localhost/broadcasting/auth",
//!         "localhost",
//!         false,
//!     );
//!
//!     client.add_event_handler(MyHandler).await;
//!     client.connect().await.unwrap();
//!     client.subscribe(public_channel("my-channel")).await.unwrap();
//! }
//! ```

mod channel;
mod client;
mod connection;
mod error;
mod event;
mod message;

// Public re-exports
pub use channel::{
    Channel, PresenceChannel, PrivateChannel, PublicChannel, presence_channel, private_channel,
    public_channel,
};
pub use client::ReverbClient;
pub use error::ReverbError;
pub use event::EventHandler;
pub use message::{ConnectionData, ErrorData};
