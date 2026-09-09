mod command;
mod local_usage;
mod models;
mod providers;
mod store;
mod tray_widget;
mod usage;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(not(feature = "microsoft-store"))]
use std::time::Duration;

use models::{Account, AccountUsage, ProviderError, ProviderId, UpdateInfo};
#[cfg(not(feature = "microsoft-store"))]
use serde::Deserialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime, State, WindowEvent};
use tauri_plugin_opener::OpenerExt;
use usage::UsageState;

#[cfg(not(feature = "microsoft-store"))]
const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/LCH-1/ai-usage-viewer/releases/latest";
#[cfg(not(feature = "microsoft-store"))]
const LATEST_RELEASE_URL: &str = "https://github.com/LCH-1/ai-usage-viewer/releases/latest";
#[cfg(feature = "microsoft-store")]
const LATEST_RELEASE_URL: &str = "https://apps.microsoft.com/detail/9N38KL2P8LK7";

#[cfg(not(feature = "microsoft-store"))]
#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
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
async fn remove_account(
    state: State<'_, Arc<UsageState>>,
    account_id: String,
) -> Result<(), ProviderError> {
    state
        .remove(&account_id, || store::remove_account(&account_id))
        .await?;
    providers::claude::clear_profile_cache(&account_id).await;
    providers::cursor::clear_metadata_cache(&account_id);
    Ok(())
}

#[tauri::command]
async fn authenticate_account<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Arc<UsageState>>,
    account_id: String,
) -> Result<(), ProviderError> {
    let account = store::find_account(&account_id)
        .map_err(|message| ProviderError::new("notFound", message))?;
    let state = Arc::clone(state.inner());
    state
        .authenticate(account_id.clone(), move || async move {
            match account.provider {
                ProviderId::Claude => providers::claude::authenticate(&account_id).await,
                ProviderId::Codex => providers::codex::authenticate(&app, &account_id).await,
                ProviderId::Cursor => providers::cursor::authenticate(&app, &account_id).await,
            }
        })
        .await
}

#[tauri::command]
async fn cancel_authentication(
    state: State<'_, Arc<UsageState>>,
    account_id: String,
) -> Result<(), ProviderError> {
    state.cancel_authentication(&account_id).await
}

#[tauri::command]
fn get_cached_usage(
    state: State<'_, Arc<UsageState>>,
    account_id: String,
) -> Result<Option<AccountUsage>, ProviderError> {
    let account = store::find_account(&account_id)
        .map_err(|message| ProviderError::new("notFound", message))?;
    state.cached(&account_id, account.provider)
}
#[tauri::command]
fn open_provider_portal<R: Runtime>(app: AppHandle<R>, account_id: String) -> Result<(), String> {
    let account = store::find_account(&account_id)?;
    app.opener()
        .open_url(account.provider.usage_url(), None::<&str>)
        .map_err(|error| error.to_string())
}

#[cfg(not(feature = "microsoft-store"))]
fn is_newer_version(latest: &str, current: &str) -> Result<bool, String> {
    let latest = semver::Version::parse(latest.trim_start_matches('v'))
        .map_err(|error| error.to_string())?;
    let current = semver::Version::parse(current.trim_start_matches('v'))
        .map_err(|error| error.to_string())?;
    Ok(latest > current)
}

#[cfg(feature = "microsoft-store")]
#[tauri::command]
async fn check_for_update() -> Result<UpdateInfo, String> {
    let current_version = env!("CARGO_PKG_VERSION");
    Ok(UpdateInfo {
        current_version: current_version.into(),
        latest_version: current_version.into(),
        available: false,
        release_url: LATEST_RELEASE_URL.into(),
    })
}

#[cfg(not(feature = "microsoft-store"))]
#[tauri::command]
async fn check_for_update() -> Result<UpdateInfo, String> {
    let current_version = env!("CARGO_PKG_VERSION");
    let client = reqwest::Client::builder()
        .user_agent(format!("ai-usage-viewer/{current_version}"))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("업데이트 확인 실패 ({})", response.status()));
    }
    let release: GitHubRelease = response.json().await.map_err(|error| error.to_string())?;
    let latest_version = release.tag_name.trim_start_matches('v').to_owned();
    Ok(UpdateInfo {
        current_version: current_version.into(),
        available: is_newer_version(&latest_version, current_version)?,
        latest_version,
        release_url: release.html_url,
    })
}

