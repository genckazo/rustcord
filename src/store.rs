use crate::model::{Channel, Guild, Message, User};
use std::collections::{HashMap, VecDeque};

pub const MESSAGE_CAP: usize = 200;
#[derive(Default)]
pub struct Store {
    pub user: Option<User>,
    pub guilds: HashMap<String, Guild>,
    pub channels: HashMap<String, Channel>,
    pub messages: HashMap<String, VecDeque<Message>>,
}
impl Store {
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
        channels.sort_by_key(|c| (c.position, snowflake(&c.id)));
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
}
