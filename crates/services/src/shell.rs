use crate::BoxServiceIo;
mod reap;
#[cfg(unix)]
mod unix;

use crate::records::CHUNK;
use anyhow::{Context, Result, ensure};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{OwnedSemaphorePermit, broadcast, mpsc},
    time::timeout,
};

pub(crate) const PROCESS_LIMIT: usize = reap::PROCESS_LIMIT;

type Master = Arc<Mutex<Box<dyn MasterPty + Send>>>;
#[derive(Default)]
pub(crate) struct Shells(Mutex<HashMap<u64, Master>>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShellOptions {
    pub stream_id: u64,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}
fn default_cols() -> u16 {
    80
}
fn default_rows() -> u16 {
    24
}

pub(crate) fn size(cols: u16, rows: u16) -> Result<PtySize> {
    ensure!(
        (1..=1000).contains(&cols) && (1..=1000).contains(&rows),
        "terminal dimensions out of bounds"
    );
    Ok(PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    })
}

impl Shells {
    pub fn resize(&self, stream_id: u64, cols: u16, rows: u16) -> Result<Value> {
        let size = size(cols, rows)?;
        let master = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("shell registry unavailable"))?
            .get(&stream_id)
            .cloned()
            .context("unknown shell stream")?;
        master
            .lock()
            .map_err(|_| anyhow::anyhow!("shell unavailable"))?
            .resize(size)?;
        Ok(json!({}))
    }

    pub async fn serve(
        self: &Arc<Self>,
        options: ShellOptions,
        program: Option<PathBuf>,
        io: BoxServiceIo,
        idle: Duration,
        notifications: broadcast::Sender<Value>,
        slot: Arc<OwnedSemaphorePermit>,
    ) -> Result<()> {
        let pty_size = size(options.cols, options.rows)?;
        // Spawning may load user profiles; keep it off the Tokio worker threads.
        let mut session =
            tokio::task::spawn_blocking(move || Session::spawn(pty_size, program, slot)).await??;
        {
            let mut shells = self
                .0
                .lock()
                .map_err(|_| anyhow::anyhow!("shell registry unavailable"))?;
            ensure!(
                !shells.contains_key(&options.stream_id),
                "duplicate shell stream id"
            );
            shells.insert(options.stream_id, session.master.clone());
        }
        session.reporter = Some((notifications, options.stream_id));
        let _registration = Registration {
            shells: self.clone(),
            id: options.stream_id,
        };
        let result = {
            let (mut input, mut output) = tokio::io::split(io);
            let input_tx = session.input.take().context("shell input unavailable")?;
            let receive = async move {
                let mut bytes = vec![0; CHUNK];
                loop {
                    let n = timeout(idle, input.read(&mut bytes)).await??;
                    if n == 0 {
                        // FIN closes only PTY input. Drain accepted input before
                        // delivering canonical EOF; keep reading shell output.
                        drop(input_tx);
                        return Ok::<_, anyhow::Error>(());
                    }
                    timeout(idle, input_tx.send(bytes[..n].to_vec()))
                        .await?
                        .map_err(|_| anyhow::anyhow!("shell input closed"))?;
                }
            };
            let send = async {
                while let Some(bytes) = timeout(idle, session.output.recv()).await? {
                    timeout(idle, output.write_all(&bytes?)).await??;
                }
                timeout(idle, output.shutdown()).await??;
                Ok::<_, anyhow::Error>(())
            };
            tokio::pin!(receive, send);
            tokio::select! {
                result=&mut receive=>match result {
                    Ok(())=>timeout(idle,&mut send).await.context("shell output drain timed out")?,
                    Err(error)=>Err(error),
                },
                result=&mut send=>result,
            }
        };
        // The same cleanup runs on success, disconnect, timeout, or Drop.
        if let Some(cleanup) = session.stop() {
            tokio::task::spawn_blocking(move || cleanup.run()).await??;
        }
        result
    }
}

