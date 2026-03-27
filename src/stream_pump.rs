use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tracing::debug;

const BUFFER_SIZE: usize = 65536; // 64KB, matching C#

/// Bidirectional async copy between two streams with half-close semantics.
///
/// Spawns two concurrent copy tasks (A→B and B→A). When one direction
/// reaches EOF, the write side of the other stream is shut down (half-close).
/// Returns when both directions are complete.
pub async fn run<A, B>(a: A, b: B) -> std::io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    B: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (a_read, a_write) = tokio::io::split(a);
    let (b_read, b_write) = tokio::io::split(b);

    let a_to_b = tokio::spawn(copy_with_shutdown(a_read, b_write, "A→B"));
    let b_to_a = tokio::spawn(copy_with_shutdown(b_read, a_write, "B→A"));

    // Pin both handles for use with select!
    tokio::pin!(a_to_b);
    tokio::pin!(b_to_a);

    // When one direction completes, give the other 5 seconds then abort it.
    let (a_bytes, b_bytes) = tokio::select! {
        result = &mut a_to_b => {
            let a_bytes = result.map_err(std::io::Error::other)??;
            let b_bytes = match tokio::time::timeout(
                std::time::Duration::from_secs(5), b_to_a
            ).await {
                Ok(Ok(Ok(n))) => n,
                _ => 0,
            };
            (a_bytes, b_bytes)
        }
        result = &mut b_to_a => {
            let b_bytes = result.map_err(std::io::Error::other)??;
            let a_bytes = match tokio::time::timeout(
                std::time::Duration::from_secs(5), a_to_b
            ).await {
                Ok(Ok(Ok(n))) => n,
                _ => 0,
            };
            (a_bytes, b_bytes)
        }
    };

    Ok((a_bytes, b_bytes))
}

/// Copy from reader to writer with a 64KB buffer.
/// On EOF, shuts down the writer (half-close).
async fn copy_with_shutdown<R, W>(
    mut reader: R,
    mut writer: W,
    direction: &str,
) -> std::io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; BUFFER_SIZE];
    let mut total: u64 = 0;

    loop {
        let n = tokio::io::AsyncReadExt::read(&mut reader, &mut buf).await?;
        if n == 0 {
            debug!(direction, total, "EOF reached, shutting down writer");
            writer.shutdown().await?;
            break;
        }
        tokio::io::AsyncWriteExt::write_all(&mut writer, &buf[..n]).await?;
        tokio::io::AsyncWriteExt::flush(&mut writer).await?;
        total += n as u64;
    }

    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn copy_with_shutdown_transfers_data() {
        let (mut writer_end, reader_end) = duplex(65536);
        let (_output_reader, output_end) = duplex(65536);

        tokio::io::AsyncWriteExt::write_all(&mut writer_end, b"hello world")
            .await
            .unwrap();
        drop(writer_end); // EOF

        let bytes = copy_with_shutdown(reader_end, output_end, "test")
            .await
            .unwrap();
        assert_eq!(bytes, 11);
    }

    #[tokio::test]
    async fn copy_with_shutdown_large_transfer() {
        let data = vec![0xABu8; 200_000]; // 200KB, spans multiple 64KB buffers
        let (mut writer_end, reader_end) = duplex(65536);
        let (output_reader, output_end) = duplex(65536);

        let data_clone = data.clone();
        tokio::spawn(async move {
            tokio::io::AsyncWriteExt::write_all(&mut writer_end, &data_clone)
                .await
                .unwrap();
            drop(writer_end);
        });

        // Read from output to consume
        let read_handle = tokio::spawn(async move {
            let mut buf = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(
                &mut tokio::io::BufReader::new(output_reader),
                &mut buf,
            )
            .await
            .unwrap();
            buf
        });

        let bytes = copy_with_shutdown(reader_end, output_end, "test")
            .await
            .unwrap();
        assert_eq!(bytes, 200_000);

        let received = read_handle.await.unwrap();
        assert_eq!(received, data);
    }

    #[test]
    fn buffer_size_is_64kb() {
        assert_eq!(BUFFER_SIZE, 65536);
    }

    #[tokio::test]
    async fn run_bidirectional_transfer() {
        let (a_client, a_server) = duplex(65536);
        let (b_client, b_server) = duplex(65536);

        let a_data = b"hello from A";
        let b_data = b"hello from B";

        let a_writer = tokio::spawn(async move {
            let mut w = a_client;
            tokio::io::AsyncWriteExt::write_all(&mut w, a_data)
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::shutdown(&mut w).await.unwrap();
            let mut buf = vec![0u8; 1024];
            let n = tokio::io::AsyncReadExt::read(&mut w, &mut buf)
                .await
                .unwrap();
            buf.truncate(n);
            buf
        });

        let b_writer = tokio::spawn(async move {
            let mut w = b_client;
            tokio::io::AsyncWriteExt::write_all(&mut w, b_data)
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::shutdown(&mut w).await.unwrap();
            let mut buf = vec![0u8; 1024];
            let n = tokio::io::AsyncReadExt::read(&mut w, &mut buf)
                .await
                .unwrap();
            buf.truncate(n);
            buf
        });

        let (a_to_b, b_to_a) = run(a_server, b_server).await.unwrap();

        assert_eq!(a_to_b, a_data.len() as u64);
        assert_eq!(b_to_a, b_data.len() as u64);

        let a_received = a_writer.await.unwrap();
        let b_received = b_writer.await.unwrap();
        assert_eq!(a_received, b_data);
        assert_eq!(b_received, a_data);
    }

    #[tokio::test]
    async fn run_one_sided_transfer() {
        let (mut a_write, a_read) = duplex(65536);
        let (b_read, b_write) = duplex(65536);

        let data = b"one-way data";
        tokio::io::AsyncWriteExt::write_all(&mut a_write, data)
            .await
            .unwrap();
        drop(a_write); // EOF on A's write side

        let consumer = tokio::spawn(async move {
            let mut buf = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(
                &mut tokio::io::BufReader::new(b_read),
                &mut buf,
            )
            .await
            .unwrap();
            buf
        });

        let (a_to_b, b_to_a) = run(a_read, b_write).await.unwrap();
        assert_eq!(a_to_b, data.len() as u64);
        assert_eq!(b_to_a, 0);

        let received = consumer.await.unwrap();
        assert_eq!(received, data);
    }

    #[tokio::test]
    async fn run_empty_transfer() {
        let (a_write, a_read) = duplex(65536);
        let (b_read, b_write) = duplex(65536);

        drop(a_write); // immediate EOF
        drop(b_read); // no reader on B side

        let (a_to_b, _b_to_a) = run(a_read, b_write).await.unwrap();
        assert_eq!(a_to_b, 0);
        // _b_to_a might be 0 or could error due to broken pipe, either is acceptable
    }

    #[tokio::test]
    async fn copy_with_shutdown_zero_bytes() {
        let (writer_end, reader_end) = duplex(65536);
        let (_output_reader, output_end) = duplex(65536);
        drop(writer_end); // immediate EOF

        let bytes = copy_with_shutdown(reader_end, output_end, "test")
            .await
            .unwrap();
        assert_eq!(bytes, 0);
    }
}
