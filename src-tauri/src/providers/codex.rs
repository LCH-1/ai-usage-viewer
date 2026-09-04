use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;

use chrono::{Local, TimeZone, Utc};
use serde_json::{json, Value};
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::command::{find_codex_executable, spawn_codex};
use crate::models::{clamp_percent, AccountUsage, ProviderCredential, UsageMetric};
use crate::store::{load_credential, save_credential};

struct CodexServer {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    notifications: VecDeque<Value>,
}

impl CodexServer {
    async fn start(executable: &Path, home: &Path) -> Result<Self, String> {
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
            "clientInfo": { "name": "ai-usage-viewer", "title": "Usage Viewer", "version": "0.1.5" },
            "capabilities": { "experimentalApi": false }
        })).await?;
        server.notify("initialized", Value::Null).await?;
        Ok(server)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await?;
        loop {
            let message = self.read().await?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = message.get("error") {
                    return Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Codex app-server 요청 실패")
                        .into());
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            if message.get("method").is_some() {
                self.notifications.push_back(message);
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn wait_for(&mut self, method: &str) -> Result<Value, String> {
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
        tokio::time::timeout(Duration::from_secs(10 * 60), future)
            .await
            .map_err(|_| "브라우저 로그인 시간이 초과되었습니다.".to_string())?
    }

    async fn write(&mut self, value: &Value) -> Result<(), String> {
        self.stdin
            .write_all(format!("{value}\n").as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        self.stdin.flush().await.map_err(|error| error.to_string())
    }

    async fn read(&mut self) -> Result<Value, String> {
        loop {
            let line = self
                .lines
                .next_line()
                .await
                .map_err(|error| error.to_string())?
                .ok_or("Codex app-server가 예기치 않게 종료되었습니다.")?;
            if let Ok(message) = serde_json::from_str(&line) {
                return Ok(message);
            }
        }
    }

    async fn close(mut self) {
        let _ = self.child.kill().await;
    }
}

fn reset_text(timestamp: Option<f64>) -> Option<String> {
    let timestamp = timestamp?;
    let date = Local.timestamp_opt(timestamp as i64, 0).single()?;
    Some(crate::models::format_local_reset(date.timestamp_millis()))
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
        detail: None,
    })
}

async fn read_auth_file(path: &Path) -> Result<String, String> {
    tokio::fs::read_to_string(path.join("auth.json"))
        .await
        .map_err(|error| error.to_string())
}

pub async fn authenticate<R: Runtime>(app: &AppHandle<R>, account_id: &str) -> Result<(), String> {
    let directory = tempfile::Builder::new()
        .prefix("usage-viewer-codex-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let executable = find_codex_executable().await?;
    let mut server = CodexServer::start(&executable, directory.path()).await?;
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
        return Err(completed
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("Codex 로그인이 완료되지 않았습니다.")
            .into());
    }
    server
        .request("account/read", json!({ "refreshToken": true }))
        .await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let auth_file = read_auth_file(directory.path()).await?;
    server.close().await;
    save_credential(account_id, &ProviderCredential::Codex { auth_file })
}

pub async fn usage(account_id: &str) -> Result<AccountUsage, String> {
    let ProviderCredential::Codex { auth_file } = load_credential(account_id)? else {
        return Err("저장된 Codex 로그인 정보가 올바르지 않습니다.".into());
    };
    let directory = tempfile::Builder::new()
        .prefix("usage-viewer-codex-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    tokio::fs::write(directory.path().join("auth.json"), &auth_file)
        .await
        .map_err(|error| error.to_string())?;
    let executable = find_codex_executable().await?;
    let mut server = CodexServer::start(&executable, directory.path()).await?;
    let account = server
        .request("account/read", json!({ "refreshToken": true }))
        .await?;
    let details = account
        .get("account")
        .filter(|value| !value.is_null())
        .ok_or("Codex 로그인이 만료되었습니다. 다시 로그인하세요.")?;
    if details.get("type").and_then(Value::as_str) != Some("chatgpt") {
        return Err("Codex 로그인이 만료되었습니다. 다시 로그인하세요.".into());
    }
    let response = server
        .request("account/rateLimits/read", Value::Null)
        .await?;
    let snapshot = response
        .pointer("/rateLimitsByLimitId/codex")
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
                detail: Some(format!("{used} / {total}")),
            });
        }
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    let refreshed_auth = read_auth_file(directory.path()).await?;
    server.close().await;
    if refreshed_auth != auth_file {
        save_credential(
            account_id,
            &ProviderCredential::Codex {
                auth_file: refreshed_auth,
            },
        )?;
    }
    let warning = metrics
        .is_empty()
        .then(|| "Codex 사용량 항목을 받지 못했습니다.".into());
    Ok(AccountUsage {
        account_id: account_id.into(),
        email: details
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_owned),
        plan: snapshot
            .get("planType")
            .or_else(|| details.get("planType"))
            .and_then(Value::as_str)
            .map(str::to_uppercase),
        metrics,
        fetched_at: Utc::now().to_rfc3339(),
        source_url: "codex-app-server://account/rateLimits/read".into(),
        warning,
    })
}
