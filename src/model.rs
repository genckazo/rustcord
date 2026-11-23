use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct User {
    pub id: String,
    #[serde(default)]
    pub username: String,
    pub global_name: Option<String>,
}
impl User {
    pub fn name(&self) -> &str {
        self.global_name.as_deref().unwrap_or(&self.username)
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct Guild {
    pub id: String,
    #[serde(default = "unknown")]
    pub name: String,
    #[serde(default)]
    pub channels: Vec<Channel>,
}
fn unknown() -> String {
    "Loading server…".into()
}
#[derive(Clone, Debug, Deserialize)]
pub struct Channel {
    pub id: String,
    pub guild_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(default)]
    pub position: i32,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub recipients: Vec<User>,
}
impl Channel {
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            self.recipients
                .iter()
                .map(|u| u.name().to_owned())
                .collect::<Vec<_>>()
                .join(", ")
        })
    }
    pub fn is_text(&self) -> bool {
        matches!(self.kind, 0 | 1 | 3 | 5)
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct Attachment {
    #[serde(default)]
    pub filename: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub author: User,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub timestamp: String,
}
impl Message {
    pub fn body(&self) -> String {
        if self.content.is_empty() && !self.attachments.is_empty() {
            match self.attachments.as_slice() {
                [a] => format!("[attachment: {}]", a.filename),
                a => format!("[{} attachments]", a.len()),
            }
        } else {
            self.content.clone()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn message(content: &str, attachments: Vec<Attachment>) -> Message {
        Message {
            id: "1".into(),
            channel_id: "1".into(),
            author: User::default(),
            content: content.into(),
            attachments,
            timestamp: "now".into(),
        }
    }
    fn attachment(filename: &str) -> Attachment {
        Attachment {
            filename: filename.into(),
        }
    }
    #[test]
    fn body_shows_placeholder_for_attachment_only_messages() {
        assert_eq!(
            message("", vec![attachment("a.png")]).body(),
            "[attachment: a.png]"
        );
        assert_eq!(
            message("", vec![attachment("a.png"), attachment("b.png")]).body(),
            "[2 attachments]"
        );
        assert_eq!(message("hi", vec![]).body(), "hi");
        assert_eq!(message("look", vec![attachment("a.png")]).body(), "look");
    }
    #[test]
    fn attachments_deserialize_from_discord_payload() {
        let m: Message = serde_json::from_value(serde_json::json!({
            "id": "9",
            "channel_id": "1",
            "author": {"id": "2", "username": "River"},
            "timestamp": "2026-11-23T10:00:00Z",
            "attachments": [{"filename": "sketch.png", "url": "https://cdn.discordapp.com/x"}]
        }))
        .unwrap();
        assert_eq!(m.body(), "[attachment: sketch.png]");
    }
}
