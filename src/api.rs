use crate::model::{Channel, Message};
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
        for _ in 0..8 {
            let mut request = self
                .client
                .request(method.clone(), format!("https://discord.com/api/v10{path}"))
                .header("Authorization", self.token.as_str());
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
    ) -> Result<Vec<Message>, String> {
        valid_id(channel)?;
        if let Some(id) = before {
            valid_id(id)?;
        }
        let suffix = before.map(|id| format!("&before={id}")).unwrap_or_default();
        self.request(
            Method::GET,
            &format!("/channels/{channel}/messages?limit=50{suffix}"),
            None,
        )
        .await
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
    use super::valid_id;
    #[test]
    fn ids_cannot_escape_discord_routes() {
        assert!(valid_id("123456789").is_ok());
        for id in ["", "../science", "123?redirect=evil", "https://example.com"] {
            assert!(valid_id(id).is_err());
        }
    }
}
