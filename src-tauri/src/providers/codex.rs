use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, TimeZone, Utc};
use serde_json::{json, Value};
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::command::{find_codex_executable, spawn_codex};
use crate::models::{clamp_percent, AccountUsage, ProviderCredential, ProviderError, UsageMetric};
use crate::store::{load_credential, save_authenticated_credential, save_credential};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);

struct AuthPersistence<F: FnMut(&str) -> Result<(), ProviderError>> {
    directory: PathBuf,
    previous: String,
    save: F,
}

impl<F: FnMut(&str) -> Result<(), ProviderError>> AuthPersistence<F> {
    fn new(directory: &Path, previous: String, save: F) -> Self {
        Self {
            directory: directory.into(),
            previous,
            save,
        }
    }

    fn persist(&mut self) -> Result<(), ProviderError> {
        let auth_file = std::fs::read_to_string(self.directory.join("auth.json"))
            .map_err(|_| ProviderError::new("storage", "Codex 로그인 정보를 읽지 못했습니다."))?;
        serde_json::from_str::<Value>(&auth_file).map_err(|_| {
            ProviderError::new("invalidData", "Codex 로그인 정보 형식이 올바르지 않습니다.")
        })?;
        if auth_file != self.previous {
            (self.save)(&auth_file)?;
            self.previous = auth_file;
        }
        Ok(())
    }
}

impl<F: FnMut(&str) -> Result<(), ProviderError>> Drop for AuthPersistence<F> {
    fn drop(&mut self) {
        let _ = self.persist();
    }
}

struct CodexServer {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    notifications: VecDeque<Value>,
}

