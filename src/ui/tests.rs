use super::*;
use crate::voice::{handshake::Target, Command as Voice};

#[test]
fn empty_token_has_visible_error_without_network() {
    for token in ["", " \t\n "] {
        let mut s = State::default();
        s.token.replace_text(token);
        connect(&mut s, &mut Tx::default());
        assert_eq!(s.error, "Enter your Discord token to connect.");
        assert!(s.network.is_none());
    }
}
#[test]
fn demo_send_appends_clears_and_rejects_empty_or_long_messages() {
    let mut s = State::default();
    demo(&mut s);
    let mut tx = Tx::default();
    let count = s.store.messages["102"].len();
    for content in ["", " \t"] {
        s.draft.replace_text(content);
        send(&mut s, &mut tx);
    }
    assert_eq!(s.store.messages["102"].len(), count);
    s.draft.replace_text("x".repeat(2001));
    send(&mut s, &mut tx);
    assert!(s.error.contains("2,000"));
    assert_eq!(s.store.messages["102"].len(), count);
    for content in ["First demo message", "Second demo message"] {
        s.draft.replace_text(content);
        send(&mut s, &mut tx);
        let message = s.store.messages["102"].back().unwrap();
        assert_eq!(message.content, content);
        assert_eq!(message.author.id, "demo-self");
        assert!(s.draft.text.is_empty());
        assert!(s.error.is_empty());
    }
    assert_eq!(s.store.messages["102"].len(), count + 2);
    assert!(s.network.is_none());
    assert!(!s.sending);
    s.channel = Some("200".into());
    s.draft.replace_text("DM reply");
    send(&mut s, &mut tx);
    assert_eq!(s.store.messages["200"].back().unwrap().content, "DM reply");
    s.channel = None;
    s.draft.replace_text("No channel");
    send(&mut s, &mut tx);
    assert_eq!(s.draft.text, "No channel");
}
#[test]
fn demo_voice_join_toggles_move_leave_and_invalid_target() {
    let mut s = State::default();
    demo(&mut s);
    s.voice_command(Voice::Leave);
    assert!(s.voice.target.is_none());
    let guild = Target {
        guild: Some("100".into()),
        channel: "104".into(),
    };
    s.voice_command(Voice::Join(guild.clone()));
    assert!(s.voice.target.as_ref() == Some(&guild));
    assert_eq!(
        s.store.voice_members("104"),
        vec![("demo-self".into(), "You".into())]
    );
    s.voice_command(Voice::Mute(true));
    assert!(s.voice.mute);
    s.voice_command(Voice::Mute(false));
    assert!(!s.voice.mute);
    s.voice_command(Voice::Deafen(true));
    assert!(s.voice.deaf);
    s.voice_command(Voice::Deafen(false));
    assert!(!s.voice.deaf);
    for (id, kind) in [("200", 1), ("201", 3)] {
        s.store.channels.insert(
            id.into(),
            serde_json::from_value(serde_json::json!({"id":id,"type":kind})).unwrap(),
        );
        let dm = Target {
            guild: None,
            channel: id.into(),
        };
        s.voice_command(Voice::Join(dm.clone()));
        assert!(s.voice.target.as_ref() == Some(&dm));
        assert!(s.store.voice_members("104").is_empty());
        assert_eq!(s.store.voice_members(id).len(), 1);
    }
    s.voice_command(Voice::Join(Target {
        guild: None,
        channel: "missing".into(),
    }));
    assert!(!s.error.is_empty());
    assert_eq!(s.voice.target.as_ref().unwrap().channel, "201");
    s.voice.speaking.insert("demo-self".into());
    s.voice_command(Voice::Leave);
    assert!(s.voice.target.is_none());
    assert!(s.voice.speaking.is_empty());
    assert!(s.store.voice_members("201").is_empty());
    assert!(s.error.is_empty());
    assert!(s.network.is_none());
}
