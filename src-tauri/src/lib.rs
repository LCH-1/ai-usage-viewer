mod command;
mod models;
mod providers;
mod store;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use models::{Account, AccountUsage, ProviderId};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime, State, WindowEvent};
use tauri_plugin_opener::OpenerExt;

#[derive(Default)]
struct AuthenticationState(Mutex<HashSet<String>>);

const CLAUDE_CACHE_TTL: Duration = Duration::from_secs(60);
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(30);
const CLAUDE_REQUEST_GAP: Duration = Duration::from_secs(2);
const CLAUDE_BACKOFF: Duration = Duration::from_secs(90);

struct CachedUsage {
    usage: AccountUsage,
    fetched: Instant,
}

#[derive(Default)]
struct UsageState {
    cache: tokio::sync::Mutex<HashMap<String, CachedUsage>>,
    blocked_until: tokio::sync::Mutex<HashMap<String, Instant>>,
    claude_gate: tokio::sync::Mutex<()>,
    claude_last_request: tokio::sync::Mutex<Option<Instant>>,
}

impl UsageState {
    async fn fresh(&self, account_id: &str, ttl: Duration) -> Option<AccountUsage> {
        let cache = self.cache.lock().await;
        let cached = cache.get(account_id)?;
        (cached.fetched.elapsed() < ttl).then(|| checked_now(cached.usage.clone()))
    }

    async fn stale(&self, account_id: &str, warning: &str) -> Option<AccountUsage> {
        let cache = self.cache.lock().await;
        cache
            .get(account_id)
            .map(|cached| with_warning(checked_now(cached.usage.clone()), warning))
    }

    async fn save(&self, account_id: &str, usage: &AccountUsage) {
        self.cache.lock().await.insert(
            account_id.into(),
            CachedUsage {
                usage: usage.clone(),
                fetched: Instant::now(),
            },
        );
        self.blocked_until.lock().await.remove(account_id);
    }

    async fn is_blocked(&self, account_id: &str) -> bool {
        self.blocked_until
            .lock()
            .await
            .get(account_id)
            .is_some_and(|until| *until > Instant::now())
    }

    async fn block(&self, account_id: &str) {
        self.blocked_until
            .lock()
            .await
            .insert(account_id.into(), Instant::now() + CLAUDE_BACKOFF);
    }

    async fn clear(&self, account_id: &str) {
        self.cache.lock().await.remove(account_id);
        self.blocked_until.lock().await.remove(account_id);
    }
}

fn checked_now(mut usage: AccountUsage) -> AccountUsage {
    usage.fetched_at = Utc::now().to_rfc3339();
    usage
}

fn with_warning(mut usage: AccountUsage, warning: &str) -> AccountUsage {
    usage.warning = Some(match usage.warning {
        Some(current) if !current.is_empty() => format!("{current} {warning}"),
        _ => warning.into(),
    });
    usage
}

#[tauri::command]
fn list_accounts() -> Result<Vec<Account>, String> {
    store::list_accounts()
}

#[tauri::command]
fn add_account(provider: String, label: String) -> Result<Account, String> {
    store::add_account(ProviderId::parse(&provider)?, &label)
}

#[tauri::command]
fn rename_account(account_id: String, label: String) -> Result<Account, String> {
    store::rename_account(&account_id, &label)
}

#[tauri::command]
async fn remove_account(state: State<'_, UsageState>, account_id: String) -> Result<(), String> {
    store::remove_account(&account_id)?;
    state.clear(&account_id).await;
    Ok(())
}

#[tauri::command]
async fn authenticate_account<R: Runtime>(
    app: AppHandle<R>,
    authentication: State<'_, AuthenticationState>,
    usage: State<'_, UsageState>,
    account_id: String,
) -> Result<(), String> {
    {
        let mut active = authentication.0.lock().map_err(|error| error.to_string())?;
        if !active.insert(account_id.clone()) {
            return Ok(());
        }
    }
    let result = async {
        let account = store::find_account(&account_id)?;
        match account.provider {
            ProviderId::Claude => providers::claude::authenticate(&account_id).await,
            ProviderId::Codex => providers::codex::authenticate(&app, &account_id).await,
            ProviderId::Cursor => providers::cursor::authenticate(&app, &account_id).await,
        }
    }
    .await;
    if let Ok(mut active) = authentication.0.lock() {
        active.remove(&account_id);
    }
    if result.is_ok() {
        usage.clear(&account_id).await;
    }
    result
}

