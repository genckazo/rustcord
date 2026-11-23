use super::State;
use crate::{
    model::{Message, User, VoiceMember, VoiceState},
    voice::Command,
};

pub(super) fn send(s: &mut State, channel: String) {
    let id = s
        .store
        .messages
        .values()
        .flatten()
        .filter_map(|m| m.id.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    s.store.insert(Message {
        id: id.to_string(),
        channel_id: channel,
        author: s.store.user.clone().unwrap_or_else(local_user),
        content: s.draft.text.clone(),
        timestamp: "Demo · just now".into(),
        embeds: Vec::new(),
        attachments: Vec::new(),
    });
    s.draft.clear();
    s.error.clear();
}
fn local_user() -> User {
    User {
        id: "demo-self".into(),
        username: "You".into(),
        global_name: None,
    }
}
pub(super) fn voice(s: &mut State, command: Command) {
    let user = s.store.user.clone().unwrap_or_else(local_user);
    match command {
        Command::Join(target) => {
            if !s
                .store
                .channels
                .get(&target.channel)
                .is_some_and(|c| c.guild_id == target.guild && matches!(c.kind, 1..=3))
            {
                s.error = "Choose a demo voice channel or DM call.".into();
                return;
            }
            s.store
                .voice_states
                .retain(|_, state| state.user_id != user.id);
            s.store.voice_state(VoiceState {
                user_id: user.id.clone(),
                channel_id: Some(target.channel.clone()),
                guild_id: target.guild.clone(),
                member: Some(VoiceMember {
                    user: Some(user),
                    nick: None,
                }),
            });
            s.voice.target = Some(target);
            s.voice.status = "Voice connected · demo, no devices or network".into();
            s.voice.speaking.clear();
        }
        Command::Leave => {
            s.store
                .voice_states
                .retain(|_, state| state.user_id != user.id);
            s.voice.target = None;
            s.voice.speaking.clear();
            s.voice.status = "Voice disconnected · demo".into();
        }
        Command::Mute(mute) => s.voice.mute = mute,
        Command::Deafen(deaf) => s.voice.deaf = deaf,
    }
    s.error.clear();
}
