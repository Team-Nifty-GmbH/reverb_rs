use crate::error::ReverbError;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;

/// WebSocket connection handler
pub(crate) struct WebSocketConnection {
    pub socket: mpsc::Sender<Message>,
    pub _task_handle: tokio::task::JoinHandle<()>,
}

impl WebSocketConnection {
    pub async fn send(&self, message: Message) -> Result<(), ReverbError> {
        self.socket
            .send(message)
            .await
            .map_err(|e| ReverbError::SendError(e.to_string()))
    }
}
