//! One ordered input stream; blur has a separate, non-droppable cancellation signal.
use anyhow::{Context, Result, ensure};
use roc_remote_core::{
    link::{LinkHandle, MuxStream},
    records,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, watch},
};

#[derive(Clone)]
pub struct Bridge {
    sender: Arc<Mutex<Option<mpsc::Sender<Event>>>>,
    epoch: watch::Sender<u64>,
    armed: Arc<AtomicBool>,
}
pub(crate) struct Event {
    epoch: u64,
    display: u32,
    records: Vec<Value>,
}

impl Bridge {
    pub fn new() -> Self {
        Self {
            sender: Arc::default(),
            epoch: watch::channel(0).0,
            armed: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn attach(&self) -> mpsc::Receiver<Event> {
        self.release();
        let (tx, rx) = mpsc::channel(128);
        *self.sender.lock().expect("input bridge lock") = Some(tx);
        rx
    }
    pub fn detach(&self) {
        self.sender.lock().expect("input bridge lock").take();
        self.release();
    }
    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Acquire)
    }

    pub fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }

    pub fn release(&self) {
        self.armed.store(false, Ordering::Release);
        self.epoch
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }
    pub fn send(&self, display: u32, records: Vec<Value>) -> bool {
        if !self.armed.load(Ordering::Acquire) {
            return false;
        }
        let epoch = *self.epoch.borrow();
        let ok = self
            .sender
            .lock()
            .expect("input bridge lock")
            .as_ref()
            .is_some_and(|tx| {
                tx.try_send(Event {
                    epoch,
                    display,
                    records,
                })
                .is_ok()
            });
        if !ok {
            self.release();
        }
        ok
    }
    pub async fn run(
        &self,
        handle: LinkHandle,
        mut events: mpsc::Receiver<Event>,
        report: impl Fn(String),
    ) -> Result<()> {
        let mut epoch = self.epoch.subscribe();
        let mut next = None;
        loop {
            let event = match next.take() {
                Some(event) => event,
                None => match events.recv().await {
                    Some(event) => event,
                    None => return Ok(()),
                },
            };
            if event.epoch != *epoch.borrow_and_update() {
                continue;
            }
            let stream = handle.open("input", json!({"display":event.display}));
            let mut stream = tokio::select! {
                stream = stream => match stream { Ok(stream) => stream, Err(error) => { self.release(); report(format!("Open remote input: {error:#}")); continue; } },
                _ = epoch.changed() => continue,
            };
            match session(&mut stream, event, &mut events, &mut epoch).await {
                Ok(event) => next = event,
                Err(error) => {
                    self.release();
                    report(format!(
                        "Remote input: {error:#}. Click the desktop to retry."
                    ));
                }
            }
        }
    }
}

