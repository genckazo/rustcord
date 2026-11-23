use crate::{
    api::Api,
    gateway,
    model::{Channel, Emoji, Message},
    search::Query,
    store::{Direction, HISTORY_LIMIT},
};
use serde_json::Value;
use std::time::Instant;
use tokio::sync::mpsc;
use zeroize::Zeroizing;

pub enum Command {
    Typing(String),
    History {
        channel: String,
        cursor: Option<String>,
        direction: Direction,
    },
    Send {
        channel: String,
        content: String,
        reply_to: Option<String>,
    },
    Edit {
        channel: String,
        message_id: String,
        content: String,
    },
    Delete {
        channel: String,
        message_id: String,
    },
    React {
        channel: String,
        message_id: String,
        emoji: Emoji,
        add: bool,
    },
    Channels(String),
    OpenDm(String),
    Search {
        query: Query,
        offset: usize,
    },
}
pub enum Event {
    TypingTick,
    Status(String),
    Dispatch(String, Value),
    History(String, Direction, Result<Vec<Message>, String>),
    Sent(Result<Message, String>),
    Edited(Result<Message, String>),
    Deleted {
        channel: String,
        message_id: String,
        result: Result<(), String>,
    },
    Reacted {
        channel: String,
        message_id: String,
        emoji: Emoji,
        add: bool,
        result: Result<(), String>,
    },
    Channels(Result<Vec<Channel>, String>),
    Voice(crate::voice::Snapshot),
    OpenedDm(Result<Channel, String>),
    Search {
        query: Query,
        offset: usize,
        result: Result<(usize, Vec<Message>), String>,
    },
}
#[derive(Clone)]
pub struct Events {
    tx: mpsc::Sender<Event>,
    waker: rune::Waker,
}
impl Events {
    pub async fn send(&self, event: Event) {
        if self.tx.send(event).await.is_ok() {
            (self.waker)();
        }
    }
}
pub struct Network {
    pub tx: mpsc::Sender<Command>,
    /// Arm a one-shot TypingTick wake at the next typing expiry; `None` disarms.
    pub tick: mpsc::Sender<Option<Instant>>,
    pub voice: mpsc::Sender<crate::voice::Command>,
    pub rx: mpsc::Receiver<Event>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Network {
    #[cfg(test)]
    pub fn mock() -> (Self, mpsc::Receiver<Command>) {
        let (tx, commands) = mpsc::channel(64);
        let (_, rx) = mpsc::channel(256);
        let (tick, _) = mpsc::channel(4);
        let (voice, _) = mpsc::channel(16);
        (
            Self {
                tx,
                tick,
                voice,
                rx,
                stop: None,
            },
            commands,
        )
    }
    pub fn start(token: Zeroizing<String>, waker: rune::Waker) -> Self {
        let (tx, mut commands) = mpsc::channel(64);
        let (events_tx, rx) = mpsc::channel(256);
        let (voice, voice_commands) = mpsc::channel(16);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let (tick_tx, tick_rx) = mpsc::channel::<Option<Instant>>(4);
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move {
                let events = Events {
                    tx: events_tx,
                    waker,
                };
                let Ok(api) = Api::new(token.clone()) else {
                    events
                        .send(Event::Status("HTTPS initialization failed".into()))
                        .await;
                    return;
                };
                let (gateway_tx, gateway_rx) = mpsc::channel(16);
                let (signal_tx, signal_rx) = mpsc::channel(64);
                let voice = tokio::spawn(crate::voice::run(
                    voice_commands,
                    signal_rx,
                    gateway_tx,
                    events.clone(),
                ));
                let gateway =
                    tokio::spawn(gateway::run(token, events.clone(), gateway_rx, signal_tx));
                let ticks = typing_ticks(tick_rx, events.clone());
                let worker = async {
                    while let Some(command) = commands.recv().await {
                        let event = match command {
                            Command::OpenDm(user) => Event::OpenedDm(api.open_dm(&user).await),
                            Command::Typing(channel) => {
                                let _ = api.typing(&channel).await;
                                continue;
                            }
                            Command::History {
                                channel,
                                cursor,
                                direction,
                            } => {
                                let before = (direction == Direction::Older)
                                    .then_some(cursor.as_deref())
                                    .flatten();
                                let after = (direction == Direction::Newer)
                                    .then_some(cursor.as_deref())
                                    .flatten();
                                let result = api
                                    .messages(&channel, before, after, Some(HISTORY_LIMIT))
                                    .await;
                                Event::History(channel, direction, result)
                            }
                            Command::Send {
                                channel,
                                content,
                                reply_to,
                            } => {
                                Event::Sent(api.send(&channel, &content, reply_to.as_deref()).await)
                            }
                            Command::Edit {
                                channel,
                                message_id,
                                content,
                            } => Event::Edited(
                                api.edit_message(&channel, &message_id, &content).await,
                            ),
                            Command::Delete {
                                channel,
                                message_id,
                            } => {
                                let result = api.delete_message(&channel, &message_id).await;
                                Event::Deleted {
                                    channel,
                                    message_id,
                                    result,
                                }
                            }
                            Command::React {
                                channel,
                                message_id,
                                emoji,
                                add,
                            } => {
                                let result = api.react(&channel, &message_id, &emoji, add).await;
                                Event::Reacted {
                                    channel,
                                    message_id,
                                    emoji,
                                    add,
                                    result,
                                }
                            }
                            Command::Search { query, offset } => {
                                let result = api.search(&query, offset).await;
                                Event::Search {
                                    query,
                                    offset,
                                    result,
                                }
                            }
                            Command::Channels(guild) => {
                                Event::Channels(api.channels(&guild).await.map(|mut channels| {
                                    for c in &mut channels {
                                        c.guild_id = Some(guild.clone());
                                    }
                                    channels
                                }))
                            }
                        };
                        events.send(event).await;
                    }
                };
                tokio::select! { _ = stopped => {}, _ = worker => {}, _ = ticks => {} }
                gateway.abort();
                voice.abort();
                let _ = gateway.await;
                let _ = voice.await;
            });
        });
        Self {
            tx,
            tick: tick_tx,
            voice,
            rx,
            stop: Some(stop),
        }
    }
}
async fn typing_ticks(mut rx: mpsc::Receiver<Option<Instant>>, events: Events) {
    let mut deadline: Option<Instant> = None;
    loop {
        let update = match deadline {
            // A newer update re-arms or disarms; firing wakes the UI once to expire typings.
            Some(at) => tokio::select! {
                _ = tokio::time::sleep_until(at.into()) => {
                    events.send(Event::TypingTick).await;
                    rx.recv().await
                }
                next = rx.recv() => next,
            },
            None => rx.recv().await,
        };
        let Some(mut update) = update else { break };
        // Collapse any queued re-arms into the newest state.
        while let Ok(next) = rx.try_recv() {
            update = next;
        }
        deadline = update;
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Duration};

    #[tokio::test]
    async fn typing_ticks_fire_once_per_deadline() {
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = {
            let wakes = wakes.clone();
            Arc::new(move || {
                wakes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        };
        let (tx, mut rx) = mpsc::channel(8);
        let (tick_tx, tick_rx) = mpsc::channel(4);
        let task = tokio::spawn(typing_ticks(tick_rx, Events { tx, waker: counter }));
        // Idle: an armed-free driver never wakes the UI.
        assert!(tokio::time::timeout(Duration::from_millis(300), rx.recv())
            .await
            .is_err());
        tick_tx
            .send(Some(Instant::now() + Duration::from_millis(50)))
            .await
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), rx.recv()).await,
            Ok(Some(Event::TypingTick))
        ));
        assert_eq!(wakes.load(std::sync::atomic::Ordering::SeqCst), 1);
        // One wake per deadline: nothing follows the fired tick.
        assert!(tokio::time::timeout(Duration::from_millis(300), rx.recv())
            .await
            .is_err());
        // A `None` re-arm disarms a pending deadline before it fires.
        tick_tx
            .send(Some(Instant::now() + Duration::from_millis(50)))
            .await
            .unwrap();
        tick_tx.send(None).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(300), rx.recv())
            .await
            .is_err());
        task.abort();
    }
}

