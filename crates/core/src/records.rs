//! Bounded application records, independent of TMP DATA boundaries.
//!
//! Reads/writes retain partial framing progress in their future. If a caller
//! cancels that future after partial I/O, it must reset/close the stream rather
//! than starting a new record on the same bytes.
use anyhow::{Result, ensure};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
pub const MAX_RECORD: usize = 8 * 1024 * 1024;
pub const MAX_CONTROL: usize = 1024 * 1024;
pub async fn read<R: AsyncRead + Unpin>(io: &mut R, limit: usize) -> Result<Option<Vec<u8>>> {
    let mut prefix = [0; 4];
    if io.read(&mut prefix[..1]).await? == 0 {
        return Ok(None);
    }
    io.read_exact(&mut prefix[1..]).await?;
    let n = u32::from_be_bytes(prefix) as usize;
    ensure!(
        n > 0 && n <= limit.min(MAX_RECORD),
        "application record length"
    );
    let mut data = vec![0; n];
    io.read_exact(&mut data).await?;
    Ok(Some(data))
}
pub async fn write<W: AsyncWrite + Unpin>(io: &mut W, data: &[u8]) -> Result<()> {
    ensure!(
        !data.is_empty() && data.len() <= MAX_RECORD,
        "application record length"
    );
    io.write_all(&(data.len() as u32).to_be_bytes()).await?;
    io.write_all(data).await?;
    Ok(())
}
pub async fn json<W: AsyncWrite + Unpin>(io: &mut W, value: &serde_json::Value) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    ensure!(data.len() <= MAX_CONTROL, "control record length");
    write(io, &data).await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn rejects_truncated_and_oversized_records() {
        assert!(read(&mut &b"\0\0"[..], 100).await.is_err());
        assert!(read(&mut &b"\0\0\x01\0"[..], 100).await.is_err());
        assert!(read(&mut &b"\0\0\0\x02a"[..], 100).await.is_err());
        assert!(read(&mut &b""[..], 100).await.unwrap().is_none());
    }
}
