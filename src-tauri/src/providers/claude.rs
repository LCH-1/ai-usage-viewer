use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::http;
use crate::command::{find_claude_executable, run_claude_login};
use crate::models::{
    clamp_percent, AccountUsage, ClaudeOauth, ProviderCredential, ProviderError, UsageMetric,
};
use crate::store::{
    data_directory, load_credential, load_snapshot, save_authenticated_credential, save_credential,
};

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const REQUEST_GAP: Duration = Duration::from_secs(2);
const PROFILE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const PROFILE_BUDGET: Duration = Duration::from_secs(6);

#[derive(Default)]
struct RequestPacer {
    last_started: Mutex<Option<Instant>>,
}

impl RequestPacer {
    async fn wait(&self) {
        loop {
            let mut last_started = self.last_started.lock().await;
            let now = Instant::now();
            let wait = last_started.and_then(|last| REQUEST_GAP.checked_sub(now - last));
            if let Some(wait) = wait.filter(|wait| !wait.is_zero()) {
                drop(last_started);
                tokio::time::sleep(wait).await;
            } else {
                *last_started = Some(now);
                return;
            }
        }
    }
}

static PACER: OnceLock<RequestPacer> = OnceLock::new();
static PROFILES: OnceLock<Mutex<HashMap<String, CachedProfile>>> = OnceLock::new();

#[derive(Clone, Default)]
struct CachedProfile {
    email: Option<String>,
    fetched: Option<Instant>,
    retry_at: Option<Instant>,
    failures: u32,
    warning: Option<String>,
}

impl CachedProfile {
    fn should_fetch(&self, now: Instant) -> bool {
        !self
            .fetched
            .is_some_and(|fetched| now - fetched < PROFILE_TTL)
            && !self.retry_at.is_some_and(|retry_at| now < retry_at)
    }

    fn failed(&mut self, error: &ProviderError) {
        self.failures = self.failures.saturating_add(1);
        let fallback = Duration::from_secs(300 * 2_u64.pow(self.failures.saturating_sub(1).min(4)));
        let retry_after = error
            .retry_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .and_then(|date| (date.with_timezone(&Utc) - Utc::now()).to_std().ok())
            .unwrap_or_default();
        self.retry_at = Some(Instant::now() + fallback.max(retry_after));
        self.warning = Some(
            "사용량은 갱신했지만 계정 프로필은 확인하지 못했습니다. 잠시 후 다시 확인합니다."
                .into(),
        );
    }
}

fn profiles() -> &'static Mutex<HashMap<String, CachedProfile>> {
    PROFILES.get_or_init(Mutex::default)
}

fn restore_profile(directory: &Path, account_id: &str, now: Instant) -> CachedProfile {
    let email = load_snapshot(directory, account_id)
        .ok()
        .and_then(|snapshot| snapshot.usage)
        .and_then(|usage| usage.email)
        .filter(|email| !email.trim().is_empty());
    CachedProfile {
        fetched: email.as_ref().map(|_| now),
        email,
        ..CachedProfile::default()
    }
}

pub async fn clear_profile_cache(account_id: &str) {
    profiles().lock().await.remove(account_id);
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: f64,
}

fn headers(access_token: &str) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    let mut authorization =
        HeaderValue::from_str(&format!("Bearer {access_token}")).map_err(|_| {
            ProviderError::auth_required(
                "저장된 Claude 로그인 정보가 올바르지 않습니다. 다시 로그인하세요.",
            )
        })?;
    authorization.set_sensitive(true);
    headers.insert(AUTHORIZATION, authorization);
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        "anthropic-beta",
        HeaderValue::from_static("oauth-2025-04-20"),
    );
    Ok(headers)
}

