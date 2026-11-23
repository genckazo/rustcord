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
    #[serde(default)]
    pub voice_states: Vec<VoiceState>,
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
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub author: User,
    #[serde(default)]
    pub content: String,
    pub timestamp: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VoiceState {
    pub user_id: String,
    pub channel_id: Option<String>,
    pub guild_id: Option<String>,
    #[serde(default)]
    pub member: Option<VoiceMember>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct VoiceMember {
    pub user: Option<User>,
    pub nick: Option<String>,
}
