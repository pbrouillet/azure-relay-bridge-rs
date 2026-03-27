//! WebSocket-backed byte stream for Azure Relay Hybrid Connections.
//!
//! [`HybridConnectionStream`] wraps a `tokio-tungstenite` WebSocket connection
//! and presents a byte-oriented interface via [`tokio::io::AsyncRead`] and
//! [`tokio::io::AsyncWrite`], analogous to the C# `HybridConnectionStream`
//! from the Azure Relay .NET SDK.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use futures_util::{Sink, Stream};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, trace};

use crate::error::Result;

/// The concrete WebSocket stream type used by the relay.
type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Controls the WebSocket frame type used for writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// Send data as WebSocket Binary frames (default).
    Binary,
    /// Send data as WebSocket Text frames.
    Text,
}

/// A bidirectional byte stream over an Azure Relay Hybrid Connection.
///
/// Wraps a WebSocket connection and exposes it as an [`AsyncRead`] +
/// [`AsyncWrite`] byte stream, transparently handling WebSocket framing.
///
/// # Half-close semantics
///
/// Calling [`shutdown`](Self::shutdown) sends a WebSocket Close frame, ending
/// the write side. The read side remains open so the remote peer can finish
/// sending data.
pub struct HybridConnectionStream {
    inner: WsStream,
    tracking_id: String,
    write_mode: WriteMode,
    /// Buffer for partially-consumed incoming messages.
    read_buf: BytesMut,
    is_read_closed: bool,
    is_write_closed: bool,
}

impl HybridConnectionStream {
    /// Creates a new `HybridConnectionStream` wrapping an established WebSocket.
    pub(crate) fn new(ws: WsStream, tracking_id: String) -> Self {
        debug!(tracking_id = %tracking_id, "HybridConnectionStream created");
        Self {
            inner: ws,
            tracking_id,
            write_mode: WriteMode::Binary,
            read_buf: BytesMut::new(),
            is_read_closed: false,
            is_write_closed: false,
        }
    }

    /// Returns the tracking ID for this connection.
    pub fn tracking_id(&self) -> &str {
        &self.tracking_id
    }

    /// Returns the current write mode (Binary or Text).
    pub fn write_mode(&self) -> WriteMode {
        self.write_mode
    }

    /// Sets the write mode for subsequent writes.
    pub fn set_write_mode(&mut self, mode: WriteMode) {
        self.write_mode = mode;
    }

