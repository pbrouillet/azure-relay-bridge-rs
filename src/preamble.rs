use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Protocol version.
pub const MAJOR_VERSION: u8 = 1;
pub const MINOR_VERSION: u8 = 0;

/// Connection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionMode {
    /// TCP or Unix socket stream connection.
    Stream = 0,
    /// UDP datagram connection.
    Datagram = 1,
}

impl TryFrom<u8> for ConnectionMode {
    type Error = PreambleError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(ConnectionMode::Stream),
            1 => Ok(ConnectionMode::Datagram),
            _ => Err(PreambleError::InvalidMode(value)),
        }
    }
}

/// Preamble request (sender -> listener).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreambleRequest {
    pub mode: ConnectionMode,
    pub port_name: String,
}

/// Preamble response (listener -> sender).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreambleResponse {
    pub accepted: bool,
    pub mode: u8, // mode echo on success, error code on failure
}

#[derive(Debug, thiserror::Error)]
pub enum PreambleError {
    #[error("unsupported protocol version {major}.{minor}")]
    UnsupportedVersion { major: u8, minor: u8 },
    #[error("invalid connection mode: {0}")]
    InvalidMode(u8),
    #[error("port name too long: {0} bytes (max 255)")]
    PortNameTooLong(usize),
    #[error("connection rejected with error code {0}")]
    Rejected(u8),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Write a preamble request to a stream.
pub async fn write_request<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    request: &PreambleRequest,
) -> Result<(), PreambleError> {
    let port_name_bytes = request.port_name.as_bytes();
    if port_name_bytes.len() > 255 {
        return Err(PreambleError::PortNameTooLong(port_name_bytes.len()));
    }
    let mut buf = Vec::with_capacity(4 + port_name_bytes.len());
    buf.push(MAJOR_VERSION);
    buf.push(MINOR_VERSION);
    buf.push(request.mode as u8);
    buf.push(port_name_bytes.len() as u8);
    buf.extend_from_slice(port_name_bytes);
    writer.write_all(&buf).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a preamble request from a stream.
pub async fn read_request<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<PreambleRequest, PreambleError> {
    let mut header = [0u8; 4];
    reader.read_exact(&mut header).await?;
    let major = header[0];
    let minor = header[1];
    if major != MAJOR_VERSION {
        return Err(PreambleError::UnsupportedVersion { major, minor });
    }
    let mode = ConnectionMode::try_from(header[2])?;
    let name_len = header[3] as usize;
    let mut name_buf = vec![0u8; name_len];
    if name_len > 0 {
        reader.read_exact(&mut name_buf).await?;
    }
    let port_name =
        String::from_utf8(name_buf).map_err(|_| PreambleError::InvalidMode(0))?;
    Ok(PreambleRequest { mode, port_name })
}

/// Write a preamble response (success).
pub async fn write_response_ok<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    mode: ConnectionMode,
) -> Result<(), PreambleError> {
    writer
        .write_all(&[MAJOR_VERSION, MINOR_VERSION, mode as u8])
        .await?;
    writer.flush().await?;
    Ok(())
}

