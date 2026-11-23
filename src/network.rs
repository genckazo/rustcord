use crate::{
    api::Api,
    gateway,
    model::{Channel, Message},
};
use serde_json::Value;
use tokio::sync::mpsc;
use zeroize::Zeroizing;

pub enum Command {
    Typing(String),
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
    TypingTick,
    Status(String),
    Dispatch(String, Value),
    History(String, Result<Vec<Message>, String>),
    Sent(Result<Message, String>),
    Channels(Result<Vec<Channel>, String>),
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
    pub rx: mpsc::Receiver<Event>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Network {
    pub fn start(token: Zeroizing<String>, waker: rune::Waker) -> Self {
        let (tx, mut commands) = mpsc::channel(64);
        let (events_tx, rx) = mpsc::channel(256);
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
                let gateway = tokio::spawn(gateway::run(token, events.clone()));
                let ticks = async {
                    let mut timer = tokio::time::interval(std::time::Duration::from_millis(500));
                    loop {
                        timer.tick().await;
                        events.send(Event::TypingTick).await;
                    }
                };
                let worker = async {
                    while let Some(command) = commands.recv().await {
                        let event = match command {
                            Command::Typing(channel) => {
                                let _ = api.typing(&channel).await;
                                continue;
                            }
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
                tokio::select! { _ = stopped => {}, _ = worker => {}, _ = ticks => {} }
                gateway.abort();
                let _ = gateway.await;
            });
        });
        Self {
            tx,
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
