use super::State;
use crate::{
    model::{Channel, Guild, Message, User},
    network::Event,
    notifications,
    store::{Direction, Store},
};
use rune::Tx;
use serde::Deserialize;
use serde_json::Value;
use std::time::Instant;

fn dispatch(s: &mut State, name: &str, data: Value) {
    match name {
        "READY" => {
            s.store = Store::default();
            s.loading.clear();
            s.pending_present.clear();
            s.sidebar_limit = super::SIDEBAR_PAGE;
            s.store.user = serde_json::from_value::<User>(data["user"].clone()).ok();
            if let Ok(guilds) = serde_json::from_value::<Vec<Guild>>(data["guilds"].clone()) {
                for g in guilds {
                    s.store.guild(g);
                }
            }
            if let Ok(channels) =
                serde_json::from_value::<Vec<Channel>>(data["private_channels"].clone())
            {
                for c in channels {
                    s.store.channels.insert(c.id.clone(), c);
                }
            }
        }
        "GUILD_CREATE" => {
            if let Ok(g) = serde_json::from_value(data) {
                s.store.guild(g);
            }
        }
        "CHANNEL_CREATE" | "CHANNEL_UPDATE" => {
            if let Ok(c) = serde_json::from_value::<Channel>(data) {
                s.store.channels.insert(c.id.clone(), c);
            }
        }
        "CHANNEL_DELETE" => {
            if let Some(id) = data["id"].as_str() {
                s.store.remove_channel(id);
            }
        }
        "TYPING_START" => {
            if let Ok(event) = serde_json::from_value::<crate::typing::TypingStart>(data) {
                let known = s
                    .store
                    .messages
                    .get(&event.channel_id)
                    .and_then(|messages| {
                        messages.iter().rev().find(|m| m.author.id == event.user_id)
                    })
                    .map(|m| &m.author)
                    .or_else(|| {
                        s.store
                            .channels
                            .get(&event.channel_id)
                            .and_then(|c| c.recipients.iter().find(|u| u.id == event.user_id))
                    });
                s.store.typing.receive(
                    event,
                    s.store.user.as_ref().map(|u| u.id.as_str()),
                    known,
                    Instant::now(),
                );
            }
        }
        "MESSAGE_CREATE" => {
            if let Ok(m) = Message::deserialize(&data) {
                if let Some(kind) = notifications::for_message(&s.store, &m, &data) {
                    notifications::post(kind);
                }
                s.store.typing.clear(&m.channel_id, &m.author.id);
                s.store.insert(m);
            }
        }
        "MESSAGE_UPDATE" => {
            if let (Some(channel), Some(id)) = (data["channel_id"].as_str(), data["id"].as_str()) {
                if let Some(m) = s
                    .store
                    .messages
                    .get_mut(channel)
                    .and_then(|ms| ms.iter_mut().find(|m| m.id == id))
                {
                    if let Some(content) = data["content"].as_str() {
                        m.content = content.into();
                    }
                    // Missing fields on a partial update leave cached media unchanged.
                    if let Ok(embeds) = serde_json::from_value(data["embeds"].clone()) {
                        m.embeds = embeds;
                    }
                    if let Ok(attachments) = serde_json::from_value(data["attachments"].clone()) {
                        m.attachments = attachments;
                    }
                }
            }
        }
        "MESSAGE_DELETE" => {
            if let (Some(channel), Some(id)) = (data["channel_id"].as_str(), data["id"].as_str()) {
                if let Some(ms) = s.store.messages.get_mut(channel) {
                    ms.retain(|m| m.id != id);
                }
            }
        }
        _ => {}
    }
}
pub(super) fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
    let now = Instant::now();
    let mut changed = s.store.typing.expire(now);
    if s.scroll_top.replace(false) {
        if let Some(id) = s.channel.clone() {
            s.history(id, Direction::Older);
            changed = true;
        }
    }
    if s.scroll_bottom.replace(false) {
        if let Some(id) = s.channel.clone() {
            s.history(id, Direction::Newer);
            changed = true;
        }
    }
    while let Some(event) = s.network.as_mut().and_then(|n| n.rx.try_recv().ok()) {
        if matches!(event, Event::TypingTick) {
            continue;
        }
        changed = true;
        match event {
            Event::TypingTick => {}
            Event::Status(status) => s.status = status,
            Event::Dispatch(name, data) => dispatch(s, &name, data),
            Event::History(channel, direction, result) => {
                if s.finish_history(&channel, direction, result) {
                    tx.scroll_to_bottom("transcript");
                }
            }
            Event::Sent(result) => {
                s.sending = false;
                match result {
                    Ok(m) => {
                        s.store.typing.reset(&m.channel_id);
                        if s.finish_sent(m) {
                            tx.scroll_to_bottom("transcript");
                        }
                    }
                    Err(error) => s.error = error,
                }
            }
            Event::Channels(result) => match result {
                Ok(channels) => {
                    for c in channels {
                        s.store.channels.insert(c.id.clone(), c);
                    }
                }
                Err(error) => s.error = error,
            },
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn typing_dispatch_self_malformed_and_message_clear() {
        let mut state = State::default();
        dispatch(
            &mut state,
            "READY",
            json!({"user":{"id":"1","username":"Self"}}),
        );
        dispatch(
            &mut state,
            "TYPING_START",
            json!({"channel_id":"2","user_id":"1","timestamp":1}),
        );
        dispatch(&mut state, "TYPING_START", json!({"channel_id":false}));
        assert_eq!(state.store.typing.label("2", Instant::now()), "");
        dispatch(
            &mut state,
            "TYPING_START",
            json!({"channel_id":"2","user_id":"3","timestamp":1,"member":{"nick":"Ada","user":{"id":"3","username":"ada"}}}),
        );
        assert_eq!(
            state.store.typing.label("2", Instant::now()),
            "Ada is typing…"
        );
        dispatch(
            &mut state,
            "MESSAGE_CREATE",
            json!({"id":"10","channel_id":"2","author":{"id":"3","username":"ada"},"content":"hello","timestamp":"now"}),
        );
        assert_eq!(state.store.typing.label("2", Instant::now()), "");
        dispatch(
            &mut state,
            "TYPING_START",
            json!({"channel_id":"2","user_id":"3","timestamp":1}),
        );
        assert_eq!(
            state.store.typing.label("2", Instant::now()),
            "ada is typing…"
        );
    }

    #[test]
    fn ready_messages_partial_edit_and_deletion() {
        let mut state = State::default();
        dispatch(
            &mut state,
            "READY",
            json!({"user":{"id":"1","username":"Ada"},"guilds":[],"private_channels":[]}),
        );
        assert_eq!(state.store.user.as_ref().unwrap().name(), "Ada");
        dispatch(
            &mut state,
            "MESSAGE_CREATE",
            json!({"id":"10","channel_id":"2","author":{"id":"1","username":"Ada"},"content":"hello","timestamp":"2026-01-01T00:00:00Z"}),
        );
        dispatch(
            &mut state,
            "MESSAGE_UPDATE",
            json!({"id":"10","channel_id":"2","content":"edited"}),
        );
        assert_eq!(state.store.messages["2"][0].content, "edited");
        assert_eq!(state.store.messages["2"][0].author.name(), "Ada");
        dispatch(
            &mut state,
            "MESSAGE_DELETE",
            json!({"id":"10","channel_id":"2"}),
        );
        assert!(state.store.messages["2"].is_empty());
        dispatch(&mut state, "MESSAGE_CREATE", json!({"id":false}));
        dispatch(&mut state, "MESSAGE_UPDATE", json!({}));
        dispatch(&mut state, "UNKNOWN_EVENT", Value::Null);
        assert!(state.store.messages["2"].is_empty());
    }

    #[test]
    fn partial_message_updates_set_keep_and_clear_media() {
        let mut state = State::default();
        dispatch(
            &mut state,
            "MESSAGE_CREATE",
            json!({"id":"10","channel_id":"2","author":{"id":"1","username":"Ada"},
                   "content":"original","timestamp":"2026-01-01T00:00:00Z",
                   "attachments":[{"id":"11","filename":"notes.txt","size":128,
                     "url":"https://cdn.discordapp.com/attachments/2/11/notes.txt"}]}),
        );
        dispatch(
            &mut state,
            "MESSAGE_UPDATE",
            json!({"id":"10","channel_id":"2","embeds":[{"title":"Fetched link"}]}),
        );
        let message = &state.store.messages["2"][0];
        assert_eq!(message.content, "original");
        assert_eq!(message.attachments[0].filename, "notes.txt");
        assert_eq!(message.embeds[0].title.as_deref(), Some("Fetched link"));

        dispatch(
            &mut state,
            "MESSAGE_UPDATE",
            json!({"id":"10","channel_id":"2","content":"edited","embeds":null}),
        );
        let message = &state.store.messages["2"][0];
        assert_eq!(message.content, "edited");
        assert_eq!(message.embeds.len(), 1);
        assert_eq!(message.attachments.len(), 1);

        dispatch(
            &mut state,
            "MESSAGE_UPDATE",
            json!({"id":"10","channel_id":"2","embeds":[],"attachments":[]}),
        );
        let message = &state.store.messages["2"][0];
        assert!(message.embeds.is_empty());
        assert!(message.attachments.is_empty());
    }

    #[test]
    fn guild_channels_and_categories() {
        let mut state = State::default();
        dispatch(
            &mut state,
            "GUILD_CREATE",
            json!({"id":"1","name":"Test","channels":[{"id":"3","type":0,"position":2,"parent_id":"2"},{"id":"2","type":4,"position":0,"name":"Category"}]}),
        );
        let channels = state.store.channels_for(Some("1"));
        assert_eq!(channels[0].kind, 4);
        assert_eq!(channels[1].parent_id.as_deref(), Some("2"));
        dispatch(
            &mut state,
            "CHANNEL_UPDATE",
            json!({"id":"3","guild_id":"1","type":0,"name":"renamed","position":1}),
        );
        assert_eq!(state.store.channels["3"].label(), "renamed");
        dispatch(&mut state, "CHANNEL_DELETE", json!({"id":"3"}));
        assert!(!state.store.channels.contains_key("3"));
    }
}
