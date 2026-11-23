use crate::model::Message;
use serde_json::Value;
use std::collections::HashSet;

pub const PAGE_SIZE: usize = 25;
pub const MAX_OFFSET: usize = 9975;
pub const MAX_CONTENT_CHARS: usize = 1024;
const INDEXING_CODE: u64 = 110000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Guild {
        guild: String,
        channel: Option<String>,
    },
    Channel(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub scope: Scope,
    pub content: String,
}
impl Query {
    pub fn new(scope: Scope, content: &str) -> Result<Self, String> {
        let content = content.trim();
        if content.is_empty() {
            return Err("Type something to search for.".into());
        }
        if content.chars().count() > MAX_CONTENT_CHARS {
            return Err(format!(
                "Searches must be {MAX_CONTENT_CHARS} characters or fewer."
            ));
        }
        Ok(Self {
            scope,
            content: content.to_owned(),
        })
    }
    pub fn path(&self) -> String {
        match &self.scope {
            Scope::Guild { guild, .. } => format!("/guilds/{guild}/messages/search"),
            Scope::Channel(channel) => format!("/channels/{channel}/messages/search"),
        }
    }
    pub fn ids(&self) -> Vec<&str> {
        match &self.scope {
            Scope::Guild { guild, channel } => std::iter::once(guild.as_str())
                .chain(channel.as_deref())
                .collect(),
            Scope::Channel(channel) => vec![channel.as_str()],
        }
    }
    pub fn params(&self, offset: usize) -> Vec<(&'static str, String)> {
        let mut params = vec![("content", self.content.clone())];
        if let Scope::Guild {
            channel: Some(channel),
            ..
        } = &self.scope
        {
            params.push(("channel_id", channel.clone()));
        }
        if offset > 0 {
            params.push(("offset", offset.min(MAX_OFFSET).to_string()));
        }
        params
    }
    pub fn matches(&self, message: &Message) -> bool {
        let needle = self.content.to_lowercase();
        message.content.to_lowercase().contains(&needle)
            && match &self.scope {
                Scope::Guild {
                    channel: Some(channel),
                    ..
                }
                | Scope::Channel(channel) => &message.channel_id == channel,
                Scope::Guild { channel: None, .. } => true,
            }
    }
}

