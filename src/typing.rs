use serde::Deserialize;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use crate::model::User;

const EXPIRY: Duration = Duration::from_secs(10);
const THROTTLE: Duration = Duration::from_secs(8);

#[derive(Deserialize)]
pub struct TypingStart {
    pub channel_id: String,
    pub user_id: String,
    pub timestamp: u64,
    pub member: Option<Member>,
}
#[derive(Deserialize)]
pub struct Member {
    pub user: Option<User>,
    pub nick: Option<String>,
}
#[derive(Default)]
pub struct Typing {
    channels: BTreeMap<String, BTreeMap<String, (String, Instant)>>,
    sent: BTreeMap<String, Instant>,
}
impl Typing {
    pub fn receive(
        &mut self,
        event: TypingStart,
        own: Option<&str>,
        known: Option<&User>,
        now: Instant,
    ) {
        if own == Some(event.user_id.as_str()) {
            return;
        }
        let _timestamp = event.timestamp;
        let member = event.member.as_ref();
        let user = member.and_then(|m| m.user.as_ref()).or(known);
        let name = member
            .and_then(|m| m.nick.as_deref())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                user.and_then(|u| u.global_name.as_deref())
                    .filter(|s| !s.trim().is_empty())
            })
            .or_else(|| {
                user.map(|u| u.username.as_str())
                    .filter(|s| !s.trim().is_empty())
            })
            .map(str::to_owned)
            .unwrap_or_else(|| format!("User {}", event.user_id));
        self.channels
            .entry(event.channel_id)
            .or_default()
            .insert(event.user_id, (name, now + EXPIRY));
    }
    pub fn clear(&mut self, channel: &str, user: &str) {
        if let Some(users) = self.channels.get_mut(channel) {
            users.remove(user);
        }
    }
    pub fn remove_channel(&mut self, channel: &str) {
        self.channels.remove(channel);
        self.sent.remove(channel);
    }
    pub fn expire(&mut self, now: Instant) -> bool {
        let mut changed = false;
        self.channels.retain(|_, users| {
            users.retain(|_, (_, expires)| {
                let keep = *expires > now;
                changed |= !keep;
                keep
            });
            !users.is_empty()
        });
        changed
    }
    pub fn label(&self, channel: &str, now: Instant) -> String {
        let names: Vec<_> = self
            .channels
            .get(channel)
            .into_iter()
            .flat_map(|users| users.values())
            .filter(|(_, expires)| *expires > now)
            .map(|(name, _)| name.as_str())
            .collect();
        match names.as_slice() {
            [] => String::new(),
            [x] => format!("{x} is typing…"),
            [x, y] => format!("{x} and {y} are typing…"),
            [x, y, z] => format!("{x}, {y} and {z} are typing…"),
            _ => "Several people are typing…".into(),
        }
    }
    pub fn can_send(&self, channel: &str, input: &str, now: Instant) -> bool {
        !input.trim().is_empty()
            && self
                .sent
                .get(channel)
                .is_none_or(|last| now.duration_since(*last) >= THROTTLE)
    }
    pub fn sent(&mut self, channel: String, now: Instant) {
        self.sent.insert(channel, now);
    }
    pub fn reset(&mut self, channel: &str) {
        self.sent.remove(channel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn event(id: &str) -> TypingStart {
        serde_json::from_value(json!({"channel_id":"1","user_id":id,"timestamp":123})).unwrap()
    }
    #[test]
    fn expiry_refresh_clear_and_channels() {
        let now = Instant::now();
        let mut typing = Typing::default();
        typing.receive(event("1"), Some("1"), None, now);
        assert_eq!(typing.label("1", now), "");
        typing.receive(event("2"), None, None, now);
        typing.receive(event("2"), None, None, now + Duration::from_secs(9));
        assert!(!typing.expire(now + EXPIRY));
        assert_eq!(typing.label("2", now), "");
        assert!(typing.expire(now + Duration::from_secs(19)));
        typing.receive(event("2"), None, None, now);
        typing.clear("2", "2");
        assert_ne!(typing.label("1", now), "");
        typing.clear("1", "2");
        assert_eq!(typing.label("1", now), "");
        typing.receive(event("2"), None, None, now);
        typing.remove_channel("1");
        assert_eq!(typing.label("1", now), "");
    }
    #[test]
    fn names_and_counts() {
        let now = Instant::now();
        let mut typing = Typing::default();
        let known = User {
            id: "1".into(),
            username: "Ada".into(),
            global_name: None,
        };
        typing.receive(event("1"), None, Some(&known), now);
        assert_eq!(typing.label("1", now), "Ada is typing…");
        let mut second = event("2");
        second.member = Some(Member {
            nick: Some("Bee".into()),
            user: Some(known.clone()),
        });
        typing.receive(second, None, None, now);
        assert_eq!(typing.label("1", now), "Ada and Bee are typing…");
        let mut third = event("3");
        third.member = Some(Member {
            nick: Some(" ".into()),
            user: Some(User {
                global_name: Some("Cee".into()),
                ..known
            }),
        });
        typing.receive(third, None, None, now);
        assert_eq!(typing.label("1", now), "Ada, Bee and Cee are typing…");
        typing.receive(event("4"), None, None, now);
        assert_eq!(typing.label("1", now), "Several people are typing…");
        assert_eq!(typing.label("1", now + EXPIRY), "");
        assert!(serde_json::from_value::<TypingStart>(json!({"channel_id":false})).is_err());
    }
    #[test]
    fn throttle_empty_reset_and_independent_channels() {
        let now = Instant::now();
        let mut typing = Typing::default();
        assert!(!typing.can_send("1", " \n", now));
        assert!(typing.can_send("1", "hello", now));
        typing.sent("1".into(), now);
        assert!(!typing.can_send("1", "hello", now + Duration::from_secs(7)));
        assert!(typing.can_send("2", "hello", now));
        assert!(typing.can_send("1", "hello", now + THROTTLE));
        typing.reset("1");
        assert!(typing.can_send("1", "hello", now));
    }
}
