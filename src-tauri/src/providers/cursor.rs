use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, TimeZone, Utc};
use rand::RngCore;
use reqwest::StatusCode;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;
use url::Url;
use uuid::Uuid;

use crate::models::{clamp_percent, AccountUsage, ProviderCredential, ProviderError, UsageMetric};
use crate::providers::http::{client, response_error, transport_error};
use crate::store::{load_credential, save_authenticated_credential, save_credential};

const LOGIN_URL: &str = "https://cursor.com/loginDeepControl";
const API_BASE: &str = "https://api2.cursor.sh";
const CLIENT_ID: &str = "KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB";
const USAGE_PATH: &str = "aiserver.v1.DashboardService/GetCurrentPeriodUsage";
const PLAN_PATH: &str = "aiserver.v1.DashboardService/GetPlanInfo";
const PROFILE_URL: &str = "https://cursor.com/api/auth/me";
const TOKEN_ENDPOINT: &str = "cursor.token";
const METADATA_TIMEOUT: Duration = Duration::from_secs(3);
const METADATA_TTL: Duration = Duration::from_secs(15 * 60);
const METADATA_RETRY: Duration = Duration::from_secs(60);

#[derive(Clone, Default)]
struct MetadataField {
    value: Option<String>,
    checked_at: Option<Instant>,
    retry_until: Option<DateTime<Utc>>,
    failed: bool,
}

impl MetadataField {
    fn due(&self) -> bool {
        if self.retry_until.is_some_and(|until| until > Utc::now()) {
            return false;
        }
        let ttl = if self.failed {
            METADATA_RETRY
        } else {
            METADATA_TTL
        };
        self.checked_at
            .is_none_or(|checked| checked.elapsed() >= ttl)
    }

    fn update(&mut self, result: Result<String, ProviderError>) {
        self.checked_at = Some(Instant::now());
        self.failed = result.is_err();
        self.retry_until = result
            .as_ref()
            .err()
            .and_then(|error| error.retry_at.as_deref())
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|date| date.with_timezone(&Utc));
        if let Ok(value) = result {
            self.value = Some(value);
        }
    }
}

#[derive(Clone, Default)]
struct Metadata {
    email: MetadataField,
    plan: MetadataField,
}

fn metadata_cache() -> &'static Mutex<HashMap<String, Metadata>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Metadata>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn clear_metadata_cache(account_id: &str) {
    if let Ok(mut cache) = metadata_cache().lock() {
        cache.remove(account_id);
    }
}

fn token_payload(access_token: &str) -> Option<Value> {
    let payload = access_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn token_needs_refresh(access_token: &str, now: i64) -> bool {
    token_payload(access_token)
        .and_then(|payload| payload.get("exp").and_then(Value::as_i64))
        .is_some_and(|expires| expires <= now.saturating_add(60))
}

fn cursor_user_id(access_token: &str) -> Option<String> {
    let value = token_payload(access_token)?;
    let subject = value.get("sub")?.as_str()?;
    subject
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .find(|part| part.starts_with("user_"))
        .map(str::to_owned)
}

async fn profile(client: &reqwest::Client, access_token: &str) -> Result<String, ProviderError> {
    let user_id = cursor_user_id(access_token).ok_or_else(|| {
        ProviderError::new("invalidData", "Cursor 계정 식별 정보를 읽지 못했습니다.")
    })?;
    let response = client
        .get(PROFILE_URL)
        .timeout(METADATA_TIMEOUT)
        .header("Accept", "application/json")
        .header(
            "Cookie",
            format!(
                "WorkosCursorSessionToken={}",
                urlencoding::encode(&format!("{user_id}::{access_token}"))
            ),
        )
        .send()
        .await
        .map_err(|error| transport_error(error, PROFILE_URL))?;
    if !response.status().is_success() {
        return Err(response_error(response, PROFILE_URL).await);
    }
    let value: Value = response
        .json()
        .await
        .map_err(|error| transport_error(error, PROFILE_URL))?;
    if value.get("sub").and_then(Value::as_str) != Some(user_id.as_str()) {
        return Err(ProviderError::new(
            "invalidData",
            "Cursor 프로필의 계정이 일치하지 않습니다.",
        ));
    }
    value
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::new("invalidData", "Cursor 이메일을 받지 못했습니다."))
}

