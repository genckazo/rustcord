use super::{button, demo as main_demo, label, State};
use crate::voice::{handshake::Target, Command};
use rune::{
    col, icon, row, text,
    tok::{self, type_},
    El, Icon,
};

static SPEAKER: Icon = Icon {
    name: "speaker",
    view: 24.0,
    body: r#"<path d="M3 9h4l5-4v14l-5-4H3z"/><path d="M16 8c3 2 3 6 0 8m3-11c5 4 5 10 0 14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
};

pub(super) fn speaker_label(value: impl Into<String>) -> El {
    row()
        .w_full()
        .gap(8.0)
        .center_y()
        .child(icon(&SPEAKER, 18.0).color(tok::theme(true).text))
        .child(label(value).wrap().grow(1.0).min_w(0.0))
}

pub(super) fn members(s: &State, channel: &str) -> El {
    let mut list = col().w_full().gap(6.0).pl(18.0);
    for (id, name) in s.store.voice_members(channel) {
        let speaking = s.voice.speaking.contains(&id);
        list = list.child(
            text(format!("{} {name}", if speaking { "●" } else { "○" }))
                .font(type_::BODY)
                .color(if speaking {
                    rune::oklch(0.78, 0.13, 155.0)
                } else {
                    tok::theme(true).text_secondary
                })
                .wrap()
                .w_full(),
        );
    }
    list
}
pub(super) fn panel(s: &State) -> El {
    let t = tok::theme(true);
    let mut panel = col()
        .w_full()
        .padding(12.0)
        .gap(8.0)
        .fill(t.card)
        .border(1.0, t.card_border);
    let channel = s
        .voice
        .target
        .as_ref()
        .map(|target| {
            s.store
                .channels
                .get(&target.channel)
                .map(|c| c.label())
                .unwrap_or_else(|| target.channel.clone())
        })
        .unwrap_or_default();
    panel = panel.child(speaker_label(format!("{channel} · {}", s.voice.status)));
    if let Some(target) = &s.voice.target {
        panel = panel.child(
            row()
                .w_full()
                .gap(12.0)
                .child(
                    button(
                        "voice-mute",
                        if s.voice.mute { "Unmute" } else { "Mute" },
                        s.voice.mute,
                        |s, _| s.voice_command(Command::Mute(!s.voice.mute)),
                    )
                    .w(140.0),
                )
                .child(
                    button(
                        "voice-deafen",
                        if s.voice.deaf { "Undeafen" } else { "Deafen" },
                        s.voice.deaf,
                        |s, _| s.voice_command(Command::Deafen(!s.voice.deaf)),
                    )
                    .w(140.0),
                )
                .child(
                    button("voice-leave", "Disconnect", false, |s, _| {
                        s.voice_command(Command::Leave)
                    })
                    .w(140.0),
                ),
        );
        if s.store
            .channels
            .get(&target.channel)
            .is_some_and(|c| matches!(c.kind, 1 | 3))
        {
            panel = panel.child(members(s, &target.channel));
        }
    }
    panel
}
pub(super) fn demo(s: &mut State) {
    main_demo(s);
    if let Some(message) = s.store.messages.get_mut("102").and_then(|ms| ms.back_mut()) {
        message.content =
            "Voice and text networking run off the UI thread. Nothing gets recorded to disk."
                .into();
    }
    let channel = serde_json::from_value(serde_json::json!({"id":"104","guild_id":"100","type":2,"name":"Lounge","parent_id":"101","position":3})).unwrap();
    s.store.channels.insert("104".into(), channel);
    s.voice.target = Some(Target {
        guild: Some("100".into()),
        channel: "104".into(),
    });
    s.voice.status = "Voice connected · demo, no devices or network".into();
    for (id, name) in [("2", "Ada"), ("3", "River")] {
        let state = serde_json::from_value(serde_json::json!({"guild_id":"100","channel_id":"104","user_id":id,"member":{"user":{"id":id,"username":name}}})).unwrap();
        s.store.voice_state(state);
    }
    s.voice.speaking.insert("3".into());
}