struct Registration {
    shells: Arc<Shells>,
    id: u64,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut shells) = self.shells.0.lock() {
            shells.remove(&self.id);
        }
    }
}

type ExitReporter = (broadcast::Sender<Value>, u64);
fn report_exit(reporter: Option<ExitReporter>, status: &portable_pty::ExitStatus) {
    if let Some((notifications, id)) = reporter {
        let _ = notifications.send(json!({"event":"shell.exit","data":{
            "stream_id":id,"code":status.exit_code(),"signal":status.signal()
        }}));
    }
}

const IO_POLL: Duration = Duration::from_millis(5);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);

struct Session {
    reporter: Option<ExitReporter>,
    slot: Arc<OwnedSemaphorePermit>,
    master: Master,
    child: Option<Box<dyn Child + Send + Sync>>,
    running: Arc<AtomicBool>,
    input: Option<mpsc::Sender<Vec<u8>>>,
    output: mpsc::Receiver<std::io::Result<Vec<u8>>>,
    workers: Vec<JoinHandle<()>>,
    lease: Option<reap::Lease>,
    #[cfg(unix)]
    tree: Option<unix::Tree>,
}

impl Session {
    fn spawn(
        size: PtySize,
        program: Option<PathBuf>,
        slot: Arc<OwnedSemaphorePermit>,
    ) -> Result<Self> {
        let lease = reap::Lease::reserve()?;
        let pair = native_pty_system().openpty(size)?;
        #[cfg(unix)]
        let (reader, writer) = unix::io_handles(pair.master.as_ref())?;
        #[cfg(not(unix))]
        let (reader, writer) = (pair.master.try_clone_reader()?, pair.master.take_writer()?);
        let command = match program {
            Some(program) => CommandBuilder::new(program),
            None => CommandBuilder::new_default_prog(),
        };
        let child = pair.slave.spawn_command(command)?;
        drop(pair.slave);
        let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>(4);
        let (output_tx, output_rx) = mpsc::channel(4);
        let running = Arc::new(AtomicBool::new(true));
        let mut session = Self {
            reporter: None,
            slot,
            master: Arc::new(Mutex::new(pair.master)),
            child: Some(child),
            running: running.clone(),
            input: Some(input_tx),
            output: output_rx,
            workers: Vec::new(),
            lease: Some(lease),
            #[cfg(unix)]
            tree: None,
        };
        #[cfg(unix)]
        {
            let pid = session
                .child
                .as_ref()
                .and_then(|child| child.process_id())
                .context("PTY child has no process ID")?;
            session.tree = Some(unix::Tree::new(pid)?);
        }
        let reader_running = running.clone();
        session.workers.push(
            std::thread::Builder::new()
                .name("roc-remote-pty-read".into())
                .spawn(move || read_output(reader, output_tx, reader_running))?,
        );
        let master = session.master.clone();
        session.workers.push(
            std::thread::Builder::new()
                .name("roc-remote-pty-write".into())
                .spawn(move || write_input(writer, input_rx, running, master))?,
        );
        Ok(session)
    }

    fn stop(&mut self) -> Option<Cleanup> {
        let child = self.child.take()?;
        self.running.store(false, Ordering::Release);
        self.input.take();
        self.output.close();
        for worker in &self.workers {
            worker.thread().unpark();
        }
        Some(Cleanup {
            child: Some(child),
            reporter: self.reporter.take(),
            workers: std::mem::take(&mut self.workers),
            _slot: self.slot.clone(),
            lease: self.lease.take(),
            #[cfg(unix)]
            tree: self.tree.take(),
        })
    }
}

fn read_output(
    mut reader: Box<dyn Read + Send>,
    output: mpsc::Sender<std::io::Result<Vec<u8>>>,
    running: Arc<AtomicBool>,
) {
    let mut bytes = vec![0; CHUNK];
    while running.load(Ordering::Acquire) {
        match reader.read(&mut bytes) {
            Ok(0) => break,
            Ok(n) => {
                if output.blocking_send(Ok(bytes[..n].to_vec())).is_err() {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::park_timeout(IO_POLL)
            }
            #[cfg(unix)]
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) => {
                let _ = output.blocking_send(Err(error));
                break;
            }
        }
    }
}

