use chrono::{Datelike, Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub provider: ProviderId,
    pub label: String,
    pub created_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Claude,
    Codex,
    Cursor,
}

impl ProviderId {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "cursor" => Ok(Self::Cursor),
            _ => Err("지원하지 않는 플랫폼입니다.".into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
        }
    }

    pub fn usage_url(self) -> &'static str {
        match self {
            Self::Claude => "https://claude.ai/settings/usage",
            Self::Codex => "https://chatgpt.com/codex/settings/usage",
            Self::Cursor => "https://cursor.com/dashboard/spending",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetric {
    pub id: String,
    pub label: String,
    pub used_percent: f64,
    pub reset_text: Option<String>,
    #[serde(default)]
    pub resets_at: Option<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsage {
    pub account_id: String,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub metrics: Vec<UsageMetric>,
    pub fetched_at: String,
    pub source_url: String,
    pub warning: Option<String>,
    #[serde(default)]
    pub checked_at: Option<String>,
    #[serde(default)]
    pub next_retry_at: Option<String>,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub error: Option<ProviderError>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderError {
    pub code: String,
    pub message: String,
    pub retry_at: Option<String>,
    pub endpoint: Option<String>,
    pub status: Option<u16>,
}

impl ProviderError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retry_at: None,
            endpoint: None,
            status: None,
        }
    }

    pub fn auth_required(message: impl Into<String>) -> Self {
        Self::new("authRequired", message)
    }

    pub fn temporary(message: impl Into<String>) -> Self {
        Self::new("temporary", message)
    }

    pub fn cancelled() -> Self {
        Self::new("cancelled", "작업이 취소되었습니다.")
    }
}

impl From<String> for ProviderError {
    fn from(message: String) -> Self {
        Self::new("internal", message)
    }
}

impl From<&str> for ProviderError {
    fn from(message: &str) -> Self {
        Self::new("internal", message)
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProviderError {}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageSnapshot {
    pub usage: Option<AccountUsage>,
    pub failure_count: u32,
    pub retry_at: Option<String>,
    pub last_error: Option<ProviderError>,
    pub checked_at: Option<String>,
    pub poll_interval_seconds: u64,
    pub successful_refreshes: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub available: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeOauth {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: f64,
    pub scopes: Option<Vec<String>>,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "provider", rename_all = "lowercase")]
pub enum ProviderCredential {
    Claude {
        oauth: ClaudeOauth,
    },
    Codex {
        #[serde(rename = "authFile")]
        auth_file: String,
    },
    Cursor {
        #[serde(rename = "accessToken")]
        access_token: String,
        #[serde(rename = "refreshToken")]
        refresh_token: String,
    },
}

pub fn clamp_percent(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}

pub fn format_local_reset(timestamp_millis: i64) -> String {
    let date = Local
        .timestamp_millis_opt(timestamp_millis)
        .single()
        .unwrap();
    let (is_pm, hour) = date.hour12();
    let period = if is_pm { "오후" } else { "오전" };
    format!(
        "{}. {}. {}. {period} {hour}:{:02} 초기화",
        date.year(),
        date.month(),
        date.day(),
        date.minute()
    )
}

#[cfg(test)]
mod tests {
    use super::format_local_reset;

    #[test]
    fn reset_time_omits_seconds() {
        assert_eq!(
            format_local_reset(1_767_225_610_000).matches(':').count(),
            1
        );
    }
}