async fn refresh(
    client: &reqwest::Client,
    account_id: &str,
    refresh_token: &str,
) -> Result<(String, String), ProviderError> {
    let endpoint = format!("{API_BASE}/oauth/token");
    let response = client.post(&endpoint)
        .json(&serde_json::json!({ "grant_type": "refresh_token", "client_id": CLIENT_ID, "refresh_token": refresh_token }))
        .send().await.map_err(|error| transport_error(error, TOKEN_ENDPOINT))?;
    if !response.status().is_success() {
        return Err(response_error(response, TOKEN_ENDPOINT).await);
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| transport_error(error, TOKEN_ENDPOINT))?;
    let tokens = refreshed_tokens(&body, refresh_token)?;
    save_tokens(account_id, &tokens.0, &tokens.1)?;
    Ok(tokens)
}

fn refreshed_tokens(
    body: &Value,
    previous_refresh_token: &str,
) -> Result<(String, String), ProviderError> {
    if body.get("shouldLogout").and_then(Value::as_bool) == Some(true) {
        return Err(ProviderError::auth_required(
            "Cursor 로그인이 만료되었습니다. 다시 로그인하세요.",
        ));
    }
    Ok((
        body.get("access_token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ProviderError::new(
                    "invalidData",
                    "Cursor 갱신 토큰 응답에 액세스 토큰이 없습니다.",
                )
            })?
            .into(),
        body.get("refresh_token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(previous_refresh_token)
            .into(),
    ))
}

fn save_tokens(
    account_id: &str,
    access_token: &str,
    refresh_token: &str,
) -> Result<(), ProviderError> {
    save_credential(
        account_id,
        &ProviderCredential::Cursor {
            access_token: access_token.into(),
            refresh_token: refresh_token.into(),
        },
    )
    .map_err(|_| ProviderError::new("storage", "Cursor 로그인 정보를 저장하지 못했습니다."))
}

async fn request(
    client: &reqwest::Client,
    path: &str,
    access_token: &str,
    timeout: Duration,
) -> Result<Value, ProviderError> {
    let endpoint = format!("{API_BASE}/{path}");
    let response = client
        .post(&endpoint)
        .timeout(timeout)
        .bearer_auth(access_token)
        .header("Content-Type", "application/json")
        .header("Connect-Protocol-Version", "1")
        .body("{}")
        .send()
        .await
        .map_err(|error| transport_error(error, &endpoint))?;
    let status = response.status();
    if !status.is_success() {
        return Err(response_error(response, &endpoint).await);
    }
    response
        .json()
        .await
        .map_err(|error| transport_error(error, &endpoint))
}

fn reset_date(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?;
    let date = if let Some(number) = value.as_f64() {
        if !number.is_finite() {
            return None;
        }
        let milliseconds = if number.abs() < 1_000_000_000_000.0 {
            number * 1000.0
        } else {
            number
        };
        Utc.timestamp_millis_opt(milliseconds as i64).single()
    } else if let Some(text) = value.as_str() {
        if let Ok(number) = text.trim().parse::<f64>() {
            if !number.is_finite() {
                return None;
            }
            let milliseconds = if number.abs() < 1_000_000_000_000.0 {
                number * 1000.0
            } else {
                number
            };
            Utc.timestamp_millis_opt(milliseconds as i64).single()
        } else {
            DateTime::parse_from_rfc3339(text)
                .ok()
                .map(|date| date.with_timezone(&Utc))
        }
    } else {
        None
    }?;
    Some(date)
}

