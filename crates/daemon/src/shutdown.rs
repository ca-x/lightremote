//! An SCM stop is durable even when it arrives before the host polls shutdown.
use anyhow::{Context, Result};
use tokio::sync::watch;

pub async fn wait(mut stopped: watch::Receiver<bool>) -> Result<()> {
    while !*stopped.borrow_and_update() {
        stopped
            .changed()
            .await
            .context("SCM stop channel closed unexpectedly")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_waits_and_repeated_controls_are_idempotent() {
        let (stop, receiver) = watch::channel(false);
        let waiting = wait(receiver);
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut waiting)
                .await
                .is_err()
        );
        stop.send_replace(true);
        stop.send_replace(true);
        waiting.await.unwrap();
    }

    #[tokio::test]
    async fn early_stop_is_retained_and_control_channel_loss_is_an_error() {
        let (stop, receiver) = watch::channel(false);
        stop.send_replace(true);
        drop(stop);
        wait(receiver).await.unwrap();
        let (stop, receiver) = watch::channel(false);
        drop(stop);
        assert!(wait(receiver).await.is_err());
    }
}
