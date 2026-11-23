use aws_lc_rs::{
    digest,
    encoding::AsDer,
    rsa::{KeySize, OaepPrivateDecryptingKey, PrivateDecryptingKey, OAEP_SHA256_MGF1SHA256},
};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use qrcode::{Color, QrCode};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use zeroize::Zeroizing;

const REMOTE_AUTH_GATEWAY: &str = "wss://remote-auth-gateway.discord.gg/?v=2";
const REMOTE_AUTH_ORIGIN: &str = "https://discord.com";
const REMOTE_AUTH_LOGIN_URL: &str = "https://discord.com/api/v9/users/@me/remote-auth/login";
const QR_URL_PREFIX: &str = "https://discord.com/ra/";

pub enum QrEvent {
    Status(String),
    Ready {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Scanned(String),
    Token(Zeroizing<String>),
    Error(String),
}

pub struct QrSession {
    pub rx: mpsc::Receiver<QrEvent>,
    _stop: tokio::sync::oneshot::Sender<()>,
}

impl QrSession {
    pub fn start(waker: rune::Waker) -> Self {
        let (tx, rx) = mpsc::channel(16);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move {
                let emit = |event: QrEvent| {
                    let tx = tx.clone();
                    let waker = waker.clone();
                    async move {
                        if tx.send(event).await.is_ok() {
                            waker();
                        }
                    }
                };
                tokio::select! {
                    _ = stopped => {}
                    result = run_remote_auth(&tx, &waker) => {
                        if let Err(err) = result {
                            emit(QrEvent::Error(err)).await;
                        }
                    }
                }
            });
        });
        Self { rx, _stop: stop }
    }
}

async fn send_event(tx: &mpsc::Sender<QrEvent>, waker: &rune::Waker, event: QrEvent) {
    if tx.send(event).await.is_ok() {
        waker();
    }
}

struct RsaSession {
    decryptor: OaepPrivateDecryptingKey,
    encoded_public_key: String,
    expected_fingerprint: String,
}

impl RsaSession {
    fn generate() -> Result<Self, String> {
        let private_key = PrivateDecryptingKey::generate(KeySize::Rsa2048)
            .map_err(|_| "Failed to generate RSA keypair".to_owned())?;
        let spki = private_key
            .public_key()
            .as_der()
            .map_err(|_| "Failed to export RSA public key".to_owned())?;
        let encoded_public_key = STANDARD.encode(spki.as_ref());
        let expected_fingerprint =
            URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, spki.as_ref()).as_ref());
        let decryptor = OaepPrivateDecryptingKey::new(private_key)
            .map_err(|_| "Failed to initialize RSA-OAEP key".to_owned())?;
        Ok(Self {
            decryptor,
            encoded_public_key,
            expected_fingerprint,
        })
    }

    fn decrypt_base64(&self, b64: &str) -> Result<Zeroizing<Vec<u8>>, String> {
        let ciphertext = STANDARD
            .decode(b64)
            .map_err(|_| "Invalid base64 payload from Discord".to_owned())?;
        let mut out = Zeroizing::new(vec![0u8; self.decryptor.min_output_size()]);
        let len = self
            .decryptor
            .decrypt(&OAEP_SHA256_MGF1SHA256, &ciphertext, &mut out, None)
            .map_err(|_| "Failed to decrypt remote auth payload".to_owned())?
            .len();
        out.truncate(len);
        Ok(out)
    }
}

pub fn render_qr_rgba(
    data: &str,
    module_px: u32,
    quiet_modules: u32,
) -> Result<(u32, u32, Vec<u8>), String> {
    let code = QrCode::new(data.as_bytes()).map_err(|_| "Failed to encode QR code".to_owned())?;
    let modules = code.width() as u32;
    let total_modules = modules + quiet_modules * 2;
    let side = total_modules * module_px;
    let mut rgba = vec![255u8; (side * side * 4) as usize];
    for my in 0..modules {
        for mx in 0..modules {
            if code[(mx as usize, my as usize)] == Color::Dark {
                let x0 = (mx + quiet_modules) * module_px;
                let y0 = (my + quiet_modules) * module_px;
                for dy in 0..module_px {
                    for dx in 0..module_px {
                        let idx = (((y0 + dy) * side + (x0 + dx)) * 4) as usize;
                        rgba[idx] = 17;
                        rgba[idx + 1] = 17;
                        rgba[idx + 2] = 20;
                        rgba[idx + 3] = 255;
                    }
                }
            }
        }
    }
    Ok((side, side, rgba))
}

fn parse_scanned_user(payload: &str) -> Option<String> {
    let mut parts = payload.splitn(4, ':');
    let _user_id = parts.next()?;
    let discriminator = parts.next()?;
    let _avatar = parts.next()?;
    let username = parts.next()?.trim();
    if username.is_empty() {
        return None;
    }
    if discriminator == "0" || discriminator.is_empty() {
        Some(username.to_owned())
    } else {
        Some(format!("{username}#{discriminator}"))
    }
}

