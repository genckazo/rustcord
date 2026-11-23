use crate::{
    api::Api,
    gateway,
    model::{Channel, Message},
    store::{Direction, HISTORY_LIMIT},
};
use serde_json::Value;
use tokio::sync::mpsc;
use zeroize::Zeroizing;

pub enum Command {
    History {
        channel: String,
        cursor: Option<String>,
        direction: Direction,
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
    History(String, Direction, Result<Vec<Message>, String>),
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
                let worker = async {
                    while let Some(command) = commands.recv().await {
                        let event = match command {
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
