use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Local, TimeZone, Utc};
use rand::RngCore;
use reqwest::StatusCode;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;
use url::Url;
use uuid::Uuid;

use crate::models::{clamp_percent, AccountUsage, ProviderCredential, UsageMetric};
use crate::store::{load_credential, save_credential};

const LOGIN_URL: &str = "https://cursor.com/loginDeepControl";
const API_BASE: &str = "https://api2.cursor.sh";
const CLIENT_ID: &str = "KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB";

fn cursor_user_id(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    let subject = value.get("sub")?.as_str()?;
    subject
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .find(|part| part.starts_with("user_"))
        .map(str::to_owned)
}

async fn profile(client: &reqwest::Client, access_token: &str) -> Option<Value> {
    let user_id = cursor_user_id(access_token)?;
    let response = client
        .get("https://cursor.com/api/auth/me")
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
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: Value = response.json().await.ok()?;
    (value.get("sub").and_then(Value::as_str) == Some(user_id.as_str())).then_some(value)
}

async fn refresh(
    client: &reqwest::Client,
    access_token: String,
    refresh_token: String,
) -> Result<(String, String), String> {
    let response = client.post(format!("{API_BASE}/oauth/token"))
        .json(&serde_json::json!({ "grant_type": "refresh_token", "client_id": CLIENT_ID, "refresh_token": refresh_token }))
        .send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Ok((access_token, refresh_token));
    }
    let body: Value = response.json().await.map_err(|error| error.to_string())?;
    if body.get("shouldLogout").and_then(Value::as_bool) == Some(true) {
        return Err("Cursor 로그인이 만료되었습니다. 다시 로그인하세요.".into());
    }
    Ok((
        body.get("access_token")
            .and_then(Value::as_str)
            .unwrap_or(&access_token)
            .into(),
        body.get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or(&refresh_token)
            .into(),
    ))
}

async fn request(
    client: &reqwest::Client,
    path: &str,
    access_token: &str,
) -> Result<Value, String> {
    let response = client
        .post(format!("{API_BASE}/{path}"))
        .bearer_auth(access_token)
        .header("Content-Type", "application/json")
        .header("Connect-Protocol-Version", "1")
        .body("{}")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("Cursor API 요청 실패 ({status})"));
    }
    response.json().await.map_err(|error| error.to_string())
}

fn reset_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    let date = if let Some(number) = value.as_f64() {
        let milliseconds = if number.abs() < 1_000_000_000_000.0 {
            number * 1000.0
        } else {
            number
        };
        Utc.timestamp_millis_opt(milliseconds as i64).single()
    } else if let Some(text) = value.as_str() {
        if let Ok(number) = text.trim().parse::<f64>() {
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
    Some(format!(
        "{} 초기화",
        date.with_timezone(&Local).format("%Y. %-m. %-d.")
    ))
}

fn usage_metric(
    id: &str,
    label: &str,
    value: Option<f64>,
    reset: &Option<String>,
) -> Option<UsageMetric> {
    let value = value?;
    Some(UsageMetric {
        id: id.into(),
        label: label.into(),
        used_percent: (clamp_percent(value) * 10.0).round() / 10.0,
        reset_text: reset.clone(),
        detail: None,
    })
}

pub async fn authenticate<R: Runtime>(app: &AppHandle<R>, account_id: &str) -> Result<(), String> {
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

    let client = reqwest::Client::new();
    let mut wait = 1000_u64;
    for _ in 0..150 {
        let response = client
            .get(format!("{API_BASE}/auth/poll"))
            .query(&[("uuid", &uuid), ("verifier", &verifier)])
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if response.status().is_success() {
            let body: Value = response.json().await.map_err(|error| error.to_string())?;
            let access_token = body
                .get("accessToken")
                .and_then(Value::as_str)
                .ok_or("Cursor 로그인 토큰을 받지 못했습니다.")?;
            let refresh_token = body
                .get("refreshToken")
                .and_then(Value::as_str)
                .ok_or("Cursor 로그인 토큰을 받지 못했습니다.")?;
            return save_credential(
                account_id,
                &ProviderCredential::Cursor {
                    access_token: access_token.into(),
                    refresh_token: refresh_token.into(),
                },
            );
        }
        if response.status() != StatusCode::NOT_FOUND {
            return Err(format!("Cursor 로그인 확인 실패 ({})", response.status()));
        }
        tokio::time::sleep(Duration::from_millis(wait)).await;
        wait = ((wait as f64 * 1.2).round() as u64).min(10_000);
    }
    Err("Cursor 로그인 시간이 초과되었습니다. 다시 시도하세요.".into())
}

pub async fn usage(account_id: &str) -> Result<AccountUsage, String> {
    let ProviderCredential::Cursor {
        access_token,
        refresh_token,
    } = load_credential(account_id)?
    else {
        return Err("저장된 Cursor 로그인 정보가 올바르지 않습니다.".into());
    };
    let client = reqwest::Client::new();
    let (access_token, refresh_token) = refresh(&client, access_token, refresh_token).await?;
    save_credential(
        account_id,
        &ProviderCredential::Cursor {
            access_token: access_token.clone(),
            refresh_token: refresh_token.clone(),
        },
    )?;
    let (usage, plan, profile) = tokio::join!(
        request(
            &client,
            "aiserver.v1.DashboardService/GetCurrentPeriodUsage",
            &access_token
        ),
        request(
            &client,
            "aiserver.v1.DashboardService/GetPlanInfo",
            &access_token
        ),
        profile(&client, &access_token),
    );
    let usage = usage?;
    let plan = plan.unwrap_or(Value::Null);
    let reset = reset_text(usage.get("billingCycleEnd"));
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
    let warning = metrics
        .is_empty()
        .then(|| "Cursor 사용량 항목을 받지 못했습니다.".into());
    Ok(AccountUsage {
        account_id: account_id.into(),
        email: profile.and_then(|value| {
            value
                .get("email")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }),
        plan: plan
            .pointer("/planInfo/planName")
            .or_else(|| plan.get("planName"))
            .and_then(Value::as_str)
            .map(str::to_uppercase),
        metrics,
        fetched_at: Utc::now().to_rfc3339(),
        source_url: format!("{API_BASE}/aiserver.v1.DashboardService/GetCurrentPeriodUsage"),
        warning,
    })
}