impl CodexServer {
    async fn start(executable: &Path, home: &Path) -> Result<Self, ProviderError> {
        let mut child = spawn_codex(executable, home)?;
        let stdin = child
            .stdin
            .take()
            .ok_or("Codex app-server 입력을 열 수 없습니다.")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Codex app-server 출력을 열 수 없습니다.")?;
        let mut server = Self {
            child,
            stdin,
            lines: BufReader::new(stdout).lines(),
            next_id: 1,
            notifications: VecDeque::new(),
        };
        server.request("initialize", json!({
            "clientInfo": { "name": "ai-usage-viewer", "title": "Usage Viewer", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "experimentalApi": false }
        })).await?;
        server.notify("initialized", Value::Null).await?;
        Ok(server)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, ProviderError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = async {
            self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
                .await?;
            loop {
                let message = self.read().await?;
                if message.get("id").and_then(Value::as_u64) == Some(id) {
                    if let Some(error) = message.get("error") {
                        return Err(rpc_error(error, method));
                    }
                    return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                }
                if message.get("method").is_some() {
                    self.notifications.push_back(message);
                }
            }
        };
        tokio::time::timeout(REQUEST_TIMEOUT, request)
            .await
            .map_err(|_| {
                ProviderError::temporary(format!("Codex {method} 응답 시간이 초과되었습니다."))
            })?
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), ProviderError> {
        tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params })),
        )
        .await
        .map_err(|_| {
            ProviderError::temporary(format!("Codex {method} 전송 시간이 초과되었습니다."))
        })?
    }

    async fn wait_for(&mut self, method: &str) -> Result<Value, ProviderError> {
        if let Some(index) = self
            .notifications
            .iter()
            .position(|message| message.get("method").and_then(Value::as_str) == Some(method))
        {
            return Ok(self
                .notifications
                .remove(index)
                .and_then(|message| message.get("params").cloned())
                .unwrap_or(Value::Null));
        }
        let future = async {
            loop {
                let message = self.read().await?;
                if message.get("method").and_then(Value::as_str) == Some(method) {
                    return Ok(message.get("params").cloned().unwrap_or(Value::Null));
                }
                if message.get("method").is_some() {
                    self.notifications.push_back(message);
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(5 * 60), future)
            .await
            .map_err(|_| ProviderError::temporary("브라우저 로그인 시간이 초과되었습니다."))?
    }

    async fn write(&mut self, value: &Value) -> Result<(), ProviderError> {
        self.stdin
            .write_all(format!("{value}\n").as_bytes())
            .await
            .map_err(|_| {
                ProviderError::temporary("Codex app-server에 요청을 전송하지 못했습니다.")
            })?;
        self.stdin
            .flush()
            .await
            .map_err(|_| ProviderError::temporary("Codex app-server에 요청을 전송하지 못했습니다."))
    }

    async fn read(&mut self) -> Result<Value, ProviderError> {
        loop {
            let line = self
                .lines
                .next_line()
                .await
                .map_err(|_| ProviderError::temporary("Codex app-server 응답을 읽지 못했습니다."))?
                .ok_or_else(|| {
                    ProviderError::temporary("Codex app-server가 예기치 않게 종료되었습니다.")
                })?;
            if let Ok(message) = serde_json::from_str(&line) {
                return Ok(message);
            }
        }
    }

    async fn close(mut self) {
        let _ = self.child.kill().await;
    }
}

fn rpc_error(value: &Value, method: &str) -> ProviderError {
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let status = value
        .pointer("/data/httpStatus")
        .or_else(|| value.pointer("/data/statusCode"))
        .or_else(|| value.pointer("/data/status"))
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok());
    let expired_auth = message.contains("unauthorized")
        || message.contains("not authenticated")
        || message.contains("authentication required")
        || [
            "invalid refresh token",
            "refresh token is invalid",
            "refresh token has expired",
            "refresh token expired",
            "refresh token has been reused",
            "refresh token revoked",
            "refresh_token_reused",
            "invalid_grant",
        ]
        .iter()
        .any(|hint| message.contains(*hint));
    let code = match status {
        Some(429) => "rateLimited",
        Some(408 | 500..=599) => "temporary",
        Some(401 | 403) => "authRequired",
        Some(_) => "temporary",
        None if message.contains("429") || message.contains("too many requests") => "rateLimited",
        None if expired_auth => "authRequired",
        None => "temporary",
    };
    let mut error = match code {
        "authRequired" => {
            ProviderError::auth_required("Codex 로그인이 만료되었습니다. 다시 로그인하세요.")
        }
        "rateLimited" => ProviderError::new(
            "rateLimited",
            "Codex 요청이 잠시 제한되었습니다. 잠시 후 다시 확인해 주세요.",
        ),
        _ => ProviderError::temporary(
            "Codex app-server 요청에 실패했습니다. 잠시 후 다시 확인해 주세요.",
        ),
    };
    error.endpoint = Some(format!("codex-app-server://{method}"));
    error.status = status;
    error
}

fn reset_text(timestamp: Option<f64>) -> Option<String> {
    let timestamp = timestamp?;
    let date = Local.timestamp_opt(timestamp as i64, 0).single()?;
    Some(crate::models::format_local_reset(date.timestamp_millis()))
}

fn resets_at(timestamp: Option<f64>) -> Option<String> {
    let date = Utc.timestamp_opt(timestamp? as i64, 0).single()?;
    Some(date.to_rfc3339())
}

fn window_metric(id: &str, fallback: &str, window: Option<&Value>) -> Option<UsageMetric> {
    let window = window?;
    let used_percent = window.get("usedPercent").and_then(Value::as_f64)?;
    let minutes = window.get("windowDurationMins").and_then(Value::as_f64);
    let label = match minutes {
        Some(300.0) => "5시간".into(),
        Some(10_080.0) => "주간".into(),
        Some(value) => format!("{}시간", (value / 60.0).round()),
        None => fallback.into(),
    };
    Some(UsageMetric {
        id: id.into(),
        label,
        used_percent: clamp_percent(used_percent),
        reset_text: reset_text(window.get("resetsAt").and_then(Value::as_f64)),
        resets_at: resets_at(window.get("resetsAt").and_then(Value::as_f64)),
        detail: None,
    })
}

