use crate::model::{Channel, Guild, Message, User};
use std::collections::{HashMap, VecDeque};

pub const MESSAGE_CAP: usize = 500;
pub const HISTORY_LIMIT: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Latest,
    Older,
    Newer,
}

#[derive(Default)]
pub struct Window {
    pub has_newer: bool,
    pub oldest_exhausted: bool,
}
/// Maximum number of channel transcripts held at once. Channels are evicted
/// least-recently-used and refetched on demand when selected again.
pub const CHANNEL_CAP: usize = 32;
#[derive(Default)]
pub struct Store {
    pub typing: crate::typing::Typing,
    pub user: Option<User>,
    pub users: HashMap<String, User>,
    pub guilds: HashMap<String, Guild>,
    pub channels: HashMap<String, Channel>,
    pub messages: HashMap<String, VecDeque<Message>>,
    pub windows: HashMap<String, Window>,
    /// LRU order of `messages` keys, least recent first.
    recency: VecDeque<String>,
    /// Selected channel, exempt from eviction.
    pinned: Option<String>,
}
impl Store {
    pub fn can_page(&self, channel: &str, direction: Direction, loading: bool) -> bool {
        !loading
            && match direction {
                Direction::Latest => true,
                Direction::Older => !self
                    .windows
                    .get(channel)
                    .is_some_and(|w| w.oldest_exhausted),
                Direction::Newer => self.windows.get(channel).is_some_and(|w| w.has_newer),
            }
    }
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
            self.windows.remove(&candidate);
            evicted = Some(candidate);
        }
        evicted
    }
    /// Returns the channel evicted while inserting, if any.
    pub fn insert(&mut self, message: Message) -> Option<String> {
        if self
            .windows
            .get(&message.channel_id)
            .is_some_and(|window| window.has_newer)
        {
            return None;
        }
        self.merge(message, Direction::Newer)
    }
    pub fn history(
        &mut self,
        channel: &str,
        direction: Direction,
        page: Vec<Message>,
        limit: usize,
    ) {
        let short = page.len() < limit;
        self.ensure(channel);
        if direction == Direction::Latest {
            self.messages.get_mut(channel).unwrap().clear();
            self.windows.remove(channel);
        }
        let window = self.windows.entry(channel.to_owned()).or_default();
        match direction {
            Direction::Latest => window.oldest_exhausted = short,
            Direction::Older => window.oldest_exhausted = short,
            Direction::Newer => window.has_newer = !short,
        }
        for message in page {
            self.merge(message, direction);
        }
    }
    fn merge(&mut self, message: Message, direction: Direction) -> Option<String> {
        let evicted = self.ensure(&message.channel_id);
        let window = self.windows.entry(message.channel_id.clone()).or_default();
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
            if direction == Direction::Older {
                messages.pop_back();
                window.has_newer = true;
            } else {
                messages.pop_front();
                window.oldest_exhausted = false;
            }
        }
        evicted
    }
    pub fn remove_channel(&mut self, id: &str) {
        self.typing.remove_channel(id);
        self.channels.remove(id);
        self.messages.remove(id);
        self.windows.remove(id);
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
            embeds: Vec::new(),
            attachments: Vec::new(),
        }
    }
    #[test]
    fn eviction_order_and_duplicates() {
        let mut store = Store::default();
        for id in (0..550).rev() {
            store.insert(message(id));
        }
        store.insert(message(549));
        let messages = &store.messages["1"];
        assert_eq!(messages.len(), MESSAGE_CAP);
        assert_eq!(messages.front().unwrap().id, "50");
        assert_eq!(messages.back().unwrap().id, "549");
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
    fn history_windows_follow_channel_lru_eviction() {
        let mut store = Store::default();
        store.pin(Some("0".into()));
        for _ in 0..3 {
            store.history("0", Direction::Latest, vec![], HISTORY_LIMIT);
        }
        assert_eq!(store.recency.len(), 1);
        for channel in 1..=CHANNEL_CAP {
            store.history(
                &channel.to_string(),
                Direction::Latest,
                vec![],
                HISTORY_LIMIT,
            );
        }
        assert!(store.messages.contains_key("0"));
        assert!(!store.messages.contains_key("1"));
        assert!(!store.windows.contains_key("1"));
        assert_eq!(store.windows.len(), CHANNEL_CAP);
        assert_eq!(store.recency.len(), CHANNEL_CAP);
        assert!(store.can_page("1", Direction::Older, false));
        store.remove_channel("0");
        assert!(!store.windows.contains_key("0"));
    }
    #[test]
    fn directional_pages_live_gap_and_reattach() {
        let mut store = Store::default();
        store.history(
            "1",
            Direction::Latest,
            (100..600).rev().map(message).collect(),
            HISTORY_LIMIT,
        );
        store.history(
            "1",
            Direction::Older,
            (50..100).rev().map(message).collect(),
            HISTORY_LIMIT,
        );
        assert_eq!(store.messages["1"].front().unwrap().id, "50");
        assert_eq!(store.messages["1"].back().unwrap().id, "549");
        assert!(store.windows["1"].has_newer);
        store.insert(message(600));
        assert_eq!(store.messages["1"].back().unwrap().id, "549");
        store.history(
            "1",
            Direction::Newer,
            (550..600).rev().map(message).collect(),
            HISTORY_LIMIT,
        );
        assert_eq!(store.messages["1"].front().unwrap().id, "100");
        assert_eq!(store.messages["1"].back().unwrap().id, "599");
        assert!(store.windows["1"].has_newer);
        store.history("1", Direction::Newer, vec![message(600)], HISTORY_LIMIT);
        assert!(!store.windows["1"].has_newer);
        store.insert(message(601));
        assert_eq!(store.messages["1"].back().unwrap().id, "601");
        assert!(!store.windows["1"].oldest_exhausted);
    }
    #[test]
    fn exhaustion_empty_pages_reset_and_guards() {
        let mut store = Store::default();
        assert!(!store.can_page("1", Direction::Newer, false));
        store.history("1", Direction::Latest, vec![message(10)], HISTORY_LIMIT);
        assert!(!store.can_page("1", Direction::Older, false));
        assert!(!store.can_page("1", Direction::Latest, true));
        store.windows.get_mut("1").unwrap().has_newer = true;
        store.history("1", Direction::Newer, vec![], HISTORY_LIMIT);
        assert!(!store.can_page("1", Direction::Newer, false));
        store.windows.get_mut("1").unwrap().has_newer = true;
        store.history("1", Direction::Latest, vec![], HISTORY_LIMIT);
        assert!(store.messages["1"].is_empty());
        assert!(!store.windows["1"].has_newer);
        assert!(store.windows["1"].oldest_exhausted);
        store.history("1", Direction::Older, vec![], HISTORY_LIMIT);
        assert!(store.windows["1"].oldest_exhausted);
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
