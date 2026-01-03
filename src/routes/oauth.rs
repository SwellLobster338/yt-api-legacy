use reqwest::Client;
use serde_json::Value;

use crate::routes::auth::AuthConfig;

pub async fn refresh_access_token(
    refresh_token: &str,
    auth_config: &AuthConfig,
) -> Result<String, String> {
    let client = Client::new();
    let params = [
        ("client_id", auth_config.client_id.as_str()),
        ("client_secret", auth_config.client_secret.as_str()),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];

    let res = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Token refresh failed: {}", res.status()));
    }

    let json: Value = res.json().await.map_err(|e| e.to_string())?;
    if let Some(access) = json.get("access_token").and_then(|t| t.as_str()) {
        Ok(access.to_string())
    } else {
        Err("No access_token in response".to_string())
    }
}