fn write_input(
    mut writer: Box<dyn Write + Send>,
    mut input: mpsc::Receiver<Vec<u8>>,
    running: Arc<AtomicBool>,
    _master: Master,
) {
    while running.load(Ordering::Acquire) {
        let bytes = match input.try_recv() {
            Ok(bytes) => bytes,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                #[cfg(unix)]
                finish_canonical_input(writer.as_mut(), &_master, &running);
                break;
            }
            Err(mpsc::error::TryRecvError::Empty) => {
                std::thread::park_timeout(IO_POLL);
                continue;
            }
        };
        if !write_cancellable(writer.as_mut(), &bytes, &running) {
            break;
        }
    }
}

#[cfg(unix)]
fn finish_canonical_input(writer: &mut dyn Write, master: &Master, running: &AtomicBool) {
    let probe = master
        .lock()
        .ok()
        .and_then(|master| unix::input_probe(master.as_ref()).ok());
    let Some(probe) = probe else {
        return;
    };
    while running.load(Ordering::Acquire) {
        // Writing the accepted bytes is not the same as the program consuming
        // them. Bash/readline toggles ICANON while parsing queued commands and
        // can discard an EOF queued before the foreground reader has drained
        // that input. Wait on the slave queue before injecting its EOF marker.
        let Ok(queued) = unix::queued_input(&probe) else {
            return;
        };
        let Ok(master) = master.lock() else {
            return;
        };
        if master.process_group_leader().is_none() {
            return;
        }
        let eof = if queued == 0 {
            unix::eof_bytes(master.as_ref())
        } else {
            None
        };
        drop(master);
        if let Some(eof) = eof {
            write_cancellable(writer, &eof, running);
            return;
        }
        std::thread::park_timeout(IO_POLL);
    }
}

fn write_cancellable(writer: &mut dyn Write, mut bytes: &[u8], running: &AtomicBool) -> bool {
    while !bytes.is_empty() && running.load(Ordering::Acquire) {
        match writer.write(bytes) {
            Ok(0) => return false,
            Ok(n) => bytes = &bytes[n..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::park_timeout(IO_POLL)
            }
            Err(_) => return false,
        }
    }
    bytes.is_empty()
}