async fn read_auth_file(path: &Path) -> Result<String, ProviderError> {
    let auth_file = tokio::fs::read_to_string(path.join("auth.json"))
        .await
        .map_err(|_| ProviderError::new("storage", "Codex 로그인 정보를 읽지 못했습니다."))?;
    serde_json::from_str::<Value>(&auth_file).map_err(|_| {
        ProviderError::new("invalidData", "Codex 로그인 정보 형식이 올바르지 않습니다.")
    })?;
    Ok(auth_file)
}

pub async fn authenticate<R: Runtime>(
    app: &AppHandle<R>,
    account_id: &str,
) -> Result<(), ProviderError> {
    let directory = tempfile::Builder::new()
        .prefix("usage-viewer-codex-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let executable = find_codex_executable().await?;
    let mut server = CodexServer::start(&executable, directory.path()).await?;
    let result: Result<String, ProviderError> = async {
        let login = server
            .request(
                "account/login/start",
                json!({
                    "type": "chatgpt",
                    "useHostedLoginSuccessPage": true,
                    "appBrand": "codex"
                }),
            )
            .await?;
        let auth_url = login
            .get("authUrl")
            .and_then(Value::as_str)
            .ok_or("Codex 로그인 주소를 받지 못했습니다.")?;
        app.opener()
            .open_url(auth_url, None::<&str>)
            .map_err(|error| error.to_string())?;
        let completed = server.wait_for("account/login/completed").await?;
        if completed.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(ProviderError::auth_required(
                "Codex 로그인이 완료되지 않았습니다. 다시 시도하세요.",
            ));
        }
        read_auth_file(directory.path()).await
    }
    .await;
    server.close().await;
    let auth_file = result?;
    save_authenticated_credential(account_id, &ProviderCredential::Codex { auth_file })
        .map_err(|_| ProviderError::new("storage", "Codex 로그인 정보를 저장하지 못했습니다."))
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::{poll_fn, Future};
    use std::task::Poll;

    use serde_json::json;

    use super::{parse_usage, rpc_error, AuthPersistence};
    use crate::models::ProviderError;

    #[tokio::test]
    async fn dropping_inflight_work_persists_latest_file_without_a_process() {
        let directory = tempfile::tempdir().unwrap();
        let saved = RefCell::new(Vec::new());
        let mut work = Box::pin(async {
            let _persistence =
                AuthPersistence::new(directory.path(), "{\"generation\":1}".into(), |auth| {
                    saved.borrow_mut().push(auth.to_owned());
                    Ok(())
                });
            std::fs::write(directory.path().join("auth.json"), "{\"generation\":2}").unwrap();
            std::future::pending::<()>().await;
        });
        poll_fn(|context| {
            assert!(work.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(work);
        assert_eq!(*saved.borrow(), vec!["{\"generation\":2}"]);
    }

    #[test]
    fn early_start_failure_still_persists_changed_file() {
        let directory = tempfile::tempdir().unwrap();
        let saved = RefCell::new(Vec::new());
        let result = (|| {
            let _persistence = AuthPersistence::new(directory.path(), "{}".into(), |auth| {
                saved.borrow_mut().push(auth.to_owned());
                Ok(())
            });
            std::fs::write(directory.path().join("auth.json"), "{\"generation\":2}").unwrap();
            Err::<(), _>(ProviderError::temporary("simulated initialization failure"))
        })();
        assert!(result.is_err());
        assert_eq!(*saved.borrow(), vec!["{\"generation\":2}"]);
    }

    #[test]
    fn failed_save_retains_previous_value_and_drop_can_retry() {
        let directory = tempfile::tempdir().unwrap();
        let allow_save = Cell::new(false);
        let saved = RefCell::new(Vec::new());
        let mut persistence = AuthPersistence::new(directory.path(), "{}".into(), |auth| {
            if !allow_save.get() {
                return Err(ProviderError::new("storage", "simulated write failure"));
            }
            saved.borrow_mut().push(auth.to_owned());
            Ok(())
        });
        std::fs::write(directory.path().join("auth.json"), "{\"generation\":2}").unwrap();
        assert_eq!(persistence.persist().unwrap_err().code, "storage");
        assert_eq!(persistence.previous, "{}");
        allow_save.set(true);
        drop(persistence);
        assert_eq!(*saved.borrow(), vec!["{\"generation\":2}"]);
    }

    #[test]
    fn incomplete_json_is_never_saved_on_normal_or_drop_path() {
        let directory = tempfile::tempdir().unwrap();
        let mut persistence = AuthPersistence::new(directory.path(), "{}".into(), |_| {
            panic!("incomplete credentials must not be saved")
        });
        std::fs::write(directory.path().join("auth.json"), "{\"generation\":").unwrap();
        assert_eq!(persistence.persist().unwrap_err().code, "invalidData");
        drop(persistence);
    }

    #[test]
    fn scoped_codex_limits_override_legacy_snapshot() {
        let usage = parse_usage("test", &json!({"email": "test@example.com", "planType": "pro"}), &json!({
            "rateLimits": {"primary": {"usedPercent": 90}},
            "rateLimitsByLimitId": {"codex": {
                "primary": {"usedPercent": 25, "windowDurationMins": 300, "resetsAt": 1_800_000_000},
                "secondary": {"usedPercent": 60, "windowDurationMins": 10_080}
            }}
        })).unwrap();
        assert_eq!(usage.metrics[0].used_percent, 25.0);
        assert_eq!(usage.metrics[0].label, "5시간");
        assert!(usage.metrics[0].resets_at.is_some());
        assert_eq!(usage.metrics[1].label, "주간");
        assert_eq!(usage.plan.as_deref(), Some("PRO"));
    }

    #[test]
    fn legacy_snapshot_and_individual_limit_are_supported() {
        let usage = parse_usage(
            "test",
            &json!({}),
            &json!({"rateLimits": {
                "primary": {"usedPercent": 0},
                "individualLimit": {"remainingPercent": 20, "used": "80", "limit": "100"}
            }}),
        )
        .unwrap();
        assert_eq!(usage.metrics.len(), 2);
        assert_eq!(usage.metrics[1].used_percent, 80.0);
        assert_eq!(usage.metrics[1].detail.as_deref(), Some("80 / 100"));
    }

    #[test]
    fn unknown_usage_schema_does_not_replace_last_success() {
        assert_eq!(
            parse_usage("test", &json!({}), &json!({"rateLimits": {}}))
                .unwrap_err()
                .code,
            "invalidData"
        );
    }

    #[test]
    fn rpc_failures_preserve_error_category_and_endpoint() {
        let error = rpc_error(&json!({"data": {"httpStatus": 401}}), "account/read");
        assert_eq!(error.code, "authRequired");
        assert_eq!(error.status, Some(401));
        assert_eq!(
            error.endpoint.as_deref(),
            Some("codex-app-server://account/read")
        );
        assert_eq!(
            rpc_error(
                &json!({"message": "Too Many Requests"}),
                "account/rateLimits/read"
            )
            .code,
            "rateLimited"
        );
    }

    #[test]
    fn token_refresh_rate_limit_and_outage_keep_explicit_http_status() {
        for (status, expected) in [
            (429, "rateLimited"),
            (503, "temporary"),
            (408, "temporary"),
            (401, "authRequired"),
            (403, "authRequired"),
        ] {
            let error = rpc_error(
                &json!({
                    "message": "invalid refresh token",
                    "data": {"httpStatus": status}
                }),
                "account/read",
            );
            assert_eq!(error.code, expected);
            assert_eq!(error.status, Some(status));
        }
        assert_eq!(
            rpc_error(
                &json!({"message": "refresh token request failed"}),
                "account/read"
            )
            .code,
            "temporary"
        );
        assert_eq!(
            rpc_error(
                &json!({"message": "refresh token has expired"}),
                "account/read"
            )
            .code,
            "authRequired"
        );
    }
}

pub async fn usage(account_id: &str) -> Result<AccountUsage, ProviderError> {
    let ProviderCredential::Codex { auth_file } = load_credential(account_id)? else {
        return Err(ProviderError::auth_required(
            "저장된 Codex 로그인 정보가 올바르지 않습니다.",
        ));
    };
    let directory = tempfile::Builder::new()
        .prefix("usage-viewer-codex-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    tokio::fs::write(directory.path().join("auth.json"), &auth_file)
        .await
        .map_err(|error| error.to_string())?;
    let mut persistence = AuthPersistence::new(directory.path(), auth_file, |auth_file| {
        save_credential(
            account_id,
            &ProviderCredential::Codex {
                auth_file: auth_file.into(),
            },
        )
        .map_err(|_| {
            ProviderError::new("storage", "갱신된 Codex 로그인 정보를 저장하지 못했습니다.")
        })
    });
    let executable = find_codex_executable().await?;
    let mut server = CodexServer::start(&executable, directory.path()).await?;
    let result = async {
        let account_result = server
            .request("account/read", json!({ "refreshToken": true }))
            .await;
        persistence.persist()?;
        let account = account_result?;
        let details = account
            .get("account")
            .filter(|value| !value.is_null())
            .ok_or_else(|| {
                ProviderError::auth_required("Codex 로그인이 만료되었습니다. 다시 로그인하세요.")
            })?;
        if details.get("type").and_then(Value::as_str) != Some("chatgpt") {
            return Err(ProviderError::auth_required(
                "Codex 로그인이 만료되었습니다. 다시 로그인하세요.",
            ));
        }
        let response_result = server.request("account/rateLimits/read", Value::Null).await;
        persistence.persist()?;
        parse_usage(account_id, details, &response_result?)
    }
    .await;
    server.close().await;
    persistence.persist()?;
    result
}

fn parse_usage(
    account_id: &str,
    details: &Value,
    response: &Value,
) -> Result<AccountUsage, ProviderError> {
    let snapshot = response
        .pointer("/rateLimitsByLimitId/codex")
        .filter(|value| !value.is_null())
        .unwrap_or_else(|| response.get("rateLimits").unwrap_or(&Value::Null));
    let mut metrics = [
        window_metric("primary", "기본 한도", snapshot.get("primary")),
        window_metric("secondary", "보조 한도", snapshot.get("secondary")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if let Some(limit) = snapshot
        .get("individualLimit")
        .filter(|value| !value.is_null())
    {
        if let Some(remaining) = limit.get("remainingPercent").and_then(Value::as_f64) {
            let used = limit
                .get("used")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let total = limit
                .get("limit")
                .and_then(Value::as_str)
                .unwrap_or_default();
            metrics.push(UsageMetric {
                id: "spend".into(),
                label: "사용 한도".into(),
                used_percent: clamp_percent(100.0 - remaining),
                reset_text: reset_text(limit.get("resetsAt").and_then(Value::as_f64)),
                resets_at: resets_at(limit.get("resetsAt").and_then(Value::as_f64)),
                detail: Some(format!("{used} / {total}")),
            });
        }
    }
    if metrics.is_empty() {
        return Err(ProviderError::new(
            "invalidData",
            "Codex 사용량 항목을 받지 못했습니다.",
        ));
    }
    Ok(AccountUsage {
        account_id: account_id.into(),
        email: details
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_owned),
        plan: snapshot
            .get("planType")
            .filter(|value| !value.is_null())
            .or_else(|| details.get("planType"))
            .and_then(Value::as_str)
            .map(str::to_uppercase),
        metrics,
        fetched_at: Utc::now().to_rfc3339(),
        checked_at: None,
        next_retry_at: None,
        stale: false,
        error: None,
        source_url: "codex-app-server://account/rateLimits/read".into(),
        warning: None,
    })
}
