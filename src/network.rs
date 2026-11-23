use crate::{
    api::Api,
    gateway,
    model::{Channel, Message},
};
use serde_json::Value;
use tokio::sync::mpsc;
use zeroize::Zeroizing;

pub enum Command {
    History {
        channel: String,
        before: Option<String>,
    },
    Send {
        channel: String,
        content: String,
    },
    Channels(String),
}
pub enum Event {
    Status(String),
    Dispatch(String, Value),
    History(String, Result<Vec<Message>, String>),
    Sent(Result<Message, String>),
    Channels(Result<Vec<Channel>, String>),
    Voice(crate::voice::Snapshot),
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
    pub voice: mpsc::Sender<crate::voice::Command>,
    pub rx: mpsc::Receiver<Event>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Network {
    pub fn start(token: Zeroizing<String>, waker: rune::Waker) -> Self {
        let (tx, mut commands) = mpsc::channel(64);
        let (events_tx, rx) = mpsc::channel(256);
        let (voice, voice_commands) = mpsc::channel(16);
        let (stop, stopped) = tokio::sync::oneshot::channel();
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
                let worker = async {
                    while let Some(command) = commands.recv().await {
                        let event = match command {
                            Command::History { channel, before } => {
                                let result = api.messages(&channel, before.as_deref()).await;
                                Event::History(channel, result)
                            }
                            Command::Send { channel, content } => {
                                Event::Sent(api.send(&channel, &content).await)
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
                tokio::select! { _ = stopped => {}, _ = worker => {} }
                gateway.abort();
                voice.abort();
                let _ = gateway.await;
                let _ = voice.await;
            });
        });
        Self {
            tx,
            voice,
            rx,
            stop: Some(stop),
        }
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
mod voice_tests {
    use super::*;
    use crate::voice::{self, handshake::Target, Command as Voice, Signal};
    use serde_json::json;
    use std::{sync::Arc, time::Duration};

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
