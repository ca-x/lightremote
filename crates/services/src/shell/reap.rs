//! A fixed-size fallback supervisor owns children that miss the cleanup deadline.
//! No service permit or per-child waiting thread is retained by this supervisor.
//! A session cleanup token remains owned until the child is actually reaped.
use super::{ExitReporter, report_exit};
use anyhow::{Context, Result};
use portable_pty::Child;
use std::{
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::task::task_tracker::TaskTrackerToken;

pub(super) const PROCESS_LIMIT: usize = 64;
type Process = Box<dyn Child + Send + Sync>;
struct Pending {
    child: Process,
    reporter: Option<ExitReporter>,
    _permit: OwnedSemaphorePermit,
    _cleanup: Option<TaskTrackerToken>,
}
struct Reaper {
    slots: Arc<Semaphore>,
    tx: mpsc::SyncSender<Pending>,
}
static REAPER: OnceLock<Mutex<Option<Reaper>>> = OnceLock::new();

pub(super) struct Lease {
    tx: mpsc::SyncSender<Pending>,
    permit: OwnedSemaphorePermit,
}
impl Lease {
    pub fn reserve() -> Result<Self> {
        let mut state = REAPER
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map_err(|_| anyhow::anyhow!("PTY reaper unavailable"))?;
        if state.is_none() {
            let (tx, rx) = mpsc::sync_channel::<Pending>(PROCESS_LIMIT);
            std::thread::Builder::new()
                .name("lightremote-pty-supervisor".into())
                .spawn(move || supervise(rx))?;
            *state = Some(Reaper {
                slots: Arc::new(Semaphore::new(PROCESS_LIMIT)),
                tx,
            });
        }
        let reaper = state.as_ref().context("PTY reaper unavailable")?;
        let permit = reaper
            .slots
            .clone()
            .try_acquire_owned()
            .context("host PTY process limit reached")?;
        Ok(Self {
            tx: reaper.tx.clone(),
            permit,
        })
    }

    pub fn defer(
        self,
        child: Process,
        reporter: Option<ExitReporter>,
        cleanup: Option<TaskTrackerToken>,
    ) {
        let pending = Pending {
            child,
            reporter,
            _permit: self.permit,
            _cleanup: cleanup,
        };
        // One reserved permit per live shell or pending child proves there are
        // fewer than PROCESS_LIMIT queued jobs when this one is submitted.
        if let Err(error) = self.tx.try_send(pending) {
            // The supervisor only exits after all senders close. A disconnected
            // channel is an internal failure; make one last nonblocking attempt.
            let mut pending = match error {
                mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job) => job,
            };
            let _ = pending.child.kill();
            if let Ok(Some(status)) = pending.child.try_wait() {
                report_exit(pending.reporter.take(), &status);
            }
        }
    }
}

fn supervise(rx: mpsc::Receiver<Pending>) {
    let mut pending = Vec::<Pending>::with_capacity(PROCESS_LIMIT);
    loop {
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(job) => pending.push(job),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if pending.is_empty() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        while let Ok(job) = rx.try_recv() {
            pending.push(job);
        }
        let mut index = 0;
        while index < pending.len() {
            match pending[index].child.try_wait() {
                Ok(Some(status)) => {
                    let job = pending.swap_remove(index);
                    report_exit(job.reporter, &status);
                }
                // Do not spin or discard an unreaped child on a transient error.
                _ => {
                    index += 1;
                }
            }
        }
    }
}