async fn request_json<T: DeserializeOwned>(
    url: &str,
    endpoint: &str,
    access_token: &str,
) -> Result<T, ProviderError> {
    let request = http::client()?.get(url).headers(headers(access_token)?);
    PACER.get_or_init(RequestPacer::default).wait().await;
    let response = request
        .send()
        .await
        .map_err(|error| http::transport_error(error, endpoint))?;
    if !response.status().is_success() {
        return Err(http::response_error(response, endpoint).await);
    }
    response
        .json()
        .await
        .map_err(|error| http::transport_error(error, endpoint))
}

async fn refresh_credential(
    credential: ClaudeOauth,
    force: bool,
) -> Result<(ClaudeOauth, bool), ProviderError> {
    if !force && credential.expires_at > Utc::now().timestamp_millis() as f64 + 60_000.0 {
        return Ok((credential, false));
    }
    let request = http::client()?
        .post(TOKEN_URL)
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": credential.refresh_token,
            "client_id": CLIENT_ID,
            "scope": "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload"
        }));
    PACER.get_or_init(RequestPacer::default).wait().await;
    let response = request
        .send()
        .await
        .map_err(|error| http::transport_error(error, "claude.token"))?;
    if !response.status().is_success() {
        return Err(http::response_error(response, "claude.token").await);
    }
    let body: TokenResponse = response
        .json()
        .await
        .map_err(|error| http::transport_error(error, "claude.token"))?;
    let refresh_token = body.refresh_token.unwrap_or(credential.refresh_token);
    if body.access_token.is_empty()
        || refresh_token.is_empty()
        || !body.expires_in.is_finite()
        || body.expires_in <= 0.0
    {
        let mut error = ProviderError::new(
            "invalidData",
            "Claude 로그인 갱신 응답이 올바르지 않습니다.",
        );
        error.endpoint = Some("claude.token".into());
        return Err(error);
    }
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