#[tauri::command]
fn open_provider_portal<R: Runtime>(app: AppHandle<R>, account_id: String) -> Result<(), String> {
    let account = store::find_account(&account_id)?;
    app.opener()
        .open_url(account.provider.usage_url(), None::<&str>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn refresh_account(
    state: State<'_, UsageState>,
    account_id: String,
    force: bool,
) -> Result<AccountUsage, String> {
    let account = store::find_account(&account_id)?;
    let ttl = if account.provider == ProviderId::Claude {
        CLAUDE_CACHE_TTL
    } else {
        DEFAULT_CACHE_TTL
    };
    if !force {
        if let Some(usage) = state.fresh(&account_id, ttl).await {
            return Ok(usage);
        }
    }

    if account.provider == ProviderId::Claude {
        let _gate = state.claude_gate.lock().await;
        if !force {
            if let Some(usage) = state.fresh(&account_id, ttl).await {
                return Ok(usage);
            }
        }
        if state.is_blocked(&account_id).await {
            return state
                .stale(&account_id, "Claude 요청 제한으로 최근 사용량을 표시합니다. 잠시 후 자동으로 다시 확인합니다.")
                .await
                .ok_or_else(|| "Claude 요청이 잠시 제한되었습니다. 잠시 후 다시 확인해 주세요.".into());
        }

        let wait = {
            let last_request = state.claude_last_request.lock().await;
            last_request.and_then(|last| CLAUDE_REQUEST_GAP.checked_sub(last.elapsed()))
        };
        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
        }
        *state.claude_last_request.lock().await = Some(Instant::now());

        return match providers::claude::usage(&account_id).await {
            Ok(usage) => {
                state.save(&account_id, &usage).await;
                Ok(usage)
            }
            Err(error) if error.contains("429") || error.contains("Too Many Requests") => {
                state.block(&account_id).await;
                state
                    .stale(&account_id, "Claude 요청 제한으로 최근 사용량을 표시합니다. 잠시 후 자동으로 다시 확인합니다.")
                    .await
                    .ok_or_else(|| "Claude 요청이 잠시 제한되었습니다. 잠시 후 다시 확인해 주세요.".into())
            }
            Err(error) => Err(error),
        };
    }

    let result = match account.provider {
        ProviderId::Claude => unreachable!(),
        ProviderId::Codex => providers::codex::usage(&account_id).await,
        ProviderId::Cursor => providers::cursor::usage(&account_id).await,
    };
    if let Ok(usage) = &result {
        state.save(&account_id, usage).await;
    }
    result
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let quitting = Arc::new(AtomicBool::new(false));
    let close_state = Arc::clone(&quitting);
    let tray_state = Arc::clone(&quitting);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_main_window(app)
        }))
        .plugin(tauri_plugin_opener::init())
        .manage(AuthenticationState::default())
        .manage(UsageState::default())
        .setup(move |app| {
            let show = MenuItem::with_id(app, "show", "Usage Viewer 열기", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main-tray")
                .tooltip("Usage Viewer")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "quit" => {
                        tray_state.store(true, Ordering::SeqCst);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if !close_state.load(Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            add_account,
            rename_account,
            remove_account,
            authenticate_account,
            open_provider_portal,
            refresh_account,
        ])
        .run(tauri::generate_context!())
        .expect("Usage Viewer 실행 중 오류가 발생했습니다.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_usage() -> AccountUsage {
        AccountUsage {
            account_id: "claude-one".into(),
            email: Some("one@example.com".into()),
            plan: Some("MAX".into()),
            metrics: Vec::new(),
            fetched_at: "2026-01-01T00:00:00Z".into(),
            source_url: "https://example.com".into(),
            warning: None,
        }
    }

    #[tokio::test]
    async fn cached_usage_updates_the_check_time() {
        let state = UsageState::default();
        let usage = sample_usage();
        state.save("claude-one", &usage).await;

        let cached = state
            .fresh("claude-one", Duration::from_secs(60))
            .await
            .unwrap();

        assert_eq!(cached.email, usage.email);
        assert_ne!(cached.fetched_at, usage.fetched_at);
    }

    #[tokio::test]
    async fn blocked_account_can_return_stale_usage() {
        let state = UsageState::default();
        state.save("claude-one", &sample_usage()).await;
        state.block("claude-one").await;

        let cached = state
            .stale("claude-one", "잠시 후 다시 확인합니다.")
            .await
            .unwrap();

        assert_eq!(cached.warning.as_deref(), Some("잠시 후 다시 확인합니다."));
        assert!(state.is_blocked("claude-one").await);
    }
}