#[cfg(test)]
mod voice_tests {
    use super::*;
    use crate::voice::{self, handshake::Target, Command as Voice, Signal};
    use serde_json::json;
    use std::{sync::Arc, time::Duration};

    #[tokio::test]
    async fn moderator_moves_never_send_gateway_leave() {
        tokio::time::timeout(Duration::from_secs(3), async {
            for server_first in [false, true] {
                let (command_tx, command_rx) = mpsc::channel(16);
                let (signal_tx, signal_rx) = mpsc::channel(16);
                let (gateway_tx, mut gateway_rx) = mpsc::channel(16);
                let (tx, mut rx) = mpsc::channel(32);
                let task = tokio::spawn(voice::run(
                    command_rx,
                    signal_rx,
                    gateway_tx,
                    Events {
                        tx,
                        waker: Arc::new(|| {}),
                    },
                ));
                signal_tx
                    .send(Signal::Dispatch("READY".into(), json!({"user":{"id":"1"}})))
                    .await
                    .unwrap();
                rx.recv().await.unwrap();
                command_tx
                    .send(Voice::Join(Target {
                        guild: Some("10".into()),
                        channel: "20".into(),
                    }))
                    .await
                    .unwrap();
                assert_eq!(gateway_rx.recv().await.unwrap()["d"]["channel_id"], "20");
                rx.recv().await.unwrap();
                // A null endpoint keeps this manager test device- and network-free.
                // Fresh connected-handshake pairing is covered in handshake's tests.
                let server = (
                    "VOICE_SERVER_UPDATE",
                    json!({"guild_id":"10","endpoint":null}),
                );
                let moved = (
                    "VOICE_STATE_UPDATE",
                    json!({"user_id":"1","guild_id":"10","channel_id":"30","session_id":"moved"}),
                );
                let signals = if server_first {
                    [server, moved]
                } else {
                    [moved, server]
                };
                for (name, data) in signals {
                    signal_tx
                        .send(Signal::Dispatch(name.into(), data))
                        .await
                        .unwrap();
                    rx.recv().await.unwrap();
                    assert!(gateway_rx.try_recv().is_err());
                }
                command_tx.send(Voice::Mute(true)).await.unwrap();
                let payload = gateway_rx.recv().await.unwrap();
                assert_eq!(payload["d"]["channel_id"], "30");
                assert_eq!(payload["d"]["guild_id"], "10");
                task.abort();
            }
        })
        .await
        .expect("voice manager stalled");
    }

