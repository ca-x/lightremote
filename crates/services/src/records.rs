use anyhow::{Result, bail, ensure};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

pub const CONTROL_LIMIT: usize = 1024 * 1024;
pub const FRAME_LIMIT: usize = 8 * 1024 * 1024;
pub const INPUT_LIMIT: usize = 64 * 1024;
pub const CHUNK: usize = 16 * 1024;

pub async fn read_record<R: AsyncRead + Unpin>(
    io: &mut R,
    limit: usize,
    idle: Duration,
) -> Result<Option<Vec<u8>>> {
    // A healthy attended/interactive session may be silent indefinitely. The
    // carrier owns peer liveness; only a partially started record has an I/O
    // deadline. Reading the first byte separately still verifies clean FIN.
    let mut header = [0_u8; 4];
    if io.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    timeout(idle, io.read_exact(&mut header[1..])).await??;
    let len = u32::from_be_bytes(header) as usize;
    ensure!(len > 0 && len <= limit, "record length out of bounds");
    let mut body = vec![0; len];
    timeout(idle, io.read_exact(&mut body)).await??;
    Ok(Some(body))
}

pub async fn write_record<W: AsyncWrite + Unpin>(
    io: &mut W,
    body: &[u8],
    limit: usize,
    idle: Duration,
) -> Result<()> {
    ensure!(
        !body.is_empty() && body.len() <= limit,
        "record length out of bounds"
    );
    let len = u32::try_from(body.len())?;
    timeout(idle, async {
        io.write_all(&len.to_be_bytes()).await?;
        io.write_all(body).await?;
        io.flush().await
    })
    .await??;
    Ok(())
}

pub async fn expect_fin<R: AsyncRead + Unpin>(io: &mut R, idle: Duration) -> Result<()> {
    let mut extra = [0_u8; 1];
    if timeout(idle, io.read(&mut extra)).await?? != 0 {
        bail!("unexpected bytes after request");
    }
    Ok(())
}