    /// Sends a WebSocket Close frame with normal closure code (1000),
    /// signaling end-of-stream to the remote side.
    ///
    /// After shutdown, writes will fail but reads continue working
    /// (half-close semantics).
    pub async fn shutdown(&mut self) -> Result<()> {
        if self.is_write_closed {
            return Ok(());
        }

        use futures_util::SinkExt;
        let close = Message::Close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: "Normal Closure".into(),
        }));
        self.inner.send(close).await?;
        self.is_write_closed = true;
        debug!(tracking_id = %self.tracking_id, "Write side shut down");
        Ok(())
    }

    /// Sends a Close frame (if not already sent) and drops the WebSocket.
    pub async fn close(mut self) -> Result<()> {
        if !self.is_write_closed {
            self.shutdown().await?;
        }

        use futures_util::SinkExt;
        SinkExt::close(&mut self.inner).await?;
        debug!(tracking_id = %self.tracking_id, "Stream fully closed");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AsyncRead – presents incoming WebSocket messages as a byte stream
// ---------------------------------------------------------------------------

impl AsyncRead for HybridConnectionStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        // Return buffered data first.
        if !this.read_buf.is_empty() {
            let len = std::cmp::min(buf.remaining(), this.read_buf.len());
            buf.put_slice(&this.read_buf.split_to(len));
            return Poll::Ready(Ok(()));
        }

        // EOF was already observed.
        if this.is_read_closed {
            return Poll::Ready(Ok(()));
        }

        // Poll the WebSocket for the next message.
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(msg))) => match msg {
                Message::Binary(data) => {
                    let len = std::cmp::min(buf.remaining(), data.len());
                    buf.put_slice(&data[..len]);
                    if len < data.len() {
                        this.read_buf.extend_from_slice(&data[len..]);
                    }
                    Poll::Ready(Ok(()))
                }
                Message::Text(text) => {
                    let data = text.as_bytes();
                    let len = std::cmp::min(buf.remaining(), data.len());
                    buf.put_slice(&data[..len]);
                    if len < data.len() {
                        this.read_buf.extend_from_slice(&data[len..]);
                    }
                    Poll::Ready(Ok(()))
                }
                Message::Close(_) => {
                    trace!(tracking_id = %this.tracking_id, "Received Close frame");
                    this.is_read_closed = true;
                    Poll::Ready(Ok(()))
                }
                // Ping/Pong are handled internally by tungstenite;
                // re-poll for the next data-bearing message.
                _ => {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            },
            Poll::Ready(Some(Err(e))) => {
                Poll::Ready(Err(io::Error::other(e)))
            }
            Poll::Ready(None) => {
                this.is_read_closed = true;
                Poll::Ready(Ok(()))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

// ---------------------------------------------------------------------------
// AsyncWrite – sends bytes as WebSocket frames
// ---------------------------------------------------------------------------

impl AsyncWrite for HybridConnectionStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();

        if this.is_write_closed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "write side is closed",
            )));
        }

        // Ensure the sink is ready to accept a message.
        match Pin::new(&mut this.inner).poll_ready(cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(e)) => {
                return Poll::Ready(Err(io::Error::other(e)));
            }
            Poll::Pending => return Poll::Pending,
        }

        let msg = match this.write_mode {
            WriteMode::Binary => Message::Binary(Bytes::copy_from_slice(buf)),
            WriteMode::Text => {
                let text = String::from_utf8_lossy(buf).into_owned();
                Message::Text(text.into())
            }
        };

        match Pin::new(&mut this.inner).start_send(msg) {
            Ok(()) => Poll::Ready(Ok(buf.len())),
            Err(e) => Poll::Ready(Err(io::Error::other(e))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner)
            .poll_flush(cx)
            .map_err(io::Error::other)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        if !this.is_write_closed {
            // Ensure the sink is ready before sending the Close frame.
            match Pin::new(&mut this.inner).poll_ready(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(e)) => {
                    return Poll::Ready(Err(io::Error::other(e)));
                }
                Poll::Pending => return Poll::Pending,
            }

            let close = Message::Close(Some(CloseFrame {
                code: CloseCode::Normal,
                reason: "Normal Closure".into(),
            }));

            match Pin::new(&mut this.inner).start_send(close) {
                Ok(()) => {
                    this.is_write_closed = true;
                }
                Err(e) => {
                    return Poll::Ready(Err(io::Error::other(e)));
                }
            }
        }

        // Flush to ensure the Close frame is sent on the wire.
        Pin::new(&mut this.inner)
            .poll_flush(cx)
            .map_err(io::Error::other)
    }
}

impl std::fmt::Debug for HybridConnectionStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HybridConnectionStream")
            .field("tracking_id", &self.tracking_id)
            .field("write_mode", &self.write_mode)
            .field("is_read_closed", &self.is_read_closed)
            .field("is_write_closed", &self.is_write_closed)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_mode_default_is_binary() {
        let mode = WriteMode::Binary;
        assert_eq!(mode, WriteMode::Binary);
        assert_ne!(mode, WriteMode::Text);
    }

    #[test]
    fn write_mode_text_variant() {
        let mode = WriteMode::Text;
        assert_eq!(mode, WriteMode::Text);
    }

    #[test]
    fn write_mode_clone_and_copy() {
        let mode = WriteMode::Binary;
        let cloned = mode.clone();
        let copied = mode; // Copy
        assert_eq!(cloned, copied);
    }

    #[test]
    fn write_mode_debug() {
        assert_eq!(format!("{:?}", WriteMode::Binary), "Binary");
        assert_eq!(format!("{:?}", WriteMode::Text), "Text");
    }
}
