use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::header::RETRY_AFTER;
use serde_json::Value;

use crate::models::ProviderError;

static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

pub fn client() -> Result<reqwest::Client, ProviderError> {
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .read_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(20))
                .build()
                .map_err(|_| "서버 연결 설정을 준비하지 못했습니다.".to_owned())
        })
        .clone()
        .map_err(ProviderError::temporary)
}

pub fn transport_error(error: reqwest::Error, endpoint: &str) -> ProviderError {
    let mut result = if error.is_decode() {
        ProviderError::new("invalidData", "서버 응답을 해석하지 못했습니다.")
    } else if error.is_timeout() {
        ProviderError::temporary("서버 응답 시간이 초과되었습니다. 잠시 후 다시 확인합니다.")
    } else {
        ProviderError::temporary("서버에 연결하지 못했습니다. 잠시 후 다시 확인합니다.")
    };
    result.endpoint = Some(endpoint.into());
    result
}

fn retry_after(value: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<i64>() {
        if seconds < 0 {
            return None;
        }
        return now.checked_add_signed(chrono::TimeDelta::try_seconds(seconds)?);
    }
    let date = DateTime::parse_from_rfc2822(value)
        .ok()?
        .with_timezone(&Utc);
    Some(date.max(now))
}

fn status_error(
    status: u16,
    endpoint: &str,
    retry_at: Option<String>,
    invalid_grant: bool,
) -> ProviderError {
    let mut error = match status {
        401 | 403 => ProviderError::auth_required(
            "로그인이 만료되었거나 접근 권한이 없습니다. 다시 로그인하세요.",
        ),
        400 if invalid_grant => {
            ProviderError::auth_required("저장된 로그인을 갱신할 수 없습니다. 다시 로그인하세요.")
        }
        429 => ProviderError::new(
            "rateLimited",
            "서버 요청이 일시적으로 제한되었습니다. 대기 후 다시 확인합니다.",
        ),
        408 | 500..=599 => ProviderError::temporary(
            "서버가 일시적으로 응답하지 못했습니다. 잠시 후 다시 확인합니다.",
        ),
        _ => ProviderError::new("invalidData", "서버에서 조회 요청을 처리하지 못했습니다."),
    };
    error.endpoint = Some(endpoint.into());
    error.status = Some(status);
    error.retry_at = retry_at;
    error
}

pub async fn response_error(response: reqwest::Response, endpoint: &str) -> ProviderError {
    let status = response.status().as_u16();
    let retry_at = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| retry_after(value, Utc::now()))
        .map(|value| value.to_rfc3339());
    let invalid_grant = if status == 400 && endpoint.ends_with(".token") {
        response
            .json::<Value>()
            .await
            .ok()
            .and_then(|body| body.get("error").and_then(Value::as_str).map(str::to_owned))
            .is_some_and(|code| {
                matches!(
                    code.as_str(),
                    "invalid_grant" | "invalid_token" | "expired_token"
                )
            })
    } else {
        false
    };
    status_error(status, endpoint, retry_at, invalid_grant)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        let now = DateTime::parse_from_rfc3339("2026-09-07T06:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            retry_after("120", now).unwrap(),
            now + chrono::TimeDelta::seconds(120)
        );
        assert_eq!(
            retry_after("Mon, 07 Sep 2026 06:02:00 GMT", now).unwrap(),
            now + chrono::TimeDelta::seconds(120)
        );
        assert_eq!(retry_after("Mon, 07 Sep 2026 05:00:00 GMT", now), Some(now));
        assert!(retry_after("-1", now).is_none());
        assert!(retry_after("invalid", now).is_none());
        assert!(retry_after("999999999999999999999999", now).is_none());
    }

    #[test]
    fn token_rate_limit_retains_status_and_retry_information() {
        let retry_at = Some("2026-09-07T06:02:00Z".into());
        let error = status_error(429, "claude.token", retry_at.clone(), false);
        assert_eq!(error.code, "rateLimited");
        assert_eq!(error.status, Some(429));
        assert_eq!(error.endpoint.as_deref(), Some("claude.token"));
        assert_eq!(error.retry_at, retry_at);
        assert_eq!(
            status_error(503, "claude.token", None, false).code,
            "temporary"
        );
        assert_eq!(
            status_error(400, "claude.token", None, true).code,
            "authRequired"
        );
        assert_eq!(
            status_error(400, "claude.token", None, false).code,
            "invalidData"
        );
    }
}