fn usage_metric(
    id: &str,
    label: &str,
    value: Option<f64>,
    reset: &Option<DateTime<Utc>>,
) -> Option<UsageMetric> {
    let value = value?;
    Some(UsageMetric {
        id: id.into(),
        label: label.into(),
        used_percent: (clamp_percent(value) * 10.0).round() / 10.0,
        reset_text: reset
            .as_ref()
            .map(|date| crate::models::format_local_reset(date.timestamp_millis())),
        resets_at: reset.as_ref().map(DateTime::to_rfc3339),
        detail: None,
    })
}

pub async fn authenticate<R: Runtime>(
    app: &AppHandle<R>,
    account_id: &str,
) -> Result<(), ProviderError> {
    let mut verifier_bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut verifier_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let uuid = Uuid::new_v4().to_string();
    let mut login = Url::parse(LOGIN_URL).map_err(|error| error.to_string())?;
    login
        .query_pairs_mut()
        .append_pair("challenge", &challenge)
        .append_pair("uuid", &uuid)
        .append_pair("mode", "login")
        .append_pair("redirectTarget", "cli");
    app.opener()
        .open_url(login.as_str(), None::<&str>)
        .map_err(|error| error.to_string())?;

    let client = client()?;
    let polling = async {
        let mut wait = 1000_u64;
        for _ in 0..150 {
            let response = client
                .get(format!("{API_BASE}/auth/poll"))
                .query(&[("uuid", &uuid), ("verifier", &verifier)])
                .send()
                .await
                .map_err(|error| transport_error(error, "Cursor 로그인 확인"))?;
            if response.status().is_success() {
                let body: Value = response
                    .json()
                    .await
                    .map_err(|error| transport_error(error, "Cursor 로그인 확인"))?;
                let access_token = body
                    .get("accessToken")
                    .and_then(Value::as_str)
                    .ok_or("Cursor 로그인 토큰을 받지 못했습니다.")?;
                let refresh_token = body
                    .get("refreshToken")
                    .and_then(Value::as_str)
                    .ok_or("Cursor 로그인 토큰을 받지 못했습니다.")?;
                clear_metadata_cache(account_id);
                return save_authenticated_credential(
                    account_id,
                    &ProviderCredential::Cursor {
                        access_token: access_token.into(),
                        refresh_token: refresh_token.into(),
                    },
                )
                .map_err(|_| {
                    ProviderError::new("storage", "Cursor 로그인 정보를 저장하지 못했습니다.")
                });
            }
            if response.status() != StatusCode::NOT_FOUND {
                return Err(response_error(response, "Cursor 로그인 확인").await);
            }
            tokio::time::sleep(Duration::from_millis(wait)).await;
            wait = ((wait as f64 * 1.2).round() as u64).min(10_000);
        }
        Err(ProviderError::temporary(
            "Cursor 로그인 시간이 초과되었습니다. 다시 시도하세요.",
        ))
    };
    tokio::time::timeout(Duration::from_secs(5 * 60), polling)
        .await
        .map_err(|_| {
            ProviderError::temporary("Cursor 로그인 시간이 초과되었습니다. 다시 시도하세요.")
        })?
}

pub async fn usage(account_id: &str) -> Result<AccountUsage, ProviderError> {
    let ProviderCredential::Cursor {
        mut access_token,
        mut refresh_token,
    } = load_credential(account_id)?
    else {
        return Err(ProviderError::auth_required(
            "저장된 Cursor 로그인 정보가 올바르지 않습니다.",
        ));
    };
    let client = client()?;
    if token_needs_refresh(&access_token, Utc::now().timestamp()) {
        (access_token, refresh_token) = refresh(&client, account_id, &refresh_token).await?;
    }
    let mut usage = request(&client, USAGE_PATH, &access_token, Duration::from_secs(20)).await;
    if usage
        .as_ref()
        .err()
        .is_some_and(|error| error.status == Some(401))
    {
        (access_token, _) = refresh(&client, account_id, &refresh_token).await?;
        usage = request(&client, USAGE_PATH, &access_token, Duration::from_secs(20)).await;
    }
    let usage = usage?;
    let metadata = metadata(&client, account_id, &access_token).await;
    parse_usage(account_id, &usage, metadata)
}

