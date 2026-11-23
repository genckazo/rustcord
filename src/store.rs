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
#[derive(Default)]
pub struct Store {
    pub user: Option<User>,
    pub guilds: HashMap<String, Guild>,
    pub channels: HashMap<String, Channel>,
    pub messages: HashMap<String, VecDeque<Message>>,
    pub windows: HashMap<String, Window>,
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
    pub fn guild(&mut self, mut guild: Guild) {
        for mut channel in guild.channels.drain(..) {
            channel.guild_id = Some(guild.id.clone());
            self.channels.insert(channel.id.clone(), channel);
        }
        self.guilds.insert(guild.id.clone(), guild);
    }
    pub fn insert(&mut self, message: Message) {
        if self
            .windows
            .entry(message.channel_id.clone())
            .or_default()
            .has_newer
        {
            return;
        }
        self.merge(message, Direction::Newer);
    }
    pub fn history(
        &mut self,
        channel: &str,
        direction: Direction,
        page: Vec<Message>,
        limit: usize,
    ) {
        let short = page.len() < limit;
        if direction == Direction::Latest {
            self.messages.remove(channel);
            self.windows.remove(channel);
        }
        let window = self.windows.entry(channel.to_owned()).or_default();
        match direction {
            Direction::Latest => window.oldest_exhausted = short,
            Direction::Older => window.oldest_exhausted = short,
            Direction::Newer => window.has_newer = !short,
        }
        self.messages.entry(channel.to_owned()).or_default();
        for message in page {
            self.merge(message, direction);
        }
    }
    fn merge(&mut self, message: Message, direction: Direction) {
        let window = self.windows.entry(message.channel_id.clone()).or_default();
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
            if direction == Direction::Older {
                messages.pop_back();
                window.has_newer = true;
            } else {
                messages.pop_front();
                window.oldest_exhausted = false;
            }
        }
    }
    pub fn remove_channel(&mut self, id: &str) {
        self.channels.remove(id);
        self.messages.remove(id);
        self.windows.remove(id);
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
}