fn reset_at(value: Option<&str>) -> Option<String> {
    let date = DateTime::parse_from_rfc3339(value?).ok()?;
    Some(date.to_rfc3339())
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
        resets_at: reset_at(window.get("resets_at").and_then(Value::as_str)),
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
            resets_at: reset_at(limit.get("resets_at").and_then(Value::as_str)),
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

pub async fn authenticate(account_id: &str) -> Result<(), ProviderError> {
    let directory = tempfile::Builder::new()
        .prefix("usage-viewer-claude-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let executable = find_claude_executable().await?;
    run_claude_login(&executable, directory.path()).await?;
    let raw = tokio::fs::read_to_string(directory.path().join(".credentials.json"))
        .await
        .map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&raw).map_err(|_| {
        ProviderError::new("invalidData", "Claude 로그인 정보를 해석하지 못했습니다.")
    })?;
    let oauth: ClaudeOauth = serde_json::from_value(
        value
            .get("claudeAiOauth")
            .cloned()
            .ok_or("Claude 로그인 정보를 가져오지 못했습니다.")?,
    )
    .map_err(|_| {
        ProviderError::new(
            "invalidData",
            "Claude 로그인 정보 형식이 올바르지 않습니다.",
        )
    })?;
    if oauth.access_token.is_empty() || oauth.refresh_token.is_empty() {
        return Err("Claude 로그인 정보를 가져오지 못했습니다.".into());
    }
    clear_profile_cache(account_id).await;
    save_authenticated_credential(account_id, &ProviderCredential::Claude { oauth })
        .map_err(|_| ProviderError::new("storage", "Claude 로그인 정보를 저장하지 못했습니다."))
}

async fn profile(account_id: &str, access_token: &str) -> CachedProfile {
    let cached = {
        let profiles = profiles().lock().await;
        profiles.get(account_id).cloned()
    };
    let mut cached = match cached {
        Some(cached) => cached,
        None => {
            let restored = data_directory()
                .map(|directory| restore_profile(&directory, account_id, Instant::now()))
                .unwrap_or_default();
            profiles()
                .lock()
                .await
                .entry(account_id.into())
                .or_insert(restored)
                .clone()
        }
    };
    if !cached.should_fetch(Instant::now()) {
        return cached;
    }
    let result = tokio::time::timeout(
        PROFILE_BUDGET,
        request_json::<Value>(PROFILE_URL, "claude.profile", access_token),
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => {
            let mut error = ProviderError::temporary("Claude 프로필 확인 시간이 초과되었습니다.");
            error.endpoint = Some("claude.profile".into());
            Err(error)
        }
    };
    match result {
        Ok(body) => {
            if let Some(email) = body
                .pointer("/account/email")
                .and_then(Value::as_str)
                .filter(|email| !email.is_empty())
            {
                cached.email = Some(email.into());
                cached.fetched = Some(Instant::now());
                cached.retry_at = None;
                cached.failures = 0;
                cached.warning = None;
            } else {
                cached.failed(&ProviderError::new(
                    "invalidData",
                    "Claude 프로필에서 계정을 확인하지 못했습니다.",
                ));
            }
        }
        Err(error) => cached.failed(&error),
    }
    profiles()
        .lock()
        .await
        .insert(account_id.into(), cached.clone());
    cached
}

pub async fn usage(account_id: &str) -> Result<AccountUsage, ProviderError> {
    let ProviderCredential::Claude { oauth } = load_credential(account_id)? else {
        return Err(ProviderError::auth_required(
            "저장된 Claude 로그인 정보가 올바르지 않습니다. 다시 로그인하세요.",
        ));
    };
    let (mut oauth, changed) = refresh_credential(oauth, false).await?;
    if changed {
        save_credential(
            account_id,
            &ProviderCredential::Claude {
                oauth: oauth.clone(),
            },
        )?;
    }
    let usage: Value = match request_json(USAGE_URL, "claude.usage", &oauth.access_token).await {
        Err(error) if error.status == Some(401) => {
            (oauth, _) = refresh_credential(oauth, true).await?;
            save_credential(
                account_id,
                &ProviderCredential::Claude {
                    oauth: oauth.clone(),
                },
            )?;
            request_json(USAGE_URL, "claude.usage", &oauth.access_token).await?
        }
        result => result?,
    };
    let fetched_at = Utc::now().to_rfc3339();
    let metrics = parse_metrics(&usage);
    if metrics.is_empty() {
        let mut error = ProviderError::new(
            "invalidData",
            "Claude 응답에서 사용량 항목을 확인하지 못했습니다.",
        );
        error.endpoint = Some("claude.usage".into());
        return Err(error);
    }
    let profile = profile(account_id, &oauth.access_token).await;
    Ok(AccountUsage {
        account_id: account_id.into(),
        email: profile.email,
        plan: oauth.subscription_type.as_deref().map(str::to_uppercase),
        metrics,
        fetched_at,
        source_url: USAGE_URL.into(),
        warning: profile.warning,
        checked_at: None,
        next_retry_at: None,
        stale: false,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_real_usage_windows_and_preserves_reset_timestamp() {
        let metrics = parse_metrics(&json!({
            "five_hour": { "utilization": 21.37, "resets_at": "2026-09-07T15:00:00+09:00" },
            "seven_day": { "utilization": 110.0, "resets_at": null },
            "seven_day_opus": null
        }));
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].id, "five-hour");
        assert_eq!(metrics[0].used_percent, 21.4);
        assert_eq!(
            metrics[0].resets_at.as_deref(),
            Some("2026-09-07T15:00:00+09:00")
        );
        assert!(metrics[0].reset_text.is_some());
        assert_eq!(metrics[1].used_percent, 100.0);
        assert!(metrics[1].resets_at.is_none());
    }

    #[test]
    fn scoped_limits_replace_matching_legacy_window_and_ignore_invalid_windows() {
        let metrics = parse_metrics(&json!({
            "seven_day_sonnet": { "utilization": 10.0 },
            "five_hour": { "utilization": "unknown" },
            "limits": [
                { "kind": "weekly_scoped", "scope": { "model": { "display_name": "Sonnet" } }, "percent": 42.0, "resets_at": "invalid" },
                { "kind": "weekly_scoped", "scope": { "model": { "display_name": "Fable" } }, "utilization": 12.0 }
            ]
        }));
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].id, "sonnet");
        assert_eq!(metrics[0].used_percent, 42.0);
        assert!(metrics[0].resets_at.is_none());
        assert_eq!(metrics[1].id, "fable");
        assert!(parse_metrics(&json!({ "unexpected": true })).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn every_http_start_is_spaced_without_holding_the_pacer_lock() {
        let pacer = RequestPacer::default();
        let started = Instant::now();
        pacer.wait().await;
        assert!(pacer.last_started.try_lock().is_ok());
        let ((), ()) = tokio::join!(pacer.wait(), pacer.wait());
        assert_eq!(Instant::now() - started, REQUEST_GAP * 2);
        assert!(pacer.last_started.try_lock().is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_wait_does_not_reserve_a_future_http_slot() {
        let pacer = RequestPacer::default();
        pacer.wait().await;
        assert!(tokio::time::timeout(Duration::from_secs(1), pacer.wait())
            .await
            .is_err());
        let started = Instant::now();
        pacer.wait().await;
        assert_eq!(Instant::now() - started, Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn restored_identity_skips_profile_requests_for_six_hours_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("usage")).unwrap();
        std::fs::write(
            directory.path().join("usage").join("restored.json"),
            serde_json::to_vec(&json!({
                "usage": {
                    "accountId": "restored",
                    "email": "restored@example.invalid",
                    "metrics": [],
                    "fetchedAt": "2000-01-01T00:00:00Z",
                    "sourceUrl": USAGE_URL
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let cached = restore_profile(directory.path(), "restored", Instant::now());
        assert_eq!(cached.email.as_deref(), Some("restored@example.invalid"));
        assert!(!cached.should_fetch(Instant::now()));
        tokio::time::advance(PROFILE_TTL - Duration::from_secs(1)).await;
        assert!(!cached.should_fetch(Instant::now()));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(cached.should_fetch(Instant::now()));
        assert!(
            restore_profile(directory.path(), "different-account", Instant::now())
                .should_fetch(Instant::now())
        );
    }

    #[test]
    fn missing_damaged_or_empty_identity_cache_still_fetches_profile() {
        let directory = tempfile::tempdir().unwrap();
        let cache_directory = directory.path().join("usage");
        std::fs::create_dir(&cache_directory).unwrap();
        std::fs::write(cache_directory.join("damaged.json"), b"broken").unwrap();
        for (account_id, email) in [("empty", json!("  ")), ("unknown", Value::Null)] {
            std::fs::write(
                cache_directory.join(format!("{account_id}.json")),
                serde_json::to_vec(&json!({
                    "usage": {
                        "accountId": account_id,
                        "email": email,
                        "metrics": [],
                        "fetchedAt": "2026-09-08T00:00:00Z",
                        "sourceUrl": USAGE_URL
                    }
                }))
                .unwrap(),
            )
            .unwrap();
        }

        for account_id in ["missing", "damaged", "empty", "unknown"] {
            let cached = restore_profile(directory.path(), account_id, Instant::now());
            assert!(cached.email.is_none());
            assert!(cached.should_fetch(Instant::now()));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn profile_failures_keep_identity_and_obey_cooldown() {
        let mut profile = CachedProfile {
            email: Some("account@example.invalid".into()),
            fetched: Some(Instant::now()),
            ..CachedProfile::default()
        };
        assert!(!profile.should_fetch(Instant::now()));
        tokio::time::advance(PROFILE_TTL).await;
        assert!(profile.should_fetch(Instant::now()));
        profile.failed(&ProviderError::new("rateLimited", "제한"));
        assert_eq!(profile.email.as_deref(), Some("account@example.invalid"));
        assert!(!profile.should_fetch(Instant::now()));
        tokio::time::advance(Duration::from_secs(299)).await;
        assert!(!profile.should_fetch(Instant::now()));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(profile.should_fetch(Instant::now()));
    }
}