#[derive(Default)]
struct Held {
    keys: BTreeSet<String>,
    buttons: BTreeSet<String>,
}
impl Held {
    fn expand(&self, value: Value) -> Vec<Value> {
        if value["type"] != "modifiers" {
            return vec![value];
        }
        let keys = value["keys"].as_array();
        ["Shift", "Control", "Alt", "Meta"]
            .into_iter()
            .filter_map(|key| {
                let down = keys.is_some_and(|keys| keys.iter().any(|value| value == key));
                (self.keys.contains(key) != down)
                    .then(|| json!({"type":"key","key":key,"down":down}))
            })
            .collect()
    }
    fn observe(&mut self, value: &Value) {
        let set = match value["type"].as_str() {
            Some("key") => &mut self.keys,
            Some("button") => &mut self.buttons,
            _ => return,
        };
        let name = value
            .get("key")
            .or_else(|| value.get("button"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if value["down"] == true {
            set.insert(name.into());
        } else {
            set.remove(name);
        }
    }
    fn releases(&self) -> Vec<Value> {
        self.buttons
            .iter()
            .map(|button| json!({"type":"button","button":button,"down":false}))
            .chain(
                self.keys
                    .iter()
                    .map(|key| json!({"type":"key","key":key,"down":false})),
            )
            .collect()
    }
}

async fn session(
    stream: &mut MuxStream,
    first: Event,
    events: &mut mpsc::Receiver<Event>,
    epoch: &mut watch::Receiver<u64>,
) -> Result<Option<Event>> {
    let mut held = Held::default();
    let active = first.epoch;
    let display = first.display;
    let mut next = Some(first);
    let result: Result<Option<Event>> = async {
        let (mut reader, mut writer) = tokio::io::split(&mut *stream);
        let mut byte = [0;1];
        loop {
            if let Some(event) = next.take() {
                if event.epoch != *epoch.borrow() { break; }
                for value in event.records {
                    for value in held.expand(value) {
                        // Remember before writing so a partial write still triggers cleanup.
                        held.observe(&value);
                        tokio::time::timeout(Duration::from_secs(2), records::json(&mut writer, &value)).await??;
                    }
                }
            }
            tokio::select! {
                changed = epoch.changed() => { let _ = changed; break; }
                read = reader.read(&mut byte) => {
                    ensure!(read? == 0, "Unexpected remote input response");
                    anyhow::bail!("Remote input stream ended; focus the desktop to retry");
                }
                event = events.recv() => match event {
                    None => break,
                    Some(event) if event.epoch != active || event.display != display => { next = Some(event); break; }
                    Some(event) => next = Some(event),
                }
            }
        }
        Ok(next.take())
    }.await;
    // Release is not queued behind ordinary input, and cannot wait forever for a
    // failed peer. Host-side InputState is also released when this stream drops.
    let cleanup = tokio::time::timeout(Duration::from_secs(2), async {
        for release in held.releases() {
            records::json(&mut *stream, &release).await?;
        }
        stream.shutdown().await?;
        let mut byte = [0; 1];
        ensure!(
            stream.read(&mut byte).await? == 0,
            "Unexpected remote input receipt"
        );
        stream
            .completed()
            .await
            .context("Remote input release did not complete")
    })
    .await
    .context("Remote input release timed out");
    let next = result?;
    cleanup??;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blur_epoch_never_replays_old_events_and_queue_overflow_releases() {
        let bridge = Bridge::new();
        let mut rx = bridge.attach();
        bridge.arm();
        assert!(bridge.send(3, vec![json!({"type":"key","key":"a","down":true})]));
        bridge.release();
        assert_ne!(rx.try_recv().unwrap().epoch, *bridge.epoch.borrow());
        bridge.arm();
        for _ in 0..128 {
            assert!(bridge.send(3, Vec::new()));
        }
        let before = *bridge.epoch.borrow();
        assert!(!bridge.send(3, Vec::new()));
        assert_ne!(*bridge.epoch.borrow(), before);
        bridge.detach();
        assert!(!bridge.send(3, Vec::new()));
    }
    #[tokio::test]
    async fn encrypted_input_blur_releases_keys_and_buttons_before_fin() -> Result<()> {
        use roc_remote_core::{
            crypto::Role,
            invite::{HostTicket, Room},
            link,
        };
        tokio::time::timeout(Duration::from_secs(15), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let relay = roc_remote_relay::router(
                roc_remote_relay::Config::new("native-test-operator".into()).unwrap(),
            )
            .unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, relay).await });
            let base = format!("http://{address}");
            let room: Room = reqwest::Client::new()
                .post(format!("{base}/v1/rooms"))
                .bearer_auth("native-test-operator")
                .json(&json!({}))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let ticket = HostTicket::new(&base, room)?;
            let (host, client) = tokio::join!(
                link::connect(&ticket.invite, Role::Host, &ticket.host_token),
                link::connect(&ticket.invite, Role::Client, &ticket.invite.client_token)
            );
            let mut host = host?;
            let client = client?;
            let (pressed_tx, pressed_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            let (done_tx, done_rx) = tokio::sync::oneshot::channel();
            let peer = tokio::spawn(async move {
                let mut incoming = host.incoming.recv().await.context("Input stream missing")?;
                ensure!(
                    incoming.route == "input" && incoming.metadata == json!({"display":3}),
                    "Selected display not sent"
                );
                let mut events = Vec::new();
                let mut pressed_tx = Some(pressed_tx);
                while let Some(record) =
                    records::read(&mut incoming.stream, records::MAX_CONTROL).await?
                {
                    events.push(serde_json::from_slice::<Value>(&record)?);
                    if events.len() == 2 {
                        let _ = pressed_tx.take().unwrap().send(());
                    }
                }
                incoming.stream.shutdown().await?;
                incoming.completion.finish(true);
                let _ = release_tx.send(events);
                let _ = done_rx.await;
                Ok::<_, anyhow::Error>(())
            });
            let bridge = Bridge::new();
            let receiver = bridge.attach();
            bridge.arm();
            let network = bridge.clone();
            let worker = tokio::spawn(async move {
                let result = network
                    .run(client.handle.clone(), receiver, |message| {
                        panic!("{message}")
                    })
                    .await;
                drop(client);
                result
            });
            assert!(bridge.send(
                3,
                vec![
                    json!({"type":"key","key":"Control","down":true}),
                    json!({"type":"button","button":"right","down":true})
                ]
            ));
            pressed_rx.await?;
            bridge.release();
            let events = release_rx.await?;
            assert_eq!(events.len(), 4);
            assert_eq!(
                events[2],
                json!({"type":"button","button":"right","down":false})
            );
            assert_eq!(
                events[3],
                json!({"type":"key","key":"Control","down":false})
            );
            bridge.detach();
            worker.await??;
            let _ = done_tx.send(());
            peer.await??;
            server.abort();
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }

    #[test]
    fn focus_with_already_held_modifiers_synchronizes_without_duplicate_presses() {
        let mut held = Held::default();
        let sync = json!({"type":"modifiers","keys":["Control","Shift"]});
        let events = held.expand(sync.clone());
        assert_eq!(events.len(), 2);
        for event in events {
            held.observe(&event);
        }
        assert!(held.expand(sync).is_empty());
        let release = held.expand(json!({"type":"modifiers","keys":[]}));
        assert_eq!(release.len(), 2);
        assert!(release.iter().all(|v| v["down"] == false));
    }

    #[test]
    fn releases_only_keys_and_buttons_still_held() {
        let mut held = Held::default();
        held.observe(&json!({"type":"key","key":"Control","down":true}));
        held.observe(&json!({"type":"key","key":"a","down":true}));
        held.observe(&json!({"type":"key","key":"a","down":false}));
        held.observe(&json!({"type":"button","button":"right","down":true}));
        let release = held.releases();
        assert_eq!(release.len(), 2);
        assert!(release.iter().all(|v| v["down"] == false));
        assert!(release.iter().any(|v| v["key"] == "Control"));
    }
}
