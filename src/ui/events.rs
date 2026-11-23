use super::State;
use crate::{
    model::{Channel, Guild, Message, User},
    network::Event,
    notifications,
    search::Query,
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
        "MESSAGE_CREATE" => {
            if let Ok(m) = Message::deserialize(&data) {
                if let Some(kind) = notifications::for_message(&s.store, &m, &data) {
                    notifications::post(kind);
                }
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
fn apply_search(
    s: &mut State,
    query: &Query,
    offset: usize,
    result: Result<(usize, Vec<Message>), String>,
) {
    if let Some(search) = s.search.as_mut().filter(|x| &x.query == query) {
        search.apply(offset, result);
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
                        s.store.insert(m);
                        s.draft.clear();
                        tx.scroll_to_bottom("transcript");
                    }
                    Err(error) => s.error = error,
                }
            }
            Event::Search {
                query,
                offset,
                result,
            } => apply_search(s, &query, offset, result),
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
    fn search_results_apply_only_to_current_query() {
        use crate::search::{Scope, Search};
        let mut state = State::default();
        let scope = Scope::Channel("2".into());
        let current = Query::new(scope.clone(), "hello").unwrap();
        let stale = Query::new(scope, "old").unwrap();
        let hit: Message = serde_json::from_value(json!({"id":"10","channel_id":"2","author":{"id":"1","username":"Ada"},"content":"hello","timestamp":"t"})).unwrap();
        apply_search(&mut state, &current, 0, Ok((1, vec![hit.clone()])));
        assert!(state.search.is_none());
        state.search = Some(Search::new(current.clone()));
        apply_search(&mut state, &stale, 0, Ok((1, vec![hit.clone()])));
        assert!(state.search.as_ref().unwrap().results.is_empty());
        apply_search(&mut state, &current, 0, Ok((1, vec![hit])));
        let search = state.search.as_ref().unwrap();
        assert_eq!(search.results.len(), 1);
        assert!(!search.loading);
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
