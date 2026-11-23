use crate::model::{Channel, Guild, Message, User};
use std::collections::{HashMap, VecDeque};

pub const MESSAGE_CAP: usize = 200;
/// Maximum number of channel transcripts held at once. Channels are evicted
/// least-recently-used and refetched on demand when selected again.
pub const CHANNEL_CAP: usize = 32;
#[derive(Default)]
pub struct Store {
    pub typing: crate::typing::Typing,
    pub user: Option<User>,
    pub guilds: HashMap<String, Guild>,
    pub channels: HashMap<String, Channel>,
    pub messages: HashMap<String, VecDeque<Message>>,
    /// LRU order of `messages` keys, least recent first.
    recency: VecDeque<String>,
    /// Selected channel, exempt from eviction.
    pinned: Option<String>,
}
impl Store {
    pub fn guild(&mut self, mut guild: Guild) {
        for mut channel in guild.channels.drain(..) {
            channel.guild_id = Some(guild.id.clone());
            self.channels.insert(channel.id.clone(), channel);
        }
        self.guilds.insert(guild.id.clone(), guild);
    }
    pub fn pin(&mut self, channel: Option<String>) {
        self.pinned = channel;
    }
    /// Ensure `channel` has a transcript buffer, marking it most recently used.
    /// Returns the least recently used channel evicted past CHANNEL_CAP, if any.
    pub fn ensure(&mut self, channel: &str) -> Option<String> {
        if self.messages.contains_key(channel) {
            if let Some(index) = self.recency.iter().position(|id| id == channel) {
                self.recency.remove(index);
            }
            self.recency.push_back(channel.into());
            return None;
        }
        self.messages.insert(channel.into(), VecDeque::new());
        self.recency.push_back(channel.into());
        let mut evicted = None;
        while self.messages.len() > CHANNEL_CAP {
            let Some(candidate) = self.recency.pop_front() else {
                break;
            };
            if self.pinned.as_ref() == Some(&candidate) {
                self.recency.push_back(candidate);
                continue;
            }
            self.messages.remove(&candidate);
            evicted = Some(candidate);
        }
        evicted
    }
    /// Returns the channel evicted while inserting, if any.
    pub fn insert(&mut self, message: Message) -> Option<String> {
        let evicted = self.ensure(&message.channel_id);
        let messages = self.messages.get_mut(&message.channel_id).unwrap();
        if let Some(old) = messages.iter_mut().find(|m| m.id == message.id) {
            *old = message;
            return evicted;
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
        evicted
    }
    pub fn remove_channel(&mut self, id: &str) {
        self.typing.remove_channel(id);
        self.channels.remove(id);
        self.messages.remove(id);
        if let Some(index) = self.recency.iter().position(|c| c == id) {
            self.recency.remove(index);
        }
        if self.pinned.as_deref() == Some(id) {
            self.pinned = None;
        }
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
            embeds: Vec::new(),
            attachments: Vec::new(),
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
    fn message_in(channel: usize, id: usize) -> Message {
        Message {
            id: id.to_string(),
            channel_id: channel.to_string(),
            author: User::default(),
            content: "x".into(),
            timestamp: "now".into(),
            embeds: Vec::new(),
            attachments: Vec::new(),
        }
    }
    #[test]
    fn channel_cache_is_bounded_and_lru() {
        let mut store = Store::default();
        for channel in 0..100 {
            store.insert(message_in(channel, channel));
        }
        assert_eq!(store.messages.len(), CHANNEL_CAP);
        assert!(!store.messages.contains_key("67"));
        assert!(store.messages.contains_key("99"));

        // The pinned (selected) channel is exempt from eviction.
        store.pin(Some("75".into()));
        for channel in 100..150 {
            store.insert(message_in(channel, channel));
        }
        assert_eq!(store.messages.len(), CHANNEL_CAP);
        assert!(store.messages.contains_key("75"));

        // Writing to a channel makes it most recently used: promoting the
        // oldest survivor (119) shifts eviction onto the next-oldest (120).
        store.pin(None);
        store.insert(message_in(119, 1000));
        for channel in 150..170 {
            store.insert(message_in(channel, channel));
        }
        assert!(store.messages.contains_key("119"));
        assert!(!store.messages.contains_key("120"));
        assert!(store.messages.contains_key("149"));
        assert!(store.insert(message_in(200, 1)).is_some());

        store.remove_channel("149");
        assert!(!store.messages.contains_key("149"));
        for channel in 300..350 {
            store.insert(message_in(channel, channel));
        }
        assert_eq!(store.messages.len(), CHANNEL_CAP);
    }
    fn messages_bytes(store: &Store) -> usize {
        store
            .messages
            .iter()
            .map(|(channel, ms)| {
                channel.len()
                    + ms.len() * std::mem::size_of::<Message>()
                    + ms.iter()
                        .map(|m| {
                            m.id.capacity()
                                + m.channel_id.capacity()
                                + m.author.username.capacity()
                                + m.author.global_name.as_ref().map_or(0, |n| n.capacity())
                                + m.content.capacity()
                                + m.timestamp.capacity()
                        })
                        .sum::<usize>()
            })
            .sum()
    }
    #[test]
    fn channel_cache_memory_estimate() {
        let mut store = Store::default();
        let content = "lorem ipsum ".repeat(40);
        for channel in 0..100 {
            for i in 0..MESSAGE_CAP {
                store.insert(Message {
                    id: (channel * 1000 + i).to_string(),
                    channel_id: channel.to_string(),
                    author: User {
                        id: "42".into(),
                        username: "someone".into(),
                        global_name: Some("Some One".into()),
                    },
                    content: content.clone(),
                    timestamp: "2026-11-23T10:00:00Z".into(),
                    embeds: vec![],
                    attachments: vec![],
                });
            }
        }
        let bytes = messages_bytes(&store);
        println!(
            "cached channels: {}, estimated transcript bytes: {}",
            store.messages.len(),
            bytes
        );
        assert_eq!(store.messages.len(), CHANNEL_CAP);
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