#[derive(Debug)]
pub enum Response {
    Page {
        total: usize,
        messages: Vec<Message>,
    },
    Indexing {
        retry_after: f64,
    },
}
/// Discord returns hits as groups of context messages; the hit is flagged
/// with `"hit": true` (newer responses send single-message groups).
pub fn parse(data: Value) -> Result<Response, String> {
    if data["code"].as_u64() == Some(INDEXING_CODE) || data.get("documents_indexed").is_some() {
        let retry_after = data["retry_after"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .unwrap_or(2.0);
        return Ok(Response::Indexing { retry_after });
    }
    let groups = data["messages"]
        .as_array()
        .ok_or("Invalid search response")?;
    let total = data["total_results"].as_u64().unwrap_or(0) as usize;
    let messages = groups
        .iter()
        .filter_map(|group| match group {
            Value::Array(items) => items
                .iter()
                .find(|m| m["hit"].as_bool() == Some(true))
                .or_else(|| items.first()),
            Value::Object(_) => Some(group),
            _ => None,
        })
        .filter_map(|m| serde_json::from_value::<Message>(m.clone()).ok())
        .collect();
    Ok(Response::Page { total, messages })
}

pub struct Search {
    pub query: Query,
    pub results: Vec<Message>,
    pub total: usize,
    pub next_offset: usize,
    pub loading: bool,
    pub error: Option<String>,
}
impl Search {
    pub fn new(query: Query) -> Self {
        Self {
            query,
            results: Vec::new(),
            total: 0,
            next_offset: 0,
            loading: true,
            error: None,
        }
    }
    pub fn apply(&mut self, offset: usize, result: Result<(usize, Vec<Message>), String>) {
        if offset != self.next_offset {
            return;
        }
        self.loading = false;
        match result {
            Ok((total, messages)) => {
                let mut seen: HashSet<String> = self.results.iter().map(|m| m.id.clone()).collect();
                self.results
                    .extend(messages.into_iter().filter(|m| seen.insert(m.id.clone())));
                self.total = total.max(self.results.len());
                self.next_offset = offset + PAGE_SIZE;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn has_more(&self) -> bool {
        !self.loading
            && self.error.is_none()
            && self.results.len() < self.total
            && self.next_offset <= MAX_OFFSET
    }
    pub fn summary(&self) -> String {
        if self.loading && self.results.is_empty() {
            return format!("Searching for “{}”…", self.query.content);
        }
        match self.total {
            0 => format!("No results for “{}”", self.query.content),
            1 => format!("1 result for “{}”", self.query.content),
            n => format!("{n} results for “{}”", self.query.content),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn guild(channel: Option<&str>) -> Scope {
        Scope::Guild {
            guild: "1".into(),
            channel: channel.map(Into::into),
        }
    }
    fn message(id: &str, channel: &str, content: &str) -> Message {
        serde_json::from_value(json!({"id":id,"channel_id":channel,"author":{"id":"9","username":"Ada"},"content":content,"timestamp":"now"})).unwrap()
    }

    #[test]
    fn query_validation_and_params() {
        assert!(Query::new(guild(None), "   ").is_err());
        assert!(Query::new(guild(None), &"x".repeat(1025)).is_err());
        let q = Query::new(guild(Some("5")), "  hello world ").unwrap();
        assert_eq!(q.content, "hello world");
        assert_eq!(q.path(), "/guilds/1/messages/search");
        assert_eq!(q.ids(), ["1", "5"]);
        assert_eq!(
            q.params(0),
            [
                ("content", "hello world".into()),
                ("channel_id", "5".into())
            ]
        );
        assert_eq!(q.params(50_000).last().unwrap(), &("offset", "9975".into()));
        let dm = Query::new(Scope::Channel("7".into()), "hi").unwrap();
        assert_eq!(dm.path(), "/channels/7/messages/search");
        assert_eq!(
            dm.params(25),
            [("content", "hi".into()), ("offset", "25".into())]
        );
    }

    #[test]
    fn parses_hits_from_context_groups() {
        let data = json!({"total_results": 3, "messages": [
            [
                {"id":"1","channel_id":"5","author":{"id":"9"},"content":"before","timestamp":"t"},
                {"id":"2","channel_id":"5","author":{"id":"9"},"content":"the hit","timestamp":"t","hit":true}
            ],
            [{"id":"3","channel_id":"6","author":{"id":"9"},"content":"solo","timestamp":"t"}],
            [{"broken":true}],
            42
        ]});
        let Ok(Response::Page { total, messages }) = parse(data) else {
            panic!("expected page");
        };
        assert_eq!(total, 3);
        let ids: Vec<_> = messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["2", "3"]);
        assert!(parse(json!({"total_results": 1})).is_err());
    }

    #[test]
    fn detects_index_not_ready() {
        let data = json!({"message":"Index not yet available. Try again later","code":110000,"documents_indexed":0,"retry_after":3.5});
        assert!(
            matches!(parse(data), Ok(Response::Indexing { retry_after }) if retry_after == 3.5)
        );
        assert!(matches!(
            parse(json!({"documents_indexed": 0, "retry_after": -1})),
            Ok(Response::Indexing { retry_after }) if retry_after == 2.0
        ));
    }

    #[test]
    fn pages_accumulate_dedupe_and_ignore_stale_offsets() {
        let q = Query::new(guild(None), "rust").unwrap();
        let mut s = Search::new(q);
        assert_eq!(s.summary(), "Searching for “rust”…");
        s.apply(25, Ok((99, vec![message("9", "5", "stale")])));
        assert!(s.results.is_empty() && s.loading);
        s.apply(
            0,
            Ok((30, vec![message("1", "5", "a"), message("2", "5", "b")])),
        );
        assert_eq!(s.results.len(), 2);
        assert!(s.has_more());
        assert_eq!(s.summary(), "30 results for “rust”");
        s.loading = true;
        s.apply(
            25,
            Ok((30, vec![message("2", "5", "b"), message("3", "5", "c")])),
        );
        assert_eq!(s.results.len(), 3);
        assert_eq!(s.next_offset, 50);
        s.loading = true;
        s.apply(50, Err("Discord HTTP 500".into()));
        assert_eq!(s.error.as_deref(), Some("Discord HTTP 500"));
        assert!(!s.has_more());
        let mut empty = Search::new(Query::new(guild(None), "none").unwrap());
        empty.apply(0, Ok((0, vec![])));
        assert_eq!(empty.summary(), "No results for “none”");
        assert!(!empty.has_more());
    }

    #[test]
    fn local_matching_respects_scope() {
        let m = message("1", "5", "Hello Rust");
        assert!(Query::new(guild(None), "rust").unwrap().matches(&m));
        assert!(Query::new(guild(Some("5")), "RUST").unwrap().matches(&m));
        assert!(!Query::new(guild(Some("6")), "rust").unwrap().matches(&m));
        assert!(!Query::new(Scope::Channel("5".into()), "go")
            .unwrap()
            .matches(&m));
    }
}