async fn metadata(client: &reqwest::Client, account_id: &str, access_token: &str) -> Metadata {
    let mut metadata = metadata_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(account_id).cloned())
        .unwrap_or_default();
    let (email, plan) = tokio::join!(
        async {
            if metadata.email.due() {
                Some(profile(client, access_token).await)
            } else {
                None
            }
        },
        async {
            if metadata.plan.due() {
                Some(
                    request(client, PLAN_PATH, access_token, METADATA_TIMEOUT)
                        .await
                        .and_then(|plan| {
                            plan.pointer("/planInfo/planName")
                                .or_else(|| plan.get("planName"))
                                .and_then(Value::as_str)
                                .map(str::to_uppercase)
                                .ok_or_else(|| {
                                    ProviderError::new(
                                        "invalidData",
                                        "Cursor 플랜을 받지 못했습니다.",
                                    )
                                })
                        }),
                )
            } else {
                None
            }
        },
    );
    if let Some(email) = email {
        metadata.email.update(email);
    }
    if let Some(plan) = plan {
        metadata.plan.update(plan);
    }
    if let Ok(mut cache) = metadata_cache().lock() {
        cache.insert(account_id.into(), metadata.clone());
    }
    metadata
}

fn parse_usage(
    account_id: &str,
    usage: &Value,
    metadata: Metadata,
) -> Result<AccountUsage, ProviderError> {
    let reset = reset_date(usage.get("billingCycleEnd"));
    let metrics = [
        usage_metric(
            "included",
            "포함 사용량",
            usage
                .pointer("/planUsage/totalPercentUsed")
                .and_then(Value::as_f64),
            &reset,
        ),
        usage_metric(
            "auto",
            "Auto",
            usage
                .pointer("/planUsage/autoPercentUsed")
                .and_then(Value::as_f64),
            &reset,
        ),
        usage_metric(
            "api",
            "API 모델",
            usage
                .pointer("/planUsage/apiPercentUsed")
                .and_then(Value::as_f64),
            &reset,
        ),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if metrics.is_empty() {
        return Err(ProviderError::new(
            "invalidData",
            "Cursor 사용량 항목을 받지 못했습니다.",
        ));
    }
    let warning = (metadata.email.failed || metadata.plan.failed)
        .then(|| "Cursor 사용량은 갱신했지만 계정 정보 일부는 확인하지 못했습니다. 이전 정보가 있으면 유지합니다.".into());
    Ok(AccountUsage {
        account_id: account_id.into(),
        email: metadata.email.value,
        plan: metadata.plan.value,
        metrics,
        fetched_at: Utc::now().to_rfc3339(),
        checked_at: None,
        next_retry_at: None,
        stale: false,
        error: None,
        source_url: format!("{API_BASE}/{USAGE_PATH}"),
        warning,
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use chrono::Utc;
    use serde_json::{json, Value};

    use super::{
        cursor_user_id, parse_usage, refreshed_tokens, reset_date, token_needs_refresh, Metadata,
        MetadataField,
    };
    use crate::models::ProviderError;

    fn token(payload: Value) -> String {
        format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(payload.to_string())
        )
    }

    #[test]
    fn valid_tokens_skip_refresh_until_expiry_margin() {
        let access = token(json!({"exp": 2_000, "sub": "auth0|user_sample"}));
        assert!(!token_needs_refresh(&access, 1_000));
        assert!(token_needs_refresh(&access, 1_940));
        assert!(token_needs_refresh(&access, 2_001));
        assert_eq!(cursor_user_id(&access).as_deref(), Some("user_sample"));
    }

    #[test]
    fn opaque_or_missing_expiry_tokens_wait_for_unauthorized_response() {
        assert!(!token_needs_refresh("opaque-token", 2_000));
        assert!(!token_needs_refresh(
            &token(json!({"sub": "user_sample"})),
            2_000
        ));
    }

    #[test]
    fn refreshed_tokens_require_access_token_and_preserve_unrotated_refresh_token() {
        assert_eq!(
            refreshed_tokens(&json!({"access_token": "new-access"}), "existing-refresh").unwrap(),
            ("new-access".into(), "existing-refresh".into())
        );
        assert_eq!(
            refreshed_tokens(&json!({}), "existing-refresh")
                .unwrap_err()
                .code,
            "invalidData"
        );
        assert_eq!(
            refreshed_tokens(&json!({"shouldLogout": true}), "existing-refresh")
                .unwrap_err()
                .code,
            "authRequired"
        );
    }

    #[test]
    fn reset_formats_agree_and_keep_time() {
        assert!(reset_date(Some(&json!("NaN"))).is_none());
        let seconds = reset_date(Some(&json!(1_800_000_000))).unwrap();
        assert_eq!(
            Some(seconds),
            reset_date(Some(&json!(1_800_000_000_000_i64)))
        );
        assert_eq!(Some(seconds), reset_date(Some(&json!("1800000000"))));
        assert_eq!(
            Some(seconds),
            reset_date(Some(&json!(seconds.to_rfc3339())))
        );
        let usage = parse_usage(
            "test",
            &json!({"billingCycleEnd": 1_800_000_000,
            "planUsage": {"totalPercentUsed": 12.25, "autoPercentUsed": 0, "apiPercentUsed": 20}}),
            Metadata::default(),
        )
        .unwrap();
        assert_eq!(usage.metrics.len(), 3);
        assert_eq!(usage.metrics[0].used_percent, 12.3);
        assert!(usage.metrics[0].reset_text.as_ref().unwrap().contains(':'));
        assert_eq!(
            usage.metrics[0].resets_at.as_deref(),
            Some(seconds.to_rfc3339().as_str())
        );
    }

    #[test]
    fn partial_metadata_failure_retains_last_known_value_and_waits_before_retry() {
        let mut field = MetadataField::default();
        assert!(field.due());
        field.update(Ok("PRO".into()));
        field.update(Err(ProviderError::temporary("unavailable")));
        assert_eq!(field.value.as_deref(), Some("PRO"));
        assert!(field.failed);
        assert!(!field.due());
        let usage = parse_usage(
            "test",
            &json!({"planUsage": {"totalPercentUsed": 5}}),
            Metadata {
                plan: field,
                email: MetadataField::default(),
            },
        )
        .unwrap();
        assert_eq!(usage.plan.as_deref(), Some("PRO"));
        assert!(usage.warning.is_some());
        assert_eq!(
            parse_usage("test", &json!({}), Metadata::default())
                .unwrap_err()
                .code,
            "invalidData"
        );
    }

    #[test]
    fn metadata_retry_waits_for_server_deadline_and_at_least_one_minute() {
        let mut field = MetadataField::default();
        let mut limited = ProviderError::new("rateLimited", "limited");
        let deadline = Utc::now() + chrono::Duration::hours(1);
        limited.retry_at = Some(deadline.to_rfc3339());
        field.update(Err(limited));
        field.checked_at = Some(Instant::now() - Duration::from_secs(61));
        assert_eq!(field.retry_until, Some(deadline));
        assert!(!field.due());
        field.retry_until = Some(Utc::now() - chrono::Duration::seconds(1));
        assert!(field.due());

        let mut limited = ProviderError::new("rateLimited", "limited");
        limited.retry_at = Some((Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
        field.update(Err(limited));
        assert!(!field.due());
        field.update(Ok("PRO".into()));
        assert!(field.retry_until.is_none());
        assert!(!field.failed);
    }
}