struct Cleanup {
    child: Option<Box<dyn Child + Send + Sync>>,
    reporter: Option<ExitReporter>,
    workers: Vec<JoinHandle<()>>,
    _slot: Arc<OwnedSemaphorePermit>,
    lease: Option<reap::Lease>,
    #[cfg(unix)]
    tree: Option<unix::Tree>,
}
impl Cleanup {
    fn run(mut self) -> Result<()> {
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        let mut child = self.child.take().context("PTY cleanup child unavailable")?;
        let mut result = Ok(());
        #[cfg(unix)]
        if let Some(tree) = &self.tree {
            // Half the deadline remains available for I/O joins and child reap.
            result = tree.terminate(Instant::now() + CLEANUP_TIMEOUT / 2);
        }
        let _ = child.kill();
        let mut status = None;
        loop {
            match child.try_wait() {
                Ok(Some(exit)) => {
                    status = Some(exit);
                    break;
                }
                Err(error) => {
                    result = Err(error.into());
                    break;
                }
                Ok(None) => {}
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(IO_POLL);
        }
        for worker in self.workers.drain(..) {
            while !worker.is_finished() && Instant::now() < deadline {
                std::thread::sleep(IO_POLL);
            }
            if worker.is_finished() {
                if worker.join().is_err() {
                    result = Err(anyhow::anyhow!("PTY I/O worker panicked"));
                }
            } else {
                result = Err(anyhow::anyhow!("PTY I/O cleanup timed out"));
            }
        }
        if let Some(status) = status {
            report_exit(self.reporter.take(), &status);
        } else {
            // A process in an uninterruptible OS wait may miss the deadline.
            // A single bounded supervisor keeps its wait handle and eventually
            // reaps it, without retaining this stream permit or worker thread.
            if let Some(lease) = self.lease.take() {
                lease.defer(child, self.reporter.take());
            }
            return Err(anyhow::anyhow!(
                "PTY child did not exit before cleanup deadline"
            ));
        }
        result
    }
}

// If dispatching the cleanup thread fails, dropping its closure must still
// terminate the session and transfer the child's wait handle to the supervisor.
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            #[cfg(unix)]
            if let Some(tree) = &self.tree {
                let _ = tree.terminate(Instant::now() + Duration::from_millis(100));
            }
            let _ = child.kill();
            if let Some(lease) = self.lease.take() {
                lease.defer(child, self.reporter.take());
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(cleanup) = self.stop() {
            let _ = std::thread::Builder::new()
                .name("roc-remote-pty-cleanup".into())
                .spawn(move || {
                    let _ = cleanup.run();
                });
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn idle_reader_and_saturated_writer_cancel_while_slave_stays_open() {
        let pair = native_pty_system().openpty(PtySize::default()).unwrap();
        let (reader, writer) = unix::io_handles(pair.master.as_ref()).unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let (tx, mut output) = mpsc::channel(4);
        let reader_running = running.clone();
        let read = std::thread::spawn(move || read_output(reader, tx, reader_running));
        let (tx, input) = mpsc::channel(4);
        // A large write saturates the PTY when its slave is deliberately unread.
        tx.blocking_send(vec![b'x'; 1024 * 1024]).unwrap();
        let writer_running = running.clone();
        let master = Arc::new(Mutex::new(pair.master));
        let write = std::thread::spawn(move || write_input(writer, input, writer_running, master));
        std::thread::sleep(Duration::from_millis(20));
        running.store(false, Ordering::Release);
        output.close();
        let deadline = Instant::now() + Duration::from_millis(500);
        while (!read.is_finished() || !write.is_finished()) && Instant::now() < deadline {
            std::thread::sleep(IO_POLL);
        }
        assert!(
            read.is_finished(),
            "PTY reader waited for slave EOF instead of cancellation"
        );
        assert!(
            write.is_finished(),
            "PTY writer remained blocked behind slave input"
        );
        read.join().unwrap();
        write.join().unwrap();
        drop(pair.slave);
    }
    #[derive(Debug)]
    struct DeferredChild {
        exited: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }
    impl portable_pty::ChildKiller for DeferredChild {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(Self {
                exited: self.exited.clone(),
                dropped: self.dropped.clone(),
            })
        }
    }
    impl Child for DeferredChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            Ok(self
                .exited
                .load(Ordering::Acquire)
                .then(|| portable_pty::ExitStatus::with_exit_code(0)))
        }
        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            panic!("cleanup must never use blocking wait")
        }
        fn process_id(&self) -> Option<u32> {
            None
        }
    }
    impl Drop for DeferredChild {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }
    #[test]
    fn deferred_reap_does_not_retain_service_slot_and_eventually_reaps() {
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let exited = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let cleanup = Cleanup {
            child: Some(Box::new(DeferredChild {
                exited: exited.clone(),
                dropped: dropped.clone(),
            })),
            reporter: None,
            workers: Vec::new(),
            _slot: Arc::new(slots.clone().try_acquire_owned().unwrap()),
            lease: Some(reap::Lease::reserve().unwrap()),
            tree: None,
        };
        let start = Instant::now();
        assert!(cleanup.run().is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(
            slots.available_permits(),
            1,
            "deferred child retained service permit"
        );
        assert!(
            !dropped.load(Ordering::Acquire),
            "unreaped child was discarded"
        );
        exited.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(1);
        while !dropped.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(IO_POLL);
        }
        assert!(
            dropped.load(Ordering::Acquire),
            "deferred supervisor failed to reap child"
        );
    }
}