    #[tokio::test]
    async fn voice_commands_reset_resume_and_bad_endpoint() {
        tokio::time::timeout(Duration::from_secs(3), async {
            let (command_tx, command_rx) = mpsc::channel(16);
            let (signal_tx, signal_rx) = mpsc::channel(16);
            let (gateway_tx, mut gateway_rx) = mpsc::channel(16);
            let (tx, mut rx) = mpsc::channel(32);
            let task = tokio::spawn(voice::run(command_rx, signal_rx, gateway_tx, Events { tx, waker: Arc::new(|| {}) }));
            let target = Target { guild: Some("10".into()), channel: "20".into() };
            command_tx.send(Voice::Join(target.clone())).await.unwrap();
            assert!(matches!(rx.recv().await, Some(Event::Voice(s)) if s.target.is_none() && s.status.contains("READY")));
            assert!(gateway_rx.try_recv().is_err());
            signal_tx.send(Signal::Dispatch("READY".into(), json!({"user":{"id":"1"}}))).await.unwrap();
            rx.recv().await.unwrap();
            command_tx.send(Voice::Join(target)).await.unwrap();
            assert_eq!(gateway_rx.recv().await.unwrap()["d"]["guild_id"], "10");
            rx.recv().await.unwrap();
            command_tx.send(Voice::Mute(true)).await.unwrap();
            assert_eq!(gateway_rx.recv().await.unwrap()["d"]["self_mute"], true);
            rx.recv().await.unwrap();
            command_tx.send(Voice::Deafen(true)).await.unwrap();
            assert_eq!(gateway_rx.recv().await.unwrap()["d"]["self_deaf"], true);
            rx.recv().await.unwrap();
            command_tx.send(Voice::Leave).await.unwrap();
            assert_eq!(gateway_rx.recv().await.unwrap(), json!({"op":4,"d":{"guild_id":"10","channel_id":null,"self_mute":true,"self_deaf":true}}));
            rx.recv().await.unwrap();
            signal_tx.send(Signal::Reset).await.unwrap();
            rx.recv().await.unwrap();
            command_tx.send(Voice::Join(Target { guild: None, channel: "30".into() })).await.unwrap();
            rx.recv().await.unwrap();
            assert!(gateway_rx.try_recv().is_err());
            signal_tx.send(Signal::Dispatch("RESUMED".into(), json!({}))).await.unwrap();
            rx.recv().await.unwrap();
            command_tx.send(Voice::Join(Target { guild: None, channel: "30".into() })).await.unwrap();
            let dm = gateway_rx.recv().await.unwrap();
            assert!(dm["d"]["guild_id"].is_null());
            assert_eq!(dm["d"]["channel_id"], "30");
            rx.recv().await.unwrap();
            signal_tx.send(Signal::Dispatch("VOICE_SERVER_UPDATE".into(), json!({"channel_id":"30","endpoint":"untrusted.example","token":"test-only"}))).await.unwrap();
            assert!(gateway_rx.recv().await.unwrap()["d"]["channel_id"].is_null());
            assert!(matches!(rx.recv().await, Some(Event::Voice(s)) if s.target.is_none() && s.status.contains("Untrusted")));
            task.abort();
        }).await.expect("voice manager stalled");
    }
}
