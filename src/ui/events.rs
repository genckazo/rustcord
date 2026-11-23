use super::State;
use crate::{
    model::{Channel, Guild, Message, User},
    network::Event,
    notifications,
    store::Store,
};
use rune::Tx;
use serde::Deserialize;
use serde_json::Value;

fn dispatch(s: &mut State, name: &str, data: Value) {
    match name {
        "READY" => {
            s.store = Store::default();
            s.loading.clear();
            s.exhausted.clear();
            s.store.user = serde_json::from_value::<User>(data["user"].clone()).ok();
            if let Some(users) = data["users"].as_array() {
                for user in users {
                    if let Ok(user) = serde_json::from_value::<User>(user.clone()) {
                        s.store.users.insert(user.id.clone(), user);
                    }
                }
            }
            if let Ok(guilds) = serde_json::from_value::<Vec<Guild>>(data["guilds"].clone()) {
                for g in guilds {
                    s.store.guild(g);
                }
            }
            if let Some(channels) = data["private_channels"].as_array() {
                for c in channels {
                    if let Ok(c) = serde_json::from_value::<Channel>(c.clone()) {
                        s.store.channels.insert(c.id.clone(), c);
                    }
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
        "MESSAGE_CREATE" => {
            if let Ok(m) = Message::deserialize(&data) {
                if let Some(kind) = notifications::for_message(&s.store, &m, &data) {
                    notifications::post(kind);
                }
                s.store.activity(&m);
                s.store.insert(m);
            }
        }
        "CHANNEL_RECIPIENT_ADD" | "CHANNEL_RECIPIENT_REMOVE" => {
            if let (Some(id), Ok(user)) = (
                data["channel_id"].as_str(),
                serde_json::from_value::<User>(data["user"].clone()),
            ) {
                if let Some(c) = s
                    .store
                    .channels
                    .get_mut(id)
                    .filter(|c| c.guild_id.is_none() && c.kind == 3)
                {
                    c.recipients.retain(|u| u.id != user.id);
                    c.recipient_ids.retain(|id| id != &user.id);
                    if name == "CHANNEL_RECIPIENT_ADD" {
                        c.recipients.push(user);
                    }
                }
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
    let mut changed = false;
    if s.scroll_top.replace(false) {
        if let Some(id) = s.channel.clone() {
            s.history(id, true);
            changed = true;
        }
    }
    while let Some(event) = s.network.as_mut().and_then(|n| n.rx.try_recv().ok()) {
        changed = true;
        match event {
            Event::Status(status) => s.status = status,
            Event::Dispatch(name, data) => dispatch(s, &name, data),
            Event::History(channel, result) => {
                s.loading.remove(&channel);
                match result {
                    Ok(messages) => {
                        let initial = !s.store.messages.contains_key(&channel);
                        if messages.len() < 50 {
                            s.exhausted.insert(channel.clone());
                        }
                        s.store.messages.entry(channel.clone()).or_default();
                        for m in messages {
                            s.store.insert(m);
                        }
                        if initial && s.channel.as_ref() == Some(&channel) {
                            tx.scroll_to_bottom("transcript");
                        }
                    }
                    Err(error) => s.error = error,
                }
            }
            Event::Sent(result) => {
                s.sending = false;
                match result {
                    Ok(m) => {
                        s.store.activity(&m);
                        s.store.insert(m);
                        s.draft.clear();
                        tx.scroll_to_bottom("transcript");
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
            Event::OpenedDm(result) => opened_dm(s, result, tx),
        }
    }
    changed
}
fn opened_dm(s: &mut State, result: Result<Channel, String>, tx: &mut Tx) {
    match result {
        Ok(c) => {
            let id = c.id.clone();
            s.store.channels.insert(id.clone(), c);
            s.guild = None;
            s.select(id, tx);
        }
        Err(error) => s.error = error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dm_ready_labels_order_activity_and_recipients() {
        let mut s = State::default();
        dispatch(
            &mut s,
            "READY",
            json!({"user":{"id":"1","username":"Me"},"users":[{"id":"2","username":"river","global_name":"River"},{"id":"1","username":"Me"}],"private_channels":[{"id":"10","type":1,"recipient_ids":["1","2"],"last_message_id":"9"},{"id":"11","type":3,"recipients":[{"id":"1","username":"Me"}]},{"id":"12","type":1,"last_message_id":"100"},{"id":false},{"id":"13","type":1}]}),
        );
        assert_eq!(s.store.channels.len(), 4);
        assert_eq!(s.store.channel_label(&s.store.channels["10"]), "River");
        assert_eq!(
            s.store.channel_label(&s.store.channels["11"]),
            "Unnamed group"
        );
        assert_eq!(
            s.store.channel_label(&s.store.channels["12"]),
            "Unknown user"
        );
        assert_eq!(s.store.dm_for("2").as_deref(), Some("10"));
        let order = |s: &State| {
            s.store
                .channels_for(None)
                .iter()
                .map(|c| c.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&s), ["12", "10", "11", "13"]);
        dispatch(
            &mut s,
            "MESSAGE_CREATE",
            json!({"id":"101","channel_id":"10","author":{"id":"2","username":"River"},"timestamp":"now"}),
        );
        assert_eq!(order(&s), ["10", "12", "11", "13"]);
        dispatch(
            &mut s,
            "CHANNEL_RECIPIENT_ADD",
            json!({"channel_id":"11","user":{"id":"2","username":"River"}}),
        );
        dispatch(
            &mut s,
            "CHANNEL_RECIPIENT_ADD",
            json!({"channel_id":"11","user":{"id":"2","username":"River"}}),
        );
        assert_eq!(s.store.channel_label(&s.store.channels["11"]), "River");
        dispatch(
            &mut s,
            "CHANNEL_RECIPIENT_REMOVE",
            json!({"channel_id":"11","user":{"id":"2"}}),
        );
        assert_eq!(
            s.store.channel_label(&s.store.channels["11"]),
            "Unnamed group"
        );
        dispatch(&mut s, "CHANNEL_RECIPIENT_ADD", json!({}));
        dispatch(&mut s, "READY", json!({"private_channels":null}));
        assert!(s.store.channels.is_empty());
    }

    #[test]
    fn open_dm_response_and_existing_selection() {
        let mut s = State {
            demo: true,
            guild: Some("100".into()),
            ..State::default()
        };
        let mut tx = Tx::default();
        let c = serde_json::from_value(
            json!({"id":"20","type":1,"recipients":[{"id":"2","username":"River"}]}),
        )
        .unwrap();
        opened_dm(&mut s, Ok(c), &mut tx);
        assert_eq!(s.channel.as_deref(), Some("20"));
        assert!(s.guild.is_none());
        s.channel = None;
        s.open_dm(
            serde_json::from_value(json!({"id":"2","username":"River"})).unwrap(),
            &mut tx,
        );
        assert_eq!(s.channel.as_deref(), Some("20"));
        s.store.user = Some(User {
            id: "1".into(),
            ..User::default()
        });
        s.open_dm(s.store.user.clone().unwrap(), &mut tx);
        assert_eq!(s.channel.as_deref(), Some("20"));
        opened_dm(&mut s, Err("Discord HTTP 403".into()), &mut tx);
        assert_eq!(s.error, "Discord HTTP 403");
        assert_eq!(s.channel.as_deref(), Some("20"));
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