#[tauri::command]
fn open_latest_release<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    app.opener()
        .open_url(LATEST_RELEASE_URL, None::<&str>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn refresh_account(
    state: State<'_, Arc<UsageState>>,
    account_id: String,
    force: bool,
) -> Result<AccountUsage, ProviderError> {
    let account = store::find_account(&account_id)
        .map_err(|message| ProviderError::new("notFound", message))?;
    let state = Arc::clone(state.inner());
    if let Some(usage) = state.refresh_local(&account_id, account.provider).await? {
        return Ok(usage);
    }
    state
        .refresh(
            account_id.clone(),
            account.provider,
            force,
            move || async move {
                match account.provider {
                    ProviderId::Claude => providers::claude::usage(&account_id).await,
                    ProviderId::Codex => providers::codex::usage(&account_id).await,
                    ProviderId::Cursor => providers::cursor::usage(&account_id).await,
                }
            },
        )
        .await
}
fn reveal_main_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let _ = tray_widget::hide(app);
    if let Some(window) = app.get_webview_window("main") {
        window.show()?;
        window.unminimize()?;
        window.set_focus()?;
    }
    Ok(())
}

#[tauri::command]
fn show_main_window<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    reveal_main_window(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn hide_widget<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    tray_widget::hide(&app).map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let quitting = Arc::new(AtomicBool::new(false));
    let close_state = Arc::clone(&quitting);
    let tray_state = Arc::clone(&quitting);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            let _ = reveal_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .manage(Arc::new(UsageState::default()))
        .manage(tray_widget::WidgetState::default())
        .setup(move |app| {
            let local_state = Arc::clone(app.state::<Arc<UsageState>>().inner());
            tauri::async_runtime::spawn(async move {
                let mut timer = tokio::time::interval(std::time::Duration::from_secs(5));
                timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    timer.tick().await;
                    if let Ok(accounts) = store::list_accounts() {
                        for account in accounts {
                            let _ = local_state
                                .refresh_local(&account.id, account.provider)
                                .await;
                        }
                    }
                }
            });
            if let Err(error) = tray_widget::create(app.handle()) {
                eprintln!("트레이 위젯을 준비하지 못했습니다: {error}");
            }
            let show = MenuItem::with_id(app, "show", "Usage Viewer 열기", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main-tray")
                .tooltip("Usage Viewer")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => {
                        let _ = reveal_main_window(app);
                    }
                    "quit" => {
                        tray_state.store(true, Ordering::SeqCst);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| match event {
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Down,
                        ..
                    } => tray_widget::press(tray.app_handle()),
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        position,
                        ..
                    } => {
                        if tray.app_handle().get_webview_window("widget").is_some() {
                            if let Err(error) =
                                tray_widget::toggle(tray.app_handle(), rect, position)
                            {
                                eprintln!("트레이 위젯을 표시하지 못했습니다: {error}");
                                let _ = reveal_main_window(tray.app_handle());
                            }
                        } else {
                            let _ = reveal_main_window(tray.app_handle());
                        }
                    }
                    TrayIconEvent::Click {
                        button: MouseButton::Right,
                        ..
                    } => {
                        let _ = tray_widget::hide(tray.app_handle());
                    }
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(move |window, event| {
            tray_widget::on_window_event(window, event);
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
            cancel_authentication,
            get_cached_usage,
            open_provider_portal,
            check_for_update,
            open_latest_release,
            refresh_account,
            show_main_window,
            hide_widget,
        ])
        .run(tauri::generate_context!())
        .expect("Usage Viewer 실행 중 오류가 발생했습니다.");
}

#[cfg(test)]
mod tests {
    #[cfg(not(feature = "microsoft-store"))]
    use super::is_newer_version;

    #[cfg(not(feature = "microsoft-store"))]
    #[test]
    fn compares_release_versions() {
        assert!(is_newer_version("v0.1.7", "0.1.6").unwrap());
        assert!(!is_newer_version("v0.1.6", "0.1.6").unwrap());
        assert!(!is_newer_version("v0.1.5", "0.1.6").unwrap());
    }

    #[cfg(feature = "microsoft-store")]
    #[tokio::test]
    async fn keeps_updates_in_microsoft_store() {
        let update = super::check_for_update().await.unwrap();
        assert!(!update.available);
        assert_eq!(update.current_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(update.latest_version, update.current_version);
        assert_eq!(
            update.release_url,
            "https://apps.microsoft.com/detail/9N38KL2P8LK7"
        );
    }
}
