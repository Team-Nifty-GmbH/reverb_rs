# reverb-rs

A Rust client library for [Laravel Reverb](https://reverb.laravel.com/) WebSocket server. This library implements the Pusher protocol for real-time WebSocket communication.

## Features

- **Channel Types**: Support for public, private, and presence channels
- **Authentication**: Server-side HMAC authentication and client-side endpoint authentication
- **Event Handling**: Async trait-based event handlers for flexible event processing
- **Automatic Keepalive**: Built-in ping interval (30s) to maintain connection
- **TLS Support**: Secure WebSocket connections via `wss://`

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
reverb-rs = { git = "https://github.com/Team-Nifty-GmbH/reverb_rs" }
```

## Quick Start

```rust
use reverb_rs::{ReverbClient, EventHandler, private_channel};
use async_trait::async_trait;

struct MyHandler;

#[async_trait]
impl EventHandler for MyHandler {
    async fn on_connection_established(&self, socket_id: &str) {
        println!("Connected with socket ID: {}", socket_id);
    }

    async fn on_channel_subscription_succeeded(&self, channel: &str) {
        println!("Subscribed to channel: {}", channel);
    }

    async fn on_channel_event(&self, channel: &str, event: &str, data: &str) {
        println!("Event {} on channel {}: {}", event, channel, data);
    }

    async fn on_error(&self, code: u32, message: &str) {
        eprintln!("Error {}: {}", code, message);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ReverbClient::new(
        "your-app-key",
        "your-app-secret",
        "https://your-app.com/broadcasting/auth",
        "your-reverb-host.com",
        true, // use TLS (wss://)
    );

    client.add_event_handler(MyHandler).await;
    client.connect().await?;

    // Subscribe to a private channel
    client.subscribe(private_channel("my-channel")).await?;

    // Keep the connection alive
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}
```

## Channel Types

### Public Channels

No authentication required:

```rust
use reverb_rs::public_channel;

client.subscribe(public_channel("news")).await?;
```

### Private Channels

Requires authentication (automatically handled):

```rust
use reverb_rs::private_channel;

// The "private-" prefix is added automatically
client.subscribe(private_channel("user.123")).await?;
```

### Presence Channels

For channels that track member presence:

```rust
use reverb_rs::presence_channel;

let user_data = serde_json::json!({
    "user_id": 123,
    "user_info": { "name": "John" }
}).to_string();

client.subscribe(presence_channel("chat-room", &user_data)).await?;
```

## Authentication

### Server-Side (HMAC)

When you provide an `app_secret`, the library authenticates using HMAC-SHA256:

```rust
let client = ReverbClient::new(
    "app-key",
    "app-secret",  // Used for HMAC authentication
    "https://app.com/broadcasting/auth",
    "reverb.example.com",
    true,
);
```

### Client-Side (Endpoint)

If no secret is provided or for additional security, authentication is fetched from your Laravel endpoint:

```rust
let client = ReverbClient::new(
    "app-key",
    "",  // Empty secret - will use endpoint auth
    "https://your-app.com/broadcasting/auth",
    "reverb.example.com",
    true,
);
```

## Triggering Client Events

Send events to channels (requires private or presence channel):

```rust
client.trigger_event(
    "private-chat",
    "typing",  // Will be prefixed with "client-" automatically
    serde_json::json!({ "user": "John" }),
).await?;
```

## Unsubscribing

```rust
client.unsubscribe("private-my-channel").await?;
```

## Disconnecting

```rust
client.disconnect().await?;
```

## Error Handling

The library uses `ReverbError` for all error types:

```rust
use reverb_rs::ReverbError;

match client.connect().await {
    Ok(_) => println!("Connected!"),
    Err(ReverbError::WebSocketError(e)) => eprintln!("WebSocket error: {}", e),
    Err(ReverbError::AuthError(msg)) => eprintln!("Auth failed: {}", msg),
    Err(e) => eprintln!("Other error: {}", e),
}
```

## Logging

The library uses `tracing` for logging. Enable debug logs to see connection details:

```rust
tracing_subscriber::fmt()
    .with_env_filter("reverb_rs=debug")
    .init();
```

## Requirements

- Rust 2024 edition
- Tokio runtime
- Laravel Reverb server (or any Pusher-compatible WebSocket server)

## License

MIT License
