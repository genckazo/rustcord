use crate::{model::Message, store::Store};
use serde_json::Value;

const SUPPRESS_NOTIFICATIONS: u64 = 1 << 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    DirectMessage,
    Mention,
}

pub fn for_message(store: &Store, message: &Message, event: &Value) -> Option<Kind> {
    let user = store.user.as_ref()?;
    if user.id == message.author.id
        || !matches!(event["type"].as_u64(), Some(0 | 19))
        || event["flags"].as_u64().unwrap_or(0) & SUPPRESS_NOTIFICATIONS != 0
        || store
            .messages
            .get(&message.channel_id)
            .is_some_and(|messages| messages.iter().any(|m| m.id == message.id))
    {
        return None;
    }

    if store
        .channels
        .get(&message.channel_id)
        .is_some_and(|channel| matches!(channel.kind, 1 | 3))
    {
        return Some(Kind::DirectMessage);
    }

    let mentioned = event["mentions"].as_array().is_some_and(|mentions| {
        mentions
            .iter()
            .any(|mention| mention["id"].as_str() == Some(user.id.as_str()))
    });
    (event["guild_id"].is_string() && mentioned).then_some(Kind::Mention)
}

#[cfg(target_os = "macos")]
pub fn post(kind: Kind) {
    if !rune_notify::available() || rune_notify::app_focused() {
        return;
    }
    let body = match kind {
        Kind::DirectMessage => "You have a new direct message.",
        Kind::Mention => "You were mentioned in a server.",
    };
    let id = format!("{:032x}", rand::random::<u128>());
    rune_notify::post(&rune_notify::Notification::new(id, "Rustcord").body(body));
}

#[cfg(not(target_os = "macos"))]
pub fn post(_: Kind) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, User};
    use serde_json::json;

    fn store() -> Store {
        let channels = [("dm", 1), ("group", 3), ("guild", 0)]
            .into_iter()
            .map(|(id, kind)| {
                let channel: Channel =
                    serde_json::from_value(json!({"id": id, "type": kind})).unwrap();
                (id.to_owned(), channel)
            })
            .collect();
        let mut store = Store::default();
        store.user = Some(User {
            id: "me".into(),
            ..User::default()
        });
        store.channels = channels;
        store
    }

    fn message(channel: &str) -> Message {
        Message {
            id: "123".into(),
            channel_id: channel.into(),
            author: User {
                id: "them".into(),
                ..User::default()
            },
            content: "<@me>".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            edited_timestamp: None,
            referenced_message: None,
            reactions: Vec::new(),
            embeds: Vec::new(),
            attachments: Vec::new(),
        }
    }

    #[test]
    fn direct_and_group_messages_notify_without_mentions() {
        let store = store();
        for channel in ["dm", "group"] {
            assert_eq!(
                for_message(&store, &message(channel), &json!({"type": 0})),
                Some(Kind::DirectMessage)
            );
        }
        assert_eq!(
            for_message(&store, &message("unknown"), &json!({"type": 0})),
            None
        );
    }

    #[test]
    fn only_explicit_user_mentions_in_servers_notify() {
        let store = store();
        let message = message("guild");
        assert_eq!(
            for_message(
                &store,
                &message,
                &json!({"type": 19, "guild_id": "1", "mentions": [{"id": "me"}]})
            ),
            Some(Kind::Mention)
        );
        assert_eq!(
            for_message(
                &store,
                &message,
                &json!({"type": 0, "guild_id": "1", "mentions": [{"id": "someone-else"}], "mention_everyone": true, "mention_roles": ["2"]})
            ),
            None
        );
    }

    #[test]
    fn skips_silent_system_self_duplicate_and_signed_out_messages() {
        let mut store = store();
        let mut message = message("dm");
        let event = json!({"type": 0});
        assert_eq!(
            for_message(
                &store,
                &message,
                &json!({"type": 0, "flags": SUPPRESS_NOTIFICATIONS})
            ),
            None
        );
        assert_eq!(for_message(&store, &message, &json!({"type": 6})), None);
        message.author.id = "me".into();
        assert_eq!(for_message(&store, &message, &event), None);
        message.author.id = "them".into();
        store.insert(message.clone());
        assert_eq!(for_message(&store, &message, &event), None);
        store.messages.clear();
        store.user = None;
        assert_eq!(for_message(&store, &message, &event), None);
    }
}
