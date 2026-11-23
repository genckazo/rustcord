use crate::{
    model::{Channel, Message},
    search::{self, Query},
};
use reqwest::{Client, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration;
use zeroize::Zeroizing;

pub struct Api {
    client: Client,
    token: Zeroizing<String>,
}
impl Api {
    pub fn new(token: Zeroizing<String>) -> Result<Self, String> {
        let client = Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| "Cannot initialize HTTPS".to_owned())?;
        Ok(Self { client, token })
    }
    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T, String> {
        self.request_with_query(method, path, &[], body).await
    }
    async fn request_with_query<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<T, String> {
        for _ in 0..8 {
            let mut request = self
                .client
                .request(method.clone(), format!("https://discord.com/api/v10{path}"))
                .header("Authorization", self.token.as_str());
            if !query.is_empty() {
                request = request.query(query);
            }
            if let Some(ref body) = body {
                request = request.json(body);
            }
            let response = request
                .send()
                .await
                .map_err(|_| "Discord request failed (network/TLS)".to_owned())?;
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                let data: Value = response
                    .json()
                    .await
                    .map_err(|_| "Invalid rate limit response")?;
                let seconds = data["retry_after"]
                    .as_f64()
                    .filter(|n| n.is_finite() && *n >= 0.0)
                    .ok_or("Invalid retry_after")?;
                tokio::time::sleep(Duration::from_secs_f64(seconds.max(0.05))).await;
                continue;
            }
            if !response.status().is_success() {
                return Err(format!("Discord HTTP {}", response.status().as_u16()));
            }
            if response.status() == StatusCode::NO_CONTENT {
                return serde_json::from_value(Value::Null)
                    .map_err(|_| "Invalid Discord response".into());
            }
            return response
                .json()
                .await
                .map_err(|_| "Invalid Discord response".into());
        }
        Err("Rate limited repeatedly; try again later".into())
    }
    pub async fn messages(
        &self,
        channel: &str,
        before: Option<&str>,
        after: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<Message>, String> {
        let path = messages_path(channel, before, after, limit)?;
        self.request(Method::GET, &path, None).await
    }
    pub async fn send(&self, channel: &str, content: &str) -> Result<Message, String> {
        valid_id(channel)?;
        self.request(
            Method::POST,
            &format!("/channels/{channel}/messages"),
            Some(json!({"content":content,"allowed_mentions":{"parse":[]}})),
        )
        .await
    }
    pub async fn search(
        &self,
        query: &Query,
        offset: usize,
    ) -> Result<(usize, Vec<Message>), String> {
        for id in query.ids() {
            valid_id(id)?;
        }
        for _ in 0..3 {
            let data: Value = self
                .request_with_query(Method::GET, &query.path(), &query.params(offset), None)
                .await?;
            match search::parse(data)? {
                search::Response::Page { total, messages } => return Ok((total, messages)),
                search::Response::Indexing { retry_after } => {
                    tokio::time::sleep(Duration::from_secs_f64(retry_after.clamp(0.5, 10.0))).await;
                }
            }
        }
        Err("Discord is still indexing these messages; try searching again shortly.".into())
    }
    pub async fn channels(&self, guild: &str) -> Result<Vec<Channel>, String> {
        valid_id(guild)?;
        self.request(Method::GET, &format!("/guilds/{guild}/channels"), None)
            .await
    }
    pub async fn typing(&self, channel: &str) -> Result<(), String> {
        valid_id(channel)?;
        self.request(Method::POST, &format!("/channels/{channel}/typing"), None)
            .await
    }
    pub async fn open_dm(&self, user: &str) -> Result<Channel, String> {
        self.request(Method::POST, "/users/@me/channels", Some(dm_body(user)?))
            .await
    }
}
fn dm_body(user: &str) -> Result<Value, String> {
    valid_id(user)?;
    Ok(json!({"recipients": [user]}))
}
fn messages_path(
    channel: &str,
    before: Option<&str>,
    after: Option<&str>,
    limit: Option<usize>,
) -> Result<String, String> {
    valid_id(channel)?;
    if before.is_some() && after.is_some() {
        return Err("Use before or after, not both".into());
    }
    let mut path = format!(
        "/channels/{channel}/messages?limit={}",
        limit.unwrap_or(crate::store::HISTORY_LIMIT).clamp(1, 100)
    );
    for (key, value) in [("before", before), ("after", after)] {
        if let Some(id) = value {
            valid_id(id)?;
            path.push_str(&format!("&{key}={id}"));
        }
    }
    Ok(path)
}
fn valid_id(id: &str) -> Result<(), String> {
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err("Invalid Discord ID".into())
    }
}

#[cfg(test)]
mod tests {
    use super::{messages_path, valid_id};
    #[test]
    fn history_queries() {
        assert_eq!(
            messages_path("1", None, None, None).unwrap(),
            "/channels/1/messages?limit=50"
        );
        assert_eq!(
            messages_path("1", Some("2"), None, Some(0)).unwrap(),
            "/channels/1/messages?limit=1&before=2"
        );
        assert_eq!(
            messages_path("1", None, Some("2"), Some(101)).unwrap(),
            "/channels/1/messages?limit=100&after=2"
        );
        assert_eq!(
            messages_path("1", None, None, Some(25)).unwrap(),
            "/channels/1/messages?limit=25"
        );
        assert!(messages_path("1", Some("2"), Some("3"), None).is_err());
        assert!(messages_path("", None, None, None).is_err());
        assert!(messages_path("1", Some(""), None, None).is_err());
        assert!(messages_path("1", None, Some("2&limit=999"), None).is_err());
    }

    #[test]
    fn create_dm_body() {
        assert_eq!(
            super::dm_body("123").unwrap(),
            serde_json::json!({"recipients":["123"]})
        );
        assert!(super::dm_body("").is_err());
        assert!(super::dm_body("../users").is_err());
    }
    #[test]
    fn ids_cannot_escape_discord_routes() {
        assert!(valid_id("123456789").is_ok());
        for id in ["", "../science", "123?redirect=evil", "https://example.com"] {
            assert!(valid_id(id).is_err());
        }
    }
}
