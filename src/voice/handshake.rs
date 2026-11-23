use serde_json::{json, Value};
use songbird::{
    id::{ChannelId, GuildId, UserId},
    ConnectionInfo,
};
use std::num::NonZeroU64;

#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub guild: Option<String>,
    pub channel: String,
}
pub fn state_payload(target: Option<&Target>, mute: bool, deaf: bool) -> Value {
    json!({"op":4,"d":{"guild_id":target.and_then(|t| t.guild.as_deref()),"channel_id":target.map(|t| t.channel.as_str()),"self_mute":mute || deaf,"self_deaf":deaf}})
}
pub fn leave_payload(target: &Target, mute: bool, deaf: bool) -> Value {
    let mut payload = state_payload(Some(target), mute, deaf);
    payload["d"]["channel_id"] = Value::Null;
    payload
}
pub struct Handshake {
    pub target: Target,
    user: String,
    session: Option<String>,
    server: Option<(String, String)>,
    paired: bool,
}
impl Handshake {
    pub fn is_pending(&self) -> bool {
        self.session.is_none() || self.server.is_none()
    }

    pub fn new(target: Target, user: String) -> Self {
        Self {
            target,
            user,
            session: None,
            server: None,
            paired: false,
        }
    }
    pub fn apply(
        &mut self,
        name: &str,
        data: &Value,
    ) -> Result<Option<ConnectionInfo>, &'static str> {
        let scope = self
            .target
            .guild
            .clone()
            .unwrap_or_else(|| self.target.channel.clone());
        match name {
            "VOICE_STATE_UPDATE" => {
                if data["user_id"].as_str() != Some(&self.user) {
                    return Ok(None);
                }
                if data["guild_id"].as_str() != self.target.guild.as_deref() {
                    return Ok(None);
                }
                let Some(channel) = data["channel_id"].as_str() else {
                    return Ok(None);
                };
                if channel != self.target.channel {
                    if self.target.guild.is_none() {
                        return Ok(None);
                    }
                    id(channel)?;
                    self.target.channel = channel.to_owned();
                    if self.paired {
                        self.server = None;
                    }
                    self.paired = false;
                }
                self.session = data["session_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
            }
            "VOICE_SERVER_UPDATE" => {
                let event_scope = data["guild_id"]
                    .as_str()
                    .or_else(|| data["channel_id"].as_str());
                if event_scope != Some(scope.as_str()) {
                    return Ok(None);
                }
                if self.paired {
                    self.session = None;
                    self.paired = false;
                }
                if data["endpoint"].is_null() {
                    self.server = None;
                    return Ok(None);
                }
                let endpoint = data["endpoint"].as_str().ok_or("Missing voice endpoint")?;
                if !allowed_endpoint(endpoint) {
                    return Err("Untrusted voice endpoint rejected");
                }
                let token = data["token"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or("Missing voice token")?;
                self.server = Some((endpoint.to_owned(), token.to_owned()));
            }
            _ => return Ok(None),
        }
        if let (Some(session), Some((endpoint, token))) = (&self.session, &self.server) {
            self.paired = true;
            Ok(Some(ConnectionInfo {
                channel_id: ChannelId(id(&self.target.channel)?),
                guild_id: GuildId(id(&scope)?),
                user_id: UserId(id(&self.user)?),
                session_id: session.clone(),
                endpoint: endpoint.clone(),
                token: token.clone(),
            }))
        } else {
            Ok(None)
        }
    }
}
fn id(value: &str) -> Result<NonZeroU64, &'static str> {
    value
        .parse::<u64>()
        .ok()
        .and_then(NonZeroU64::new)
        .ok_or("Invalid voice snowflake")
}
fn allowed_endpoint(endpoint: &str) -> bool {
    let host = if let Some((host, port)) = endpoint.split_once(':') {
        if port.is_empty()
            || !port.bytes().all(|c| c.is_ascii_digit())
            || !port.parse::<u16>().is_ok_and(|p| p != 0)
        {
            return false;
        }
        host
    } else {
        endpoint
    };
    host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
        && (host.ends_with(".discord.media") || host.ends_with(".discord.gg"))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> Value {
        json!({"user_id":"1","guild_id":"2","channel_id":"3","session_id":"session"})
    }
    fn server() -> Value {
        json!({"guild_id":"2","endpoint":"test.discord.media:443","token":"test-only"})
    }
    #[test]
    fn endpoint_ports_and_trust_boundary() {
        for endpoint in [
            "sweetwater-12345.discord.media:2048",
            "test.discord.gg:65535",
            "test.discord.media:1",
            "test.discord.media",
        ] {
            assert!(allowed_endpoint(endpoint), "{endpoint}");
            let mut h = Handshake::new(
                Target {
                    guild: Some("2".into()),
                    channel: "3".into(),
                },
                "1".into(),
            );
            h.apply("VOICE_STATE_UPDATE", &state()).unwrap();
            let info = h
                .apply(
                    "VOICE_SERVER_UPDATE",
                    &json!({"guild_id":"2","endpoint":endpoint,"token":"test"}),
                )
                .unwrap()
                .unwrap();
            assert_eq!(info.endpoint, endpoint);
        }
        for endpoint in [
            "test.discord.media:0",
            "test.discord.media:65536",
            "test.discord.media:",
            "test.discord.media:+443",
            "test.discord.media:-1",
            "test.discord.media:abc",
            "test.discord.media:443:443",
            "user@test.discord.media:443",
            "test.discord.media/path",
            "test.discord.media?x",
            "test.discord.media#x",
            "wss://test.discord.media",
            "discord.media",
            "test.discord.media.evil.example:443",
            "evil.example:2048",
            ".discord.media",
            "-test.discord.media",
            "test..discord.media",
        ] {
            assert!(!allowed_endpoint(endpoint), "{endpoint}");
        }
    }
    #[test]
    fn moderator_move_pairs_fresh_events_in_both_orders() {
        for server_first in [false, true] {
            let mut h = Handshake::new(
                Target {
                    guild: Some("2".into()),
                    channel: "3".into(),
                },
                "1".into(),
            );
            h.apply("VOICE_STATE_UPDATE", &state()).unwrap();
            assert!(h.apply("VOICE_SERVER_UPDATE", &server()).unwrap().is_some());
            assert!(!h.is_pending());
            let moved =
                json!({"user_id":"1","guild_id":"2","channel_id":"4","session_id":"new-session"});
            let new_server =
                json!({"guild_id":"2","endpoint":"new.discord.media:2048","token":"new-token"});
            let events = if server_first {
                [
                    ("VOICE_SERVER_UPDATE", new_server),
                    ("VOICE_STATE_UPDATE", moved),
                ]
            } else {
                [
                    ("VOICE_STATE_UPDATE", moved),
                    ("VOICE_SERVER_UPDATE", new_server),
                ]
            };
            assert!(h.apply(events[0].0, &events[0].1).unwrap().is_none());
            assert!(h.is_pending());
            let info = h.apply(events[1].0, &events[1].1).unwrap().unwrap();
            assert_eq!(h.target.channel, "4");
            assert!(!h.is_pending());
            assert_eq!(info.channel_id.0.get(), 4);
            assert_eq!(info.session_id, "new-session");
            assert_eq!(info.token, "new-token");
            assert_eq!(info.endpoint, "new.discord.media:2048");
        }
    }
    #[test]
    fn both_orders_build_connection_info() {
        for reverse in [false, true] {
            let mut h = Handshake::new(
                Target {
                    guild: Some("2".into()),
                    channel: "3".into(),
                },
                "1".into(),
            );
            let events = if reverse {
                [
                    ("VOICE_SERVER_UPDATE", server()),
                    ("VOICE_STATE_UPDATE", state()),
                ]
            } else {
                [
                    ("VOICE_STATE_UPDATE", state()),
                    ("VOICE_SERVER_UPDATE", server()),
                ]
            };
            assert!(h.apply(events[0].0, &events[0].1).unwrap().is_none());
            let info = h.apply(events[1].0, &events[1].1).unwrap().unwrap();
            assert_eq!(info.guild_id.0.get(), 2);
            assert_eq!(info.channel_id.0.get(), 3);
            assert_eq!(info.user_id.0.get(), 1);
            assert_eq!(info.session_id, "session");
        }
    }
    #[test]
    fn dm_uses_channel_as_voice_server_id() {
        let mut h = Handshake::new(
            Target {
                guild: None,
                channel: "3".into(),
            },
            "1".into(),
        );
        h.apply(
            "VOICE_SERVER_UPDATE",
            &json!({"channel_id":"3","endpoint":"test.discord.media","token":"test-only"}),
        )
        .unwrap();
        let info = h
            .apply(
                "VOICE_STATE_UPDATE",
                &json!({"user_id":"1","channel_id":"3","session_id":"dm"}),
            )
            .unwrap()
            .unwrap();
        assert_eq!(info.guild_id.0.get(), 3);
    }
    #[test]
    fn rejects_unrelated_empty_and_untrusted_events() {
        let mut h = Handshake::new(
            Target {
                guild: Some("2".into()),
                channel: "3".into(),
            },
            "1".into(),
        );
        assert!(h.apply("VOICE_STATE_UPDATE", &json!({})).unwrap().is_none());
        assert!(h
            .apply("VOICE_SERVER_UPDATE", &json!({"guild_id":"other"}))
            .unwrap()
            .is_none());
        assert!(h
            .apply(
                "VOICE_SERVER_UPDATE",
                &json!({"guild_id":"2","endpoint":"evil.example","token":"x"})
            )
            .is_err());
        assert!(h
            .apply(
                "VOICE_SERVER_UPDATE",
                &json!({"guild_id":"2","endpoint":null})
            )
            .unwrap()
            .is_none());
    }
    #[test]
    fn opcode_four_guild_dm_and_leave() {
        let guild = Target {
            guild: Some("2".into()),
            channel: "3".into(),
        };
        assert_eq!(
            state_payload(Some(&guild), false, false),
            json!({"op":4,"d":{"guild_id":"2","channel_id":"3","self_mute":false,"self_deaf":false}})
        );
        let dm = Target {
            guild: None,
            channel: "3".into(),
        };
        assert_eq!(
            state_payload(Some(&dm), false, true)["d"],
            json!({"guild_id":null,"channel_id":"3","self_mute":true,"self_deaf":true})
        );
        assert_eq!(
            state_payload(None, false, false)["d"]["channel_id"],
            Value::Null
        );
        assert_eq!(
            leave_payload(&guild, true, false),
            json!({"op":4,"d":{"guild_id":"2","channel_id":null,"self_mute":true,"self_deaf":false}})
        );
        assert_eq!(
            leave_payload(&dm, false, false),
            json!({"op":4,"d":{"guild_id":null,"channel_id":null,"self_mute":false,"self_deaf":false}})
        );
    }
    #[test]
    fn malformed_session_token_and_snowflakes() {
        for user in ["0", "invalid"] {
            let mut h = Handshake::new(
                Target {
                    guild: Some("2".into()),
                    channel: "3".into(),
                },
                user.into(),
            );
            h.apply("VOICE_SERVER_UPDATE", &server()).unwrap();
            assert!(h
                .apply(
                    "VOICE_STATE_UPDATE",
                    &json!({"user_id":user,"guild_id":"2","channel_id":"3","session_id":"session"})
                )
                .is_err());
        }
        let mut h = Handshake::new(
            Target {
                guild: Some("2".into()),
                channel: "3".into(),
            },
            "1".into(),
        );
        assert!(h
            .apply(
                "VOICE_STATE_UPDATE",
                &json!({"user_id":"1","guild_id":"2","channel_id":"3","session_id":""})
            )
            .unwrap()
            .is_none());
        assert!(h
            .apply(
                "VOICE_SERVER_UPDATE",
                &json!({"guild_id":"2","endpoint":"test.discord.media","token":""})
            )
            .is_err());
        for endpoint in [
            "evil.example",
            "test.discord.media.evil.example",
            "test.discord.media@evil.example",
            "https://test.discord.media",
            "test.discord.media:65536",
        ] {
            assert!(!allowed_endpoint(endpoint));
        }
    }
}
