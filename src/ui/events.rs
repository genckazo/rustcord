use super::State;
use crate::{
    model::{Channel, Guild, Message, User, VoiceState},
    network::Event,
    notifications,
    search::Query,
    store::Store,
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
            if let Ok(states) =
                serde_json::from_value::<Vec<VoiceState>>(data["voice_states"].clone())
            {
                for state in states {
                    s.store.voice_state(state);
                }
            }
            if let Some(calls) = data["calls"].as_array() {
                for call in calls {
                    apply_call(s, call);
                }
            }
        }
        "VOICE_STATE_UPDATE" => {
            if let Ok(state) = serde_json::from_value(data) {
                s.store.voice_state(state);
            }
        }
        "CALL_CREATE" | "CALL_UPDATE" => apply_call(s, &data),
        "CALL_DELETE" => {
            if let Some(channel) = data["channel_id"].as_str() {
                s.store
                    .voice_states
                    .retain(|_, state| state.channel_id.as_deref() != Some(channel));
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
                s.store.activity(&m);
                s.store.typing.clear(&m.channel_id, &m.author.id);
                if let Some(evicted) = s.store.insert(m) {
                    s.exhausted.remove(&evicted);
                }
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
fn voice_snapshot(s: &mut State, voice: crate::voice::Snapshot) {
    if voice.target.is_none() && !voice.status.is_empty() && voice.status != "Voice disconnected" {
        s.error = voice.status.clone();
    }
    s.voice = voice;
}

pub(super) fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
    let now = Instant::now();
    let mut changed = s.store.typing.expire(now);
    if s.scroll_top.replace(false) {
        if let Some(id) = s.channel.clone() {
            s.history(id, true);
            changed = true;
        }
    }
    if s.scrolled_up_changed.replace(false) {
        changed = true;
    }
    while let Some(event) = s.network.as_mut().and_then(|n| n.rx.try_recv().ok()) {
        if matches!(event, Event::TypingTick) {
            continue;
        }
        changed = true;
        match event {
            Event::Voice(voice) => voice_snapshot(s, voice),
            Event::TypingTick => {}
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
                        if let Some(evicted) = s.store.ensure(&channel) {
                            s.exhausted.remove(&evicted);
                        }
                        for m in messages {
                            if let Some(evicted) = s.store.insert(m) {
                                s.exhausted.remove(&evicted);
                            }
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
                        s.store.typing.reset(&m.channel_id);
                        s.drafts.remove(&m.channel_id);
                        if let Some(evicted) = s.store.insert(m) {
                            s.exhausted.remove(&evicted);
                        }
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

fn apply_call(s: &mut State, data: &Value) {
    if let Some(states) = data["voice_states"].as_array() {
        for state in states {
            let mut state = state.clone();
            if state["channel_id"].is_null() {
                state["channel_id"] = data["channel_id"].clone();
            }
            if let Ok(state) = serde_json::from_value(state) {
                s.store.voice_state(state);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failed_voice_join_shows_error_without_a_panel() {
        for status in [
            "Voice connection failed",
            "Voice handshake timed out",
            "Untrusted voice endpoint",
            "Wait for Discord READY before joining voice",
        ] {
            let mut s = State::default();
            super::super::voice_panel::demo(&mut s);
            voice_snapshot(
                &mut s,
                crate::voice::Snapshot {
                    status: status.into(),
                    ..Default::default()
                },
            );
            assert_eq!(s.error, status);
            assert_eq!(s.voice.status, status);
            assert!(super::super::voice_panel::panel(&s).is_none());
        }
    }

    #[test]
    fn normal_voice_snapshots_do_not_replace_banner_errors() {
        let mut s = State {
            error: "Existing error".into(),
            ..Default::default()
        };
        for status in ["", "Voice disconnected"] {
            voice_snapshot(
                &mut s,
                crate::voice::Snapshot {
                    status: status.into(),
                    ..Default::default()
                },
            );
            assert_eq!(s.error, "Existing error");
        }
        super::super::voice_panel::demo(&mut s);
        let voice = s.voice.clone();
        voice_snapshot(&mut s, voice);
        assert_eq!(s.error, "Existing error");
        assert!(s.voice.target.is_some());
    }

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
    fn voice_members_ready_moves_leave_and_invalid_states() {
        let mut s = State::default();
        dispatch(
            &mut s,
            "READY",
            json!({"user":{"id":"1","username":"Me"},"guilds":[{"id":"10","name":"Guild","voice_states":[{"user_id":"2","channel_id":"20","member":{"user":{"id":"2","username":"Ada"}}}]}],"calls":[{"channel_id":"30","voice_states":[{"user_id":"1"}]}]}),
        );
        assert_eq!(
            s.store.voice_members("20"),
            vec![("2".into(), "Ada".into())]
        );
        assert_eq!(s.store.voice_members("30"), vec![("1".into(), "Me".into())]);
        dispatch(
            &mut s,
            "VOICE_STATE_UPDATE",
            json!({"guild_id":"10","user_id":"2","channel_id":"21"}),
        );
        assert!(s.store.voice_members("20").is_empty());
        assert_eq!(s.store.voice_members("21").len(), 1);
        dispatch(
            &mut s,
            "VOICE_STATE_UPDATE",
            json!({"guild_id":"10","user_id":"2","channel_id":null}),
        );
        dispatch(&mut s, "VOICE_STATE_UPDATE", json!({}));
        assert!(s.store.voice_members("21").is_empty());
        assert_eq!(s.store.voice_members("30").len(), 1);
        dispatch(
            &mut s,
            "READY",
            json!({"user":{"id":"1","username":"Me"},"guilds":[]}),
        );
        assert!(s.store.voice_states.is_empty());
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
    fn drafts_survive_channel_switch() {
        let mut state = State::default();
        let mut tx = Tx::default();
        state.select("1".into(), &mut tx);
        state.draft.replace_text("first draft");
        state.select("2".into(), &mut tx);
        assert!(state.draft.text.is_empty());
        state.draft.replace_text("second draft");
        state.select("1".into(), &mut tx);
        assert_eq!(state.draft.text, "first draft");
        state.select("2".into(), &mut tx);
        assert_eq!(state.draft.text, "second draft");
        state.draft.clear();
        state.select("1".into(), &mut tx);
        state.select("2".into(), &mut tx);
        assert!(state.draft.text.is_empty());
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
