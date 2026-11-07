//! Cancellation and ownership for blocking OS operations.
use crate::permissions::Lease;
use anyhow::{Context, Result, ensure};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::task::AbortHandle;
use tokio_util::task::TaskTracker;

struct Group {
    closed: Mutex<bool>,
    tasks: TaskTracker,
}

#[derive(Clone)]
pub(crate) struct Work {
    group: Arc<Group>,
    lease: Option<Lease>,
}
impl Default for Work {
    fn default() -> Self {
        Self {
            group: Arc::new(Group {
                closed: Mutex::new(false),
                tasks: TaskTracker::new(),
            }),
            lease: None,
        }
    }
}
impl Work {
    pub(crate) fn authorized(&self, lease: Lease) -> Self {
        Self {
            group: self.group.clone(),
            lease: Some(lease),
        }
    }

    /// Queued operations are aborted when the request is dropped; a running
    /// closure checks its original grant generation before beginning OS work.
    /// All started closures remain tracked until actual completion.
    pub(crate) async fn run<T: Send + 'static>(
        &self,
        run: impl FnOnce(&Access) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let access = Access {
            cancelled: cancelled.clone(),
            lease: self.lease.clone(),
        };
        let task = {
            let closed = self
                .group
                .closed
                .lock()
                .expect("blocking admission lock poisoned");
            ensure!(!*closed, "host services are shutting down");
            self.group.tasks.spawn_blocking(move || {
                access.check()?;
                run(&access)
            })
        };
        let _cancel = Cancel {
            cancelled,
            task: task.abort_handle(),
        };
        task.await
            .context("host OS operation stopped unexpectedly")?
    }

    pub(crate) async fn shutdown(&self) {
        {
            let mut closed = self
                .group
                .closed
                .lock()
                .expect("blocking admission lock poisoned");
            *closed = true;
            self.group.tasks.close();
        }
        self.group.tasks.wait().await;
    }
}

pub(crate) struct Access {
    cancelled: Arc<AtomicBool>,
    lease: Option<Lease>,
}
impl Access {
    /// Recheck after waiting for a resource mutex and before touching the OS.
    pub(crate) fn check(&self) -> Result<()> {
        ensure!(
            !self.cancelled.load(Ordering::Acquire),
            "OS operation was cancelled"
        );
        if let Some(lease) = &self.lease {
            ensure!(
                lease.valid(),
                "permission was revoked before OS operation began"
            );
        }
        Ok(())
    }
}
struct Cancel {
    cancelled: Arc<AtomicBool>,
    task: AbortHandle,
}
impl Drop for Cancel {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{ApprovalMode, Permissions, Scope};
    #[tokio::test]
    async fn recheck_after_resource_wait_rejects_old_grant_generation() {
        let permissions = Permissions::new([Scope::ClipboardWrite], ApprovalMode::Unattended);
        let work = Work::default().authorized(permissions.lease(&[Scope::ClipboardWrite]).unwrap());
        let (started, running) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let writes = Arc::new(AtomicBool::new(false));
        let written = writes.clone();
        let worker = work.clone();
        let task = tokio::spawn(async move {
            worker
                .run(move |access| {
                    let _ = started.send(());
                    let _ = blocked.recv(); // Models waiting for an OS resource mutex.
                    access.check()?;
                    written.store(true, Ordering::Release);
                    Ok(())
                })
                .await
        });
        running.await.unwrap();
        permissions.revoke(&[Scope::ClipboardWrite]).unwrap();
        let id = permissions
            .request(&[Scope::ClipboardWrite])
            .unwrap()
            .pending
            .unwrap()
            .id;
        permissions.approve(id, None).unwrap();
        release.send(()).unwrap();
        assert!(task.await.unwrap().is_err());
        assert!(!writes.load(Ordering::Acquire));
        work.shutdown().await;
    }
    #[tokio::test]
    async fn shutdown_waits_for_started_os_work_after_caller_cancellation() {
        let work = Work::default();
        let (started, running) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let worker = work.clone();
        let task = tokio::spawn(async move {
            worker
                .run(move |_| {
                    let _ = started.send(());
                    let _ = blocked.recv();
                    Ok(())
                })
                .await
        });
        running.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let mut drain = Box::pin(work.shutdown());
        tokio::select! { biased; _ = &mut drain => panic!("shutdown detached a started OS operation"), _ = tokio::task::yield_now() => {} }
        release.send(()).unwrap();
        drain.await;
        assert!(work.run(|_| Ok(())).await.is_err());
    }
}
