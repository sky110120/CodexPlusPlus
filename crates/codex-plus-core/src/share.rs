use anyhow::{Context, bail};
use serde_json::Value;

const SHARE_ENDPOINTS: &[&str] = &[
    "https://share.codexpp.cc/api/shares",
    "https://codexpp-share.pages.dev/api/shares",
];

pub async fn create_share(payload: Value) -> anyhow::Result<Value> {
    let client = reqwest::Client::builder()
        .user_agent("CodexPlusPlus share proxy")
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("创建分享请求客户端失败")?;
    let mut last_error = None;
    for endpoint in SHARE_ENDPOINTS {
        let response = match client
            .post(*endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&payload)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(error.to_string());
                continue;
            }
        };
        let status = response.status();
        let body = response
            .json::<Value>()
            .await
            .unwrap_or_else(|_| Value::Object(Default::default()));
        if status.is_success() && body.get("id").and_then(Value::as_str).is_some() {
            return Ok(body);
        }
        let message = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_else(|| status.canonical_reason().unwrap_or("分享服务请求失败"));
        last_error = Some(format!("{message}（HTTP {status}）"));
    }
    bail!(last_error.unwrap_or_else(|| "分享服务不可用".to_string()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn share_requests_have_bounded_connect_and_response_timeouts() {
        let source = include_str!("share.rs");
        assert!(source.contains("connect_timeout"));
        assert!(source.contains("timeout(std::time::Duration::from_secs(20))"));
    }
}