async fn exchange_ticket(ticket: &str) -> Result<String, String> {
    let client = Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| "Cannot initialize HTTPS client".to_owned())?;
    let response = client
        .post(REMOTE_AUTH_LOGIN_URL)
        .json(&json!({ "ticket": ticket }))
        .send()
        .await
        .map_err(|_| "Remote auth ticket exchange failed (network/TLS)".to_owned())?;
    if !response.status().is_success() {
        return Err(format!(
            "Discord rejected QR login ticket (HTTP {})",
            response.status().as_u16()
        ));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|_| "Invalid ticket exchange response".to_owned())?;
    body["encrypted_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "Missing encrypted token from Discord".to_owned())
}

async fn run_remote_auth(tx: &mpsc::Sender<QrEvent>, waker: &rune::Waker) -> Result<(), String> {
    crate::ensure_tls_provider();
    send_event(tx, waker, QrEvent::Status("Generating QR key…".into())).await;
    let rsa = RsaSession::generate()?;

    send_event(
        tx,
        waker,
        QrEvent::Status("Connecting to Discord remote auth…".into()),
    )
    .await;
    let mut request = REMOTE_AUTH_GATEWAY
        .into_client_request()
        .map_err(|_| "Invalid remote auth gateway URL".to_owned())?;
    request.headers_mut().insert(
        "Origin",
        REMOTE_AUTH_ORIGIN
            .parse()
            .map_err(|_| "Invalid Origin header".to_owned())?,
    );
    let (mut socket, _) = timeout(Duration::from_secs(30), connect_async(request))
        .await
        .map_err(|_| "Remote auth connection timed out".to_owned())?
        .map_err(|e| format!("Remote auth connection failed: {e}"))?;

    let mut heartbeat_interval = Duration::from_secs(41);
    let mut next_heartbeat = Instant::now() + heartbeat_interval;
    let mut awaiting_ack = false;

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(next_heartbeat) => {
                if awaiting_ack {
                    return Err("QR session timed out. Click Refresh QR to try again.".into());
                }
                socket
                    .send(Message::Text(json!({"op": "heartbeat"}).to_string().into()))
                    .await
                    .map_err(|_| "Remote auth heartbeat failed".to_owned())?;
                awaiting_ack = true;
                next_heartbeat = Instant::now() + heartbeat_interval;
            }
            frame = socket.next() => {
                let frame = frame
                    .ok_or_else(|| "QR session closed. Click Refresh QR to try again.".to_owned())?
                    .map_err(|e| format!("Remote auth read error: {e}"))?;
                match frame {
                    Message::Close(close) => {
                        let code = close.as_ref().map(|c| u16::from(c.code)).unwrap_or(1000);
                        return match code {
                            4003 => Err("QR code expired. Click Refresh QR for a new one.".into()),
                            1000 => Err("QR session ended. Click Refresh QR to try again.".into()),
                            _ => Err(format!("Remote auth closed (code {code}). Click Refresh QR.")),
                        };
                    }
                    Message::Ping(data) => {
                        socket
                            .send(Message::Pong(data))
                            .await
                            .map_err(|_| "Remote auth pong failed".to_owned())?;
                    }
                    Message::Text(text) => {
                        let payload: Value = serde_json::from_str(&text)
                            .map_err(|_| "Invalid remote auth message".to_owned())?;
                        match payload["op"].as_str().unwrap_or_default() {
                            "hello" => {
                                if let Some(ms) = payload["heartbeat_interval"].as_u64().filter(|n| *n > 0) {
                                    heartbeat_interval = Duration::from_millis(ms);
                                    next_heartbeat = Instant::now() + heartbeat_interval;
                                }
                                let init = json!({
                                    "op": "init",
                                    "encoded_public_key": rsa.encoded_public_key,
                                });
                                socket
                                    .send(Message::Text(init.to_string().into()))
                                    .await
                                    .map_err(|_| "Failed to send remote auth init".to_owned())?;
                            }
                            "heartbeat_ack" => {
                                awaiting_ack = false;
                            }
                            "nonce_proof" => {
                                let encrypted_nonce = payload["encrypted_nonce"]
                                    .as_str()
                                    .ok_or_else(|| "Missing nonce proof challenge".to_owned())?;
                                let nonce = rsa.decrypt_base64(encrypted_nonce)?;
                                let proof = URL_SAFE_NO_PAD.encode(&*nonce);
                                socket
                                    .send(Message::Text(json!({"op": "nonce_proof", "nonce": proof}).to_string().into()))
                                    .await
                                    .map_err(|_| "Failed to send nonce proof".to_owned())?;
                            }
                            "pending_remote_init" => {
                                let fingerprint = payload["fingerprint"]
                                    .as_str()
                                    .ok_or_else(|| "Missing QR fingerprint".to_owned())?;
                                if fingerprint != rsa.expected_fingerprint {
                                    return Err("QR fingerprint mismatch; please refresh.".into());
                                }
                                let url = format!("{QR_URL_PREFIX}{fingerprint}");
                                let (width, height, rgba) = render_qr_rgba(&url, 6, 4)?;
                                send_event(tx, waker, QrEvent::Ready { width, height, rgba }).await;
                            }
                            "pending_ticket" => {
                                let label = payload["encrypted_user_payload"]
                                    .as_str()
                                    .and_then(|enc| rsa.decrypt_base64(enc).ok())
                                    .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
                                    .and_then(|raw| parse_scanned_user(&raw))
                                    .unwrap_or_else(|| "your account".into());
                                send_event(tx, waker, QrEvent::Scanned(label)).await;
                            }
                            "pending_login" => {
                                let ticket = payload["ticket"]
                                    .as_str()
                                    .filter(|s| !s.is_empty())
                                    .ok_or_else(|| "Missing login ticket".to_owned())?;
                                send_event(tx, waker, QrEvent::Status("Finishing sign-in…".into())).await;
                                let encrypted_token = exchange_ticket(ticket).await?;
                                let token_bytes = rsa.decrypt_base64(&encrypted_token)?;
                                let token = String::from_utf8(token_bytes.to_vec())
                                    .map_err(|_| "Invalid UTF-8 in decrypted token".to_owned())?;
                                let token = Zeroizing::new(token);
                                send_event(tx, waker, QrEvent::Token(token)).await;
                                return Ok(());
                            }
                            "cancel" => {
                                return Err("Login canceled on phone. Click Refresh QR to try again.".into());
                            }
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
    use aws_lc_rs::rsa::OaepPublicEncryptingKey;

    #[test]
    fn rsa_session_roundtrip_and_fingerprint_match() {
        let private_key = PrivateDecryptingKey::generate(KeySize::Rsa2048).unwrap();
        let spki = private_key.public_key().as_der().unwrap();
        let pub_encryptor = OaepPublicEncryptingKey::new(private_key.public_key()).unwrap();
        let session = RsaSession {
            decryptor: OaepPrivateDecryptingKey::new(private_key).unwrap(),
            encoded_public_key: STANDARD.encode(spki.as_ref()),
            expected_fingerprint: URL_SAFE_NO_PAD
                .encode(digest::digest(&digest::SHA256, spki.as_ref()).as_ref()),
        };

        assert!(!session.encoded_public_key.is_empty());
        assert_eq!(session.expected_fingerprint.len(), 43);

        let secret = b"test-nonce-or-token-payload";
        let mut ct = vec![0u8; pub_encryptor.ciphertext_size()];
        let encrypted = pub_encryptor
            .encrypt(&OAEP_SHA256_MGF1SHA256, secret, &mut ct, None)
            .unwrap();
        let decrypted = session.decrypt_base64(&STANDARD.encode(encrypted)).unwrap();
        assert_eq!(&*decrypted, secret);
    }

    #[test]
    fn parses_scanned_user_payloads() {
        assert_eq!(
            parse_scanned_user("123456:0:abc:gencay").as_deref(),
            Some("gencay")
        );
        assert_eq!(
            parse_scanned_user("123456:1234:abc:Ada").as_deref(),
            Some("Ada#1234")
        );
        assert!(parse_scanned_user("bad").is_none());
    }

    #[test]
    fn renders_crisp_qr_rgba_bitmap() {
        let url = "https://discord.com/ra/UZ0-kOVzXDZTFVV5_QlpURSO2BQHrtkKWHNpIGoDI0k";
        let (w, h, rgba) = render_qr_rgba(url, 6, 4).unwrap();
        assert_eq!(w, h);
        assert!(w >= 180);
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        // Top-left quiet zone is white, and there are dark modules inside.
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        assert!(rgba.as_chunks::<4>().0.iter().any(|px| px[0] < 50));
    }

    #[tokio::test]
    #[ignore = "contacts live Discord remote-auth-gateway without credentials"]
    async fn live_remote_auth_handshake_reaches_ready() {
        let (tx, mut rx) = mpsc::channel(16);
        let waker: rune::Waker = std::sync::Arc::new(|| {});
        let handle = tokio::spawn(async move {
            if let Err(err) = run_remote_auth(&tx, &waker).await {
                let _ = tx.try_send(QrEvent::Error(err));
            }
        });

        let deadline = tokio::time::timeout(Duration::from_secs(15), async {
            while let Some(ev) = rx.recv().await {
                match ev {
                    QrEvent::Ready {
                        width,
                        height,
                        rgba,
                    } => {
                        assert!(width >= 180 && width == height);
                        assert_eq!(rgba.len(), (width * height * 4) as usize);
                        return Ok(());
                    }
                    QrEvent::Error(err) => return Err(err),
                    QrEvent::Status(_) => continue,
                    other => {
                        return Err(format!(
                            "unexpected event: {:?}",
                            std::mem::discriminant(&other)
                        ))
                    }
                }
            }
            Err("channel closed before Ready".into())
        })
        .await;

        handle.abort();
        deadline
            .expect("timed out waiting for live QR")
            .expect("live Discord remote auth handshake failed");
    }
}
