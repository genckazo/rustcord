use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct User {
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
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub author: User,
    #[serde(default)]
    pub content: String,
    pub timestamp: String,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Embed {
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub color: Option<u32>,
    pub author: Option<EmbedAuthor>,
    pub footer: Option<EmbedFooter>,
    #[serde(default)]
    pub fields: Vec<EmbedField>,
    pub image: Option<EmbedImage>,
    pub thumbnail: Option<EmbedImage>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EmbedAuthor {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EmbedFooter {
    pub text: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EmbedImage {
    pub url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub description: Option<String>,
    pub content_type: Option<String>,
    pub size: u64,
    pub url: String,
    pub proxy_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_decode_media_without_requiring_text_or_media_fields() {
        let mut data = json!({
            "id":"1","channel_id":"2","author":{"id":"3"},"timestamp":"2026-11-23T10:00:00Z"
        });
        let plain: Message = serde_json::from_value(data.clone()).unwrap();
        assert!(plain.content.is_empty());
        assert!(plain.embeds.is_empty());
        assert!(plain.attachments.is_empty());

        data["embeds"] = json!([
            {"type":"rich","title":"Release notes","fields":[{"name":"Version","value":"1.0"}]},
            {"type":"image","thumbnail":{"url":"https://example.com/image.png"}}
        ]);
        data["attachments"] = json!([{
            "id":"4","filename":"notes.txt","size":1024,
            "url":"https://cdn.discordapp.com/attachments/2/4/notes.txt"
        }]);
        let rich: Message = serde_json::from_value(data).unwrap();
        assert_eq!(rich.embeds[0].title.as_deref(), Some("Release notes"));
        assert_eq!(rich.embeds[0].fields[0].value, "1.0");
        assert!(rich.embeds[1].thumbnail.is_some());
        assert_eq!(rich.attachments[0].filename, "notes.txt");
        assert!(rich.attachments[0].content_type.is_none());
    }
}
