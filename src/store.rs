use crate::model::{Channel, Guild, Message, User};
use std::collections::{HashMap, VecDeque};

pub const MESSAGE_CAP: usize = 200;
#[derive(Default)]
pub struct Store {
    pub user: Option<User>,
    pub users: HashMap<String, User>,
    pub guilds: HashMap<String, Guild>,
    pub channels: HashMap<String, Channel>,
    pub messages: HashMap<String, VecDeque<Message>>,
}
impl Store {
    pub fn channel_label(&self, channel: &Channel) -> String {
        if channel.guild_id.is_some() {
            return channel.label();
        }
        if let Some(name) = channel.name.as_ref().filter(|name| !name.is_empty()) {
            return name.clone();
        }
        let names: Vec<_> = channel
            .recipients
            .iter()
            .chain(channel.recipient_ids.iter().filter_map(|id| {
                (!channel.recipients.iter().any(|u| &u.id == id))
                    .then(|| self.users.get(id))
                    .flatten()
            }))
            .filter(|u| self.user.as_ref().is_none_or(|me| me.id != u.id))
            .map(User::name)
            .filter(|name| !name.is_empty())
            .collect();
        if names.is_empty() {
            if channel.kind == 3 {
                "Unnamed group"
            } else {
                "Unknown user"
            }
            .into()
        } else {
            names.join(", ")
        }
    }
    pub fn dm_for(&self, user: &str) -> Option<String> {
        self.channels
            .values()
            .find(|c| {
                c.guild_id.is_none()
                    && c.kind == 1
                    && (c.recipients.iter().any(|u| u.id == user)
                        || c.recipient_ids.iter().any(|id| id == user))
            })
            .map(|c| c.id.clone())
    }
    pub fn activity(&mut self, message: &Message) {
        if let Some(c) = self.channels.get_mut(&message.channel_id) {
            if c.guild_id.is_none()
                && c.last_message_id
                    .as_ref()
                    .is_none_or(|id| snowflake(id) < snowflake(&message.id))
            {
                c.last_message_id = Some(message.id.clone());
            }
        }
    }
    pub fn guild(&mut self, mut guild: Guild) {
        for mut channel in guild.channels.drain(..) {
            channel.guild_id = Some(guild.id.clone());
            self.channels.insert(channel.id.clone(), channel);
        }
        self.guilds.insert(guild.id.clone(), guild);
    }
    pub fn insert(&mut self, message: Message) {
        let messages = self.messages.entry(message.channel_id.clone()).or_default();
        if let Some(old) = messages.iter_mut().find(|m| m.id == message.id) {
            *old = message;
            return;
        }
        let key = snowflake(&message.id);
        let index = messages
            .iter()
            .position(|m| snowflake(&m.id) > key)
            .unwrap_or(messages.len());
        messages.insert(index, message);
        while messages.len() > MESSAGE_CAP {
            messages.pop_front();
        }
    }
    pub fn remove_channel(&mut self, id: &str) {
        self.channels.remove(id);
        self.messages.remove(id);
    }
    pub fn channels_for(&self, guild: Option<&str>) -> Vec<&Channel> {
        let mut channels: Vec<_> = self
            .channels
            .values()
            .filter(|c| c.guild_id.as_deref() == guild)
            .collect();
        if guild.is_none() {
            channels.sort_by_key(|c| {
                (
                    std::cmp::Reverse(c.last_message_id.as_deref().map(snowflake)),
                    snowflake(&c.id),
                )
            });
        } else {
            channels.sort_by_key(|c| (c.position, snowflake(&c.id)));
        }
        channels
    }
}
fn snowflake(id: &str) -> u64 {
    id.parse().unwrap_or(0)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn message(id: usize) -> Message {
        Message {
            id: id.to_string(),
            channel_id: "1".into(),
            author: User::default(),
            content: "hello".into(),
            timestamp: "now".into(),
        }
    }
    #[test]
    fn eviction_order_and_duplicates() {
        let mut store = Store::default();
        for id in (0..250).rev() {
            store.insert(message(id));
        }
        store.insert(message(249));
        let messages = &store.messages["1"];
        assert_eq!(messages.len(), 200);
        assert_eq!(messages.front().unwrap().id, "50");
        assert_eq!(messages.back().unwrap().id, "249");
        store.remove_channel("1");
        assert!(store.messages.is_empty());
    }
    #[test]
    fn empty_and_channel_ordering() {
        let mut store = Store::default();
        assert!(store.channels_for(None).is_empty());
        for (id, position) in [("10", 2), ("2", 1), ("3", 1)] {
            let c: Channel =
                serde_json::from_value(serde_json::json!({"id":id,"type":0,"position":position}))
                    .unwrap();
            store.channels.insert(id.into(), c);
        }
        assert_eq!(
            store
                .channels_for(None)
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["2", "3", "10"]
        );
    }
    #[test]
    fn activity_is_monotonic_and_does_not_reorder_guilds() {
        let mut store = Store::default();
        for (id, position) in [("1", 2), ("2", 1)] {
            let c = serde_json::from_value(
                serde_json::json!({"id":id,"type":0,"guild_id":"5","position":position}),
            )
            .unwrap();
            store.channels.insert(id.into(), c);
        }
        store.activity(&message(100));
        assert!(store.channels["1"].last_message_id.is_none());
        assert_eq!(store.channels_for(Some("5"))[0].id, "2");
        store.channels.get_mut("1").unwrap().guild_id = None;
        store.activity(&message(100));
        store.activity(&message(9));
        assert_eq!(store.channels["1"].last_message_id.as_deref(), Some("100"));
    }
}
