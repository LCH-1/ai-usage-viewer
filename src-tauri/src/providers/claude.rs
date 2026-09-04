use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;

use crate::command::{find_claude_executable, run_claude_login};
use crate::models::{clamp_percent, AccountUsage, ClaudeOauth, ProviderCredential, UsageMetric};
use crate::store::{load_credential, save_credential};

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: f64,
}

fn headers(access_token: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    let authorization = HeaderValue::from_str(&format!("Bearer {access_token}"))
        .map_err(|error| error.to_string())?;
    headers.insert(AUTHORIZATION, authorization);
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        "anthropic-beta",
        HeaderValue::from_static("oauth-2025-04-20"),
    );
    Ok(headers)
}

async fn request_json<T: DeserializeOwned>(url: &str, access_token: &str) -> Result<T, String> {
    let response = reqwest::Client::new()
        .get(url)
        .headers(headers(access_token)?)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("Claude API 요청 실패 ({status})"));
    }
    response.json().await.map_err(|error| error.to_string())
}

async fn refresh_credential(credential: ClaudeOauth) -> Result<(ClaudeOauth, bool), String> {
    if credential.expires_at > Utc::now().timestamp_millis() as f64 + 60_000.0 {
        return Ok((credential, false));
    }
    let response = reqwest::Client::new()
        .post(TOKEN_URL)
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": credential.refresh_token,
            "client_id": CLIENT_ID,
            "scope": "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload"
        }))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err("Claude 로그인을 갱신하지 못했습니다. 다시 로그인하세요.".into());
    }
    let body: TokenResponse = response.json().await.map_err(|error| error.to_string())?;
    let refresh_token = body.refresh_token.unwrap_or(credential.refresh_token);
    Ok((
        ClaudeOauth {
            access_token: body.access_token,
            refresh_token,
            expires_at: Utc::now().timestamp_millis() as f64 + body.expires_in * 1000.0,
            scopes: credential.scopes,
            subscription_type: credential.subscription_type,
            rate_limit_tier: credential.rate_limit_tier,
        },
        true,
    ))
}

fn reset_text(value: Option<&str>) -> Option<String> {
    let date = DateTime::parse_from_rfc3339(value?).ok()?;
    Some(crate::models::format_local_reset(date.timestamp_millis()))
}

fn metric(id: &str, label: &str, window: Option<&Value>) -> Option<UsageMetric> {
    let window = window?;
    let used_percent = window.get("utilization").and_then(Value::as_f64)?;
    Some(UsageMetric {
        id: id.into(),
        label: label.into(),
        used_percent: (clamp_percent(used_percent) * 10.0).round() / 10.0,
        reset_text: reset_text(window.get("resets_at").and_then(Value::as_str)),
        detail: None,
    })
}

fn model_metric_id(display_name: &str) -> String {
    let normalized = display_name.trim().to_lowercase();
    if normalized.contains("sonnet") {
        return "sonnet".into();
    }
    if normalized.contains("opus") {
        return "opus".into();
    }
    if normalized.contains("fable") {
        return "fable".into();
    }
    let slug = normalized
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("model-{}", slug.trim_matches('-'))
}

fn parse_metrics(payload: &Value) -> Vec<UsageMetric> {
    let mut metrics = [
        metric("five-hour", "5시간", payload.get("five_hour")),
        metric("weekly", "주간", payload.get("seven_day")),
        metric("sonnet", "Sonnet 주간", payload.get("seven_day_sonnet")),
        metric("opus", "Opus 주간", payload.get("seven_day_opus")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    for limit in payload
        .get("limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = limit.get("kind").and_then(Value::as_str);
        let group = limit.get("group").and_then(Value::as_str);
        if kind != Some("weekly_scoped") && group != Some("weekly") {
            continue;
        }
        let Some(display_name) = limit
            .pointer("/scope/model/display_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let Some(percent) = limit
            .get("percent")
            .or_else(|| limit.get("utilization"))
            .and_then(Value::as_f64)
        else {
            continue;
        };
        let item = UsageMetric {
            id: model_metric_id(display_name),
            label: format!("{display_name} 주간"),
            used_percent: (clamp_percent(percent) * 10.0).round() / 10.0,
            reset_text: reset_text(limit.get("resets_at").and_then(Value::as_str)),
            detail: None,
        };
        if let Some(index) = metrics.iter().position(|metric| metric.id == item.id) {
            metrics[index] = item;
        } else {
            metrics.push(item);
        }
    }
    metrics
}

pub async fn authenticate(account_id: &str) -> Result<(), String> {
    let directory = tempfile::Builder::new()
        .prefix("usage-viewer-claude-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let executable = find_claude_executable().await?;
    run_claude_login(&executable, directory.path()).await?;
    let raw = tokio::fs::read_to_string(directory.path().join(".credentials.json"))
        .await
        .map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
    let oauth: ClaudeOauth = serde_json::from_value(
        value
            .get("claudeAiOauth")
            .cloned()
            .ok_or("Claude 로그인 정보를 가져오지 못했습니다.")?,
    )
    .map_err(|error| error.to_string())?;
    if oauth.access_token.is_empty() || oauth.refresh_token.is_empty() {
        return Err("Claude 로그인 정보를 가져오지 못했습니다.".into());
    }
    save_credential(account_id, &ProviderCredential::Claude { oauth })
}

pub async fn usage(account_id: &str) -> Result<AccountUsage, String> {
    let ProviderCredential::Claude { oauth } = load_credential(account_id)? else {
        return Err("저장된 Claude 로그인 정보가 올바르지 않습니다.".into());
    };
    let (oauth, changed) = refresh_credential(oauth).await?;
    if changed {
        save_credential(
            account_id,
            &ProviderCredential::Claude {
                oauth: oauth.clone(),
            },
        )?;
    }
    let usage: Value = request_json(USAGE_URL, &oauth.access_token).await?;
    let profile: Value = request_json(PROFILE_URL, &oauth.access_token)
        .await
        .unwrap_or(Value::Null);
    let metrics = parse_metrics(&usage);
    let warning = metrics
        .is_empty()
        .then(|| "Claude 사용량 항목을 받지 못했습니다.".into());
    Ok(AccountUsage {
        account_id: account_id.into(),
        email: profile
            .pointer("/account/email")
            .and_then(Value::as_str)
            .map(str::to_owned),
        plan: oauth.subscription_type.as_deref().map(str::to_uppercase),
        metrics,
        fetched_at: Utc::now().to_rfc3339(),
        source_url: USAGE_URL.into(),
        warning,
    })
}
