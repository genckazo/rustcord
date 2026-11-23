mod audio;
pub mod handshake;

use crate::network::{Event, Events};
use async_trait::async_trait;
use handshake::{leave_payload, state_payload, Handshake, Target};
use serde_json::Value;
use songbird::{
    driver::{DecodeConfig, DecodeMode},
    Config, CoreEvent, Driver, Event as SongEvent, EventContext, EventHandler,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

pub enum Command {
    Join(Target),
    Leave,
    Mute(bool),
    Deafen(bool),
}
pub enum Signal {
    Dispatch(String, Value),
    Reset,
}
#[derive(Clone, Default)]
pub struct Snapshot {
    pub target: Option<Target>,
    pub status: String,
    pub mute: bool,
    pub deaf: bool,
    pub speaking: HashSet<String>,
}
enum Internal {
    Connected(u64, bool),
    Speaking(u64, HashSet<String>),
    Dropped(u64),
}
#[derive(Clone)]
struct Receiver {
    bus: audio::Bus,
    ids: Arc<Mutex<HashMap<u32, String>>>,
    speaking: Arc<Mutex<HashSet<String>>>,
    tx: mpsc::Sender<Internal>,
    generation: u64,
}
#[async_trait]
impl EventHandler for Receiver {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<SongEvent> {
        match ctx {
            EventContext::SpeakingStateUpdate(s) => {
                if let Some(user) = s.user_id {
                    if let Ok(mut ids) = self.ids.lock() {
                        ids.insert(s.ssrc, user.to_string());
                    }
                }
            }
            EventContext::VoiceTick(tick) => {
                self.bus.mix(
                    tick.speaking
                        .values()
                        .filter_map(|d| d.decoded_voice.clone()),
                );
                if let (Ok(ids), Ok(mut prior)) = (self.ids.lock(), self.speaking.lock()) {
                    let active: HashSet<_> = tick
                        .speaking
                        .iter()
                        .filter(|(_, d)| {
                            d.decoded_voice
                                .as_ref()
                                .is_some_and(|v| v.iter().any(|s| s.unsigned_abs() > 200))
                        })
                        .filter_map(|(ssrc, _)| ids.get(ssrc).cloned())
                        .collect();
                    if *prior != active
                        && self
                            .tx
                            .try_send(Internal::Speaking(self.generation, active.clone()))
                            .is_ok()
                    {
                        *prior = active;
                    }
                }
            }
            EventContext::ClientDisconnect(d) => {
                if let Ok(mut ids) = self.ids.lock() {
                    ids.retain(|_, id| id != &d.user_id.to_string());
                }
            }
            EventContext::DriverDisconnect(_) => {
                let _ = self.tx.try_send(Internal::Dropped(self.generation));
            }
            _ => {}
        }
        None
    }
}
pub async fn run(
    mut commands: mpsc::Receiver<Command>,
    mut signals: mpsc::Receiver<Signal>,
    gateway: mpsc::Sender<Value>,
    events: Events,
) {
    let (internal_tx, mut internal_rx) = mpsc::channel(32);
    let mut state = Snapshot::default();
    let mut user: Option<String> = None;
    let mut available = false;
    let mut handshake: Option<Handshake> = None;
    let mut driver: Option<Driver> = None;
    let mut audio = None;
    let mut bus = audio::Bus::new();
    let mut generation = 0_u64;
    let mut last_info = None;
    let mut deadline = tokio::time::Instant::now();
    let mut health = tokio::time::interval(Duration::from_millis(250));
    loop {
        let previous = state.target.clone();
        let mut disconnect = false;
        let mut notify_gateway = false;
        tokio::select! {
            Some(command) = commands.recv() => match command {
                Command::Join(target) => {
                    if state.target.as_ref() == Some(&target) { continue; }
                    if let Some(user) = user.as_ref().filter(|_| available) {
                        if let Some(d) = driver.take() { drop(d); } audio = None; bus.clear();
                        generation += 1; last_info = None; bus = audio::Bus::new();
                        handshake = Some(Handshake::new(target.clone(), user.clone())); state.target = Some(target); state.status = "Joining voice…".into(); state.speaking.clear();
                        deadline = tokio::time::Instant::now() + Duration::from_secs(30); notify_gateway = true;
                    } else { state.status = "Wait for Discord READY before joining voice".into(); }
                }
                Command::Leave => { disconnect = true; notify_gateway = true; }
                Command::Mute(mute) => { state.mute = mute; notify_gateway = state.target.is_some(); }
                Command::Deafen(deaf) => { state.deaf = deaf; notify_gateway = state.target.is_some(); }
            },
            Some(signal) = signals.recv() => match signal {
                Signal::Reset => { disconnect = true; available = false; }
                Signal::Dispatch(name, data) => {
                    if name == "READY" { user = data["user"]["id"].as_str().map(str::to_owned); available = true; }
                    if name == "RESUMED" { available = true; }
                    if name == "VOICE_STATE_UPDATE" && data["user_id"].as_str() == user.as_deref() && state.target.as_ref().is_some_and(|t| data["guild_id"].as_str() == t.guild.as_deref()) && data["channel_id"].is_null() { disconnect = true; }
                    if let Some(h) = &mut handshake {
                        let result = h.apply(&name, &data);
                        if state.target.as_ref() != Some(&h.target) || (last_info.is_some() && h.is_pending()) {
                            driver = None; audio = None; bus.clear(); bus = audio::Bus::new();
                            generation += 1; last_info = None;
                            state.target = Some(h.target.clone()); state.speaking.clear();
                            state.status = "Joining voice…".into();
                            deadline = tokio::time::Instant::now() + Duration::from_secs(30);
                        }
                        match result {
                            Ok(Some(info)) if last_info.as_ref() != Some(&info) => {
                                generation += 1; audio = None; bus.clear();
                                deadline = tokio::time::Instant::now() + Duration::from_secs(30);
                                last_info = Some(info.clone());
                                let mut d = Driver::new(Config::default().decode_mode(DecodeMode::Decode(DecodeConfig::default())));
                                let handler = Receiver { bus: bus.clone(), ids: Arc::new(Mutex::new(HashMap::new())), speaking: Arc::new(Mutex::new(HashSet::new())), tx: internal_tx.clone(), generation };
                                for event in [CoreEvent::VoiceTick, CoreEvent::SpeakingStateUpdate, CoreEvent::ClientDisconnect, CoreEvent::DriverDisconnect] { d.add_global_event(SongEvent::Core(event), handler.clone()); }
                                let connected = d.connect(info); driver = Some(d);
                                let tx = internal_tx.clone(); let attempt = generation;
                                tokio::spawn(async move { let ok = tokio::time::timeout(Duration::from_secs(25), connected).await.is_ok_and(|r| r.is_ok()); let _ = tx.send(Internal::Connected(attempt, ok)).await; });
                            }
                            Err(error) => { state.status = error.into(); disconnect = true; notify_gateway = true; }
                            _ => {}
                        }
                    }
                }
            },
            Some(internal) = internal_rx.recv() => match internal {
                Internal::Connected(attempt, ok) if attempt == generation && state.target.is_some() => {
                    if ok {
                        match audio::Audio::start(bus.clone()).await {
                            Ok(streams) => { audio = Some(streams); if let Some(d) = &mut driver { d.play_only_input(bus.input()); } state.status = "Voice connected".into(); }
                            Err(error) => { state.status = error; disconnect = true; notify_gateway = true; }
                        }
                    } else { state.status = "Voice connection failed".into(); disconnect = true; notify_gateway = true; }
                }
                Internal::Speaking(attempt, speaking) if attempt == generation && state.target.is_some() => state.speaking = speaking,
                Internal::Dropped(attempt) if attempt == generation && state.target.is_some() => { state.status = "Voice transport disconnected".into(); disconnect = true; notify_gateway = true; }
                _ => {}
            },
            _ = health.tick() => {
                if state.target.is_some() && audio.is_none() && tokio::time::Instant::now() >= deadline { state.status = "Voice handshake timed out".into(); disconnect = true; notify_gateway = true; }
                else if bus.failed.swap(false, Ordering::Relaxed) { state.status = "Audio device failed; reconnect voice after checking devices".into(); disconnect = true; notify_gateway = true; }
                else { continue; }
            }
            else => break,
        }
        if disconnect {
            generation += 1;
            if let Some(mut d) = driver.take() {
                d.leave();
            }
            audio = None;
            bus.clear();
            handshake = None;
            last_info = None;
            state.target = None;
            state.speaking.clear();
            if state.status.starts_with("Voice connected") || state.status.starts_with("Joining") {
                state.status = "Voice disconnected".into();
            }
        }
        bus.controls(state.mute, state.deaf);
        if let Some(d) = &mut driver {
            d.mute(state.mute || state.deaf);
        }
        if notify_gateway {
            if state.target.is_none() {
                if let Some(previous) = previous {
                    if gateway
                        .send(leave_payload(&previous, state.mute, state.deaf))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            if state.target.is_some()
                && gateway
                    .send(state_payload(state.target.as_ref(), state.mute, state.deaf))
                    .await
                    .is_err()
            {
                break;
            }
        }
        events.send(Event::Voice(state.clone())).await;
    }
    drop(audio);
    if let Some(mut d) = driver {
        d.leave();
    }
    bus.clear();
}