/// Write a preamble response (error/rejection).
pub async fn write_response_err<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    error_code: u8,
) -> Result<(), PreambleError> {
    writer.write_all(&[0, 0, error_code]).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a preamble response from a stream.
pub async fn read_response<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<PreambleResponse, PreambleError> {
    let mut buf = [0u8; 3];
    reader.read_exact(&mut buf).await?;
    let major = buf[0];
    if major == 0 {
        return Err(PreambleError::Rejected(buf[2]));
    }
    Ok(PreambleResponse {
        accepted: true,
        mode: buf[2],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn write_read_request_stream_round_trip() {
        let req = PreambleRequest {
            mode: ConnectionMode::Stream,
            port_name: "myport".into(),
        };
        let mut buf = Vec::new();
        write_request(&mut buf, &req).await.unwrap();
        assert_eq!(&buf, &[1, 0, 0, 6, b'm', b'y', b'p', b'o', b'r', b't']);
        let mut cursor = Cursor::new(&buf);
        let parsed = read_request(&mut cursor).await.unwrap();
        assert_eq!(parsed, req);
    }

    #[tokio::test]
    async fn write_read_request_datagram_round_trip() {
        let req = PreambleRequest {
            mode: ConnectionMode::Datagram,
            port_name: "udp1".into(),
        };
        let mut buf = Vec::new();
        write_request(&mut buf, &req).await.unwrap();
        assert_eq!(buf[2], 1); // datagram mode
        let mut cursor = Cursor::new(&buf);
        let parsed = read_request(&mut cursor).await.unwrap();
        assert_eq!(parsed, req);
    }

    #[tokio::test]
    async fn write_read_request_empty_port_name() {
        let req = PreambleRequest {
            mode: ConnectionMode::Stream,
            port_name: "".into(),
        };
        let mut buf = Vec::new();
        write_request(&mut buf, &req).await.unwrap();
        assert_eq!(&buf, &[1, 0, 0, 0]);
        let mut cursor = Cursor::new(&buf);
        let parsed = read_request(&mut cursor).await.unwrap();
        assert_eq!(parsed.port_name, "");
    }

    #[tokio::test]
    async fn port_name_too_long_fails() {
        let req = PreambleRequest {
            mode: ConnectionMode::Stream,
            port_name: "x".repeat(256),
        };
        let mut buf = Vec::new();
        let result = write_request(&mut buf, &req).await;
        assert!(matches!(result, Err(PreambleError::PortNameTooLong(256))));
    }

    #[tokio::test]
    async fn write_read_response_ok_round_trip() {
        let mut buf = Vec::new();
        write_response_ok(&mut buf, ConnectionMode::Stream)
            .await
            .unwrap();
        assert_eq!(&buf, &[1, 0, 0]);
        let mut cursor = Cursor::new(&buf);
        let resp = read_response(&mut cursor).await.unwrap();
        assert!(resp.accepted);
        assert_eq!(resp.mode, 0);
    }

    #[tokio::test]
    async fn write_read_response_err_round_trip() {
        let mut buf = Vec::new();
        write_response_err(&mut buf, 42).await.unwrap();
        assert_eq!(&buf, &[0, 0, 42]);
        let mut cursor = Cursor::new(&buf);
        let result = read_response(&mut cursor).await;
        assert!(matches!(result, Err(PreambleError::Rejected(42))));
    }

    #[tokio::test]
    async fn read_request_unsupported_version() {
        let buf = [2, 0, 0, 0];
        let mut cursor = Cursor::new(&buf[..]);
        let result = read_request(&mut cursor).await;
        assert!(matches!(
            result,
            Err(PreambleError::UnsupportedVersion {
                major: 2,
                minor: 0
            })
        ));
    }

    #[tokio::test]
    async fn read_request_invalid_mode() {
        let buf = [1, 0, 5, 0];
        let mut cursor = Cursor::new(&buf[..]);
        let result = read_request(&mut cursor).await;
        assert!(matches!(result, Err(PreambleError::InvalidMode(5))));
    }

    #[test]
    fn connection_mode_try_from() {
        assert_eq!(ConnectionMode::try_from(0).unwrap(), ConnectionMode::Stream);
        assert_eq!(
            ConnectionMode::try_from(1).unwrap(),
            ConnectionMode::Datagram
        );
        assert!(ConnectionMode::try_from(2).is_err());
    }

    #[tokio::test]
    async fn csharp_compat_stream_request() {
        // C# sends: version 1.0, mode 0 (stream), port name "29876"
        let csharp_bytes = [1u8, 0, 0, 5, b'2', b'9', b'8', b'7', b'6'];
        let mut cursor = Cursor::new(&csharp_bytes[..]);
        let req = read_request(&mut cursor).await.unwrap();
        assert_eq!(req.mode, ConnectionMode::Stream);
        assert_eq!(req.port_name, "29876");
    }

    #[tokio::test]
    async fn csharp_compat_datagram_request() {
        // C# sends: version 1.0, mode 1 (datagram), port name "29876U"
        let csharp_bytes = [1u8, 0, 1, 6, b'2', b'9', b'8', b'7', b'6', b'U'];
        let mut cursor = Cursor::new(&csharp_bytes[..]);
        let req = read_request(&mut cursor).await.unwrap();
        assert_eq!(req.mode, ConnectionMode::Datagram);
        assert_eq!(req.port_name, "29876U");
    }
}
