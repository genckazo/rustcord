use crate::network::{Event, Events};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use zeroize::Zeroizing;

#[derive(Debug, Deserialize, Serialize)]
pub struct Payload {
    pub op: u8,
    pub d: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub s: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t: Option<String>,
}
#[derive(Default)]
struct Session {
    id: Option<String>,
    url: Option<String>,
    seq: Option<u64>,
}
pub fn gateway_url(url: &str) -> Option<String> {
    // Discord's regional resume hosts are deliberately not allowlisted.
    let base = url.trim_end_matches('/');
    (base == "wss://gateway.discord.gg").then(|| format!("{base}/?v=10&encoding=json"))
}
pub async fn run(token: Zeroizing<String>, events: Events) {
    let mut session = Session::default();
    let mut backoff = 1;
    loop {
        events.send(Event::Status("Connecting…".into())).await;
        match connection(&token, &events, &mut session).await {
            Ok(()) => {
                backoff = 1;
            }
            Err(Failure::Fatal) => {
                events
                    .send(Event::Status(
                        "Authentication rejected. Log out and check your token.".into(),
                    ))
                    .await;
                return;
            }
            Err(Failure::Retry) => {}
        }
        events
            .send(Event::Status(format!("Reconnecting in {backoff}s…")))
            .await;
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(60);
    }
}
enum Failure {
    Retry,
    Fatal,
}
async fn connection(token: &str, events: &Events, session: &mut Session) -> Result<(), Failure> {
    let url = session
        .url
        .as_deref()
        .and_then(gateway_url)
        .unwrap_or_else(|| gateway_url("wss://gateway.discord.gg").unwrap());
    let (mut socket, _) = timeout(Duration::from_secs(30), connect_async(url))
        .await
        .map_err(|_| Failure::Retry)?
        .map_err(|_| Failure::Retry)?;
    let hello = timeout(Duration::from_secs(30), socket.next())
        .await
        .map_err(|_| Failure::Retry)?
        .ok_or(Failure::Retry)?
        .map_err(|_| Failure::Retry)?;
    let hello: Payload = serde_json::from_str(hello.to_text().map_err(|_| Failure::Retry)?)
        .map_err(|_| Failure::Retry)?;
    if hello.op != 10 {
        return Err(Failure::Retry);
    }
    let interval = hello.d["heartbeat_interval"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or(Failure::Retry)?;
    let interval = Duration::from_millis(interval);
    let mut next = Instant::now() + interval.mul_f64(rand::random::<f64>());
    let mut awaiting_ack = false;
    let auth = if let (Some(id), Some(seq)) = (&session.id, session.seq) {
        json!({"op":6,"d":{"token":token,"session_id":id,"seq":seq}})
    } else {
        json!({"op":2,"d":{"token":token,"properties":{"os":"Mac OS X","browser":"Discord Client","device":"Discord Client"},"compress":false}})
    };
    socket
        .send(Message::Text(auth.to_string().into()))
        .await
        .map_err(|_| Failure::Retry)?;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(next) => {
                if awaiting_ack { return Err(Failure::Retry); }
                socket.send(Message::Text(json!({"op":1,"d":session.seq}).to_string().into())).await.map_err(|_| Failure::Retry)?;
                awaiting_ack = true;
                next = Instant::now() + interval;
            }
            frame = socket.next() => {
                let frame = frame.ok_or(Failure::Retry)?.map_err(|_| Failure::Retry)?;
                match frame {
                    Message::Close(close) => {
                        let code = close.map(|c| u16::from(c.code)).unwrap_or(1000);
                        if matches!(code, 4004 | 4010..=4014) { return Err(Failure::Fatal); }
                        if matches!(code, 4007 | 4009) { *session = Session::default(); }
                        return Err(Failure::Retry);
                    }
                    Message::Ping(data) => { socket.send(Message::Pong(data)).await.map_err(|_| Failure::Retry)?; }
                    Message::Text(text) => {
                        let payload: Payload = serde_json::from_str(&text).map_err(|_| Failure::Retry)?;
                        if let Some(seq) = payload.s { session.seq = Some(seq); }
                        match payload.op {
                            0 => {
                                let name = payload.t.unwrap_or_default();
                                if name == "READY" {
                                    session.id = payload.d["session_id"].as_str().map(str::to_owned);
                                    session.url = payload.d["resume_gateway_url"].as_str().map(str::to_owned);
                                    events.send(Event::Status("Connected".into())).await;
                                } else if name == "RESUMED" { events.send(Event::Status("Connected (resumed)".into())).await; }
                                events.send(Event::Dispatch(name, payload.d)).await;
                            }
                            1 => {
                                socket.send(Message::Text(json!({"op":1,"d":session.seq}).to_string().into())).await.map_err(|_| Failure::Retry)?;
                                awaiting_ack = true;
                            }
                            7 => return Ok(()),
                            9 => {
                                if payload.d.as_bool() != Some(true) { *session = Session::default(); }
                                tokio::time::sleep(Duration::from_secs(1 + rand::random::<u64>() % 5)).await;
                                return Ok(());
                            }
                            11 => awaiting_ack = false,
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hello_ready_and_roundtrip() {
        let hello: Payload =
            serde_json::from_str(r#"{"op":10,"d":{"heartbeat_interval":41250}}"#).unwrap();
        assert_eq!(hello.op, 10);
        assert_eq!(hello.d["heartbeat_interval"], 41250);
        let ready: Payload = serde_json::from_str(r#"{"op":0,"s":42,"t":"READY","d":{"session_id":"test","user":{"id":"1","username":"Ada"},"guilds":[],"private_channels":[]}}"#).unwrap();
        let encoded = serde_json::to_string(&ready).unwrap();
        let decoded: Payload = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.s, Some(42));
        assert_eq!(decoded.t.as_deref(), Some("READY"));
        assert_eq!(decoded.d["user"]["username"], "Ada");
    }
    #[test]
    fn heartbeat_and_invalid_payloads() {
        let heartbeat = Payload {
            op: 1,
            d: Value::Null,
            s: None,
            t: None,
        };
        assert_eq!(
            serde_json::to_value(heartbeat).unwrap(),
            json!({"op":1,"d":null})
        );
        for input in ["", "{}", r#"{"op":"bad","d":null}"#] {
            assert!(serde_json::from_str::<Payload>(input).is_err());
        }
        assert!(gateway_url("wss://gateway.discord.gg.evil.test").is_none());
        assert!(gateway_url("ws://gateway.discord.gg").is_none());
        assert!(gateway_url("wss://gateway.discord.gg/").is_some());
    }
}
