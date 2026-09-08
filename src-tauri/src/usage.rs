use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rand::Rng;
use tokio::sync::{watch, Mutex as AsyncMutex};

use crate::models::{AccountUsage, ProviderError, ProviderId, UsageSnapshot};
use crate::store;

type OperationResult<T> = Result<T, ProviderError>;
type Completion<T> = watch::Receiver<Option<OperationResult<T>>>;

struct Job<T: Clone> {
    id: u64,
    cancel: watch::Sender<bool>,
    completion: Completion<T>,
}

#[derive(Default)]
struct Control {
    deleted: bool,
    generation: u64,
    next_job: u64,
    refresh: Option<Job<AccountUsage>>,
    authentication: Option<Job<()>>,
    snapshot: UsageSnapshot,
}

#[derive(Default)]
struct AccountState {
    gate: AsyncMutex<()>,
    control: Mutex<Control>,
}

#[derive(Default)]
pub struct UsageState {
    accounts: Mutex<HashMap<String, Arc<AccountState>>>,
    directory: Option<PathBuf>,
}

const CLAUDE_POLL_SECONDS: u64 = 300;
const CLAUDE_MAX_POLL_SECONDS: u64 = 1800;
const CLAUDE_MANUAL_SECONDS: i64 = 60;

fn ttl(snapshot: &UsageSnapshot, provider: ProviderId) -> i64 {
    if provider == ProviderId::Claude {
        snapshot
            .poll_interval_seconds
            .clamp(CLAUDE_POLL_SECONDS, CLAUDE_MAX_POLL_SECONDS) as i64
    } else {
        30
    }
}

fn timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

fn snapshot_view(snapshot: &UsageSnapshot, provider: ProviderId) -> Option<AccountUsage> {
    let mut usage = snapshot.usage.clone()?;
    usage.stale = snapshot.last_error.is_some()
        || timestamp(&usage.fetched_at).is_none_or(|date| {
            Utc::now().signed_duration_since(date).num_seconds() >= ttl(snapshot, provider)
        });
    usage.checked_at = snapshot.checked_at.clone();
    usage.next_retry_at = snapshot.retry_at.clone();
    usage.error = snapshot.last_error.clone();
    Some(usage)
}

fn cached_result(
    snapshot: &UsageSnapshot,
    provider: ProviderId,
    force: bool,
) -> Option<OperationResult<AccountUsage>> {
    let blocked = snapshot
        .last_error
        .as_ref()
        .is_some_and(|error| error.code == "authRequired")
        || snapshot
            .retry_at
            .as_deref()
            .and_then(timestamp)
            .is_some_and(|until| until > Utc::now());
    if blocked {
        return Some(snapshot_view(snapshot, provider).ok_or_else(|| {
            snapshot
                .last_error
                .clone()
                .unwrap_or_else(|| ProviderError::temporary("잠시 후 다시 확인해 주세요."))
        }));
    }
    let view = snapshot_view(snapshot, provider)?;
    let recently_fetched = provider == ProviderId::Claude
        && timestamp(&view.fetched_at).is_some_and(|date| {
            Utc::now().signed_duration_since(date).num_seconds() < CLAUDE_MANUAL_SECONDS
        });
    (recently_fetched || (!force && !view.stale)).then_some(Ok(view))
}

fn retry_deadline(
    error: &ProviderError,
    failures: u32,
    now: DateTime<Utc>,
    claude_interval: Option<u64>,
) -> String {
    let base = if error.code == "rateLimited" {
        90_u64
    } else {
        15
    };
    let cap = if error.code == "rateLimited" {
        3600
    } else {
        300
    };
    let backoff = if error.code == "rateLimited" && claude_interval.is_some() {
        claude_interval.unwrap_or(base)
    } else {
        (base * (1_u64 << failures.saturating_sub(1).min(8))).min(cap)
    };
    let seconds = backoff.max(claude_interval.unwrap_or(0)) + rand::rng().random_range(0..=10);
    let local_deadline = now + chrono::Duration::seconds(seconds as i64);
    error
        .retry_at
        .as_deref()
        .and_then(timestamp)
        .map_or(local_deadline, |until| until.max(local_deadline))
        .to_rfc3339()
}

async fn wait_result<T: Clone>(mut completion: Completion<T>) -> OperationResult<T> {
    loop {
        if let Some(result) = completion.borrow().clone() {
            return result;
        }
        completion.changed().await.map_err(|_| {
            ProviderError::temporary("작업 결과를 받지 못했습니다. 다시 시도하세요.")
        })?;
    }
}

async fn wait_cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            return;
        }
    }
}

async fn cancellable<T, F>(
    cancel: &mut watch::Receiver<bool>,
    timeout: Duration,
    future: F,
) -> OperationResult<T>
where
    F: Future<Output = OperationResult<T>>,
{
    tokio::select! {
        biased;
        _ = wait_cancelled(cancel) => Err(ProviderError::cancelled()),
        result = tokio::time::timeout(timeout, future) => result
            .map_err(|_| ProviderError::temporary("응답 시간이 초과되었습니다. 잠시 후 다시 시도합니다."))?,
    }
}

impl UsageState {
    fn directory(&self) -> OperationResult<PathBuf> {
        self.directory.clone().map(Ok).unwrap_or_else(|| {
            store::data_directory().map_err(|message| ProviderError::new("storage", message))
        })
    }

    fn account(&self, account_id: &str) -> OperationResult<Arc<AccountState>> {
        let mut accounts = self
            .accounts
            .lock()
            .map_err(|error| ProviderError::from(error.to_string()))?;
        if let Some(account) = accounts.get(account_id) {
            return Ok(Arc::clone(account));
        }
        let snapshot = store::load_snapshot(&self.directory()?, account_id).unwrap_or_else(|_| {
            UsageSnapshot {
                last_error: Some(ProviderError::new(
                    "storage",
                    "이전 사용량 캐시를 읽지 못했습니다. 사용량을 다시 확인합니다.",
                )),
                ..UsageSnapshot::default()
            }
        });
        let state = Arc::new(AccountState {
            control: Mutex::new(Control {
                snapshot,
                ..Control::default()
            }),
            ..AccountState::default()
        });
        accounts.insert(account_id.into(), Arc::clone(&state));
        Ok(state)
    }

    pub fn cached(
        &self,
        account_id: &str,
        provider: ProviderId,
    ) -> OperationResult<Option<AccountUsage>> {
        let account = self.account(account_id)?;
        let control = account
            .control
            .lock()
            .map_err(|error| ProviderError::from(error.to_string()))?;
        if control.deleted {
            return Err(ProviderError::new("notFound", "계정을 찾을 수 없습니다."));
        }
        Ok(snapshot_view(&control.snapshot, provider))
    }

    pub async fn refresh<F, Fut>(
        self: &Arc<Self>,
        account_id: String,
        provider: ProviderId,
        force: bool,
        fetch: F,
    ) -> OperationResult<AccountUsage>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = OperationResult<AccountUsage>> + Send + 'static,
    {
        let account = self.account(&account_id)?;
        let directory = self.directory()?;
        let completion = {
            let mut control = account
                .control
                .lock()
                .map_err(|error| ProviderError::from(error.to_string()))?;
            if control.deleted {
                return Err(ProviderError::new("notFound", "계정을 찾을 수 없습니다."));
            }
            if control.authentication.is_some() {
                return Err(ProviderError::new(
                    "authenticating",
                    "로그인이 진행 중입니다.",
                ));
            }
            if let Some(job) = &control.refresh {
                job.completion.clone()
            } else if let Some(result) = cached_result(&control.snapshot, provider, force) {
                return result;
            } else {
                let (finished, completion) = watch::channel(None);
                let (cancel, mut cancelled) = watch::channel(false);
                control.next_job += 1;
                let job_id = control.next_job;
                let generation = control.generation;
                control.refresh = Some(Job {
                    id: job_id,
                    cancel,
                    completion: completion.clone(),
                });
                let worker = Arc::clone(&account);
                tokio::spawn(async move {
                    let result = cancellable(&mut cancelled, Duration::from_secs(120), async {
                        let _gate = worker.gate.lock().await;
                        fetch().await
                    })
                    .await;
                    let result = match worker.control.lock() {
                        Ok(mut control) => {
                            let current = control.generation == generation && !control.deleted;
                            let result = if current {
                                finish_refresh(
                                    &mut control.snapshot,
                                    result,
                                    provider,
                                    &directory,
                                    &account_id,
                                )
                            } else {
                                Err(ProviderError::cancelled())
                            };
                            if control.refresh.as_ref().is_some_and(|job| job.id == job_id) {
                                control.refresh = None;
                            }
                            result
                        }
                        Err(error) => Err(ProviderError::from(error.to_string())),
                    };
                    finished.send_replace(Some(result));
                });
                completion
            }
        };
        wait_result(completion).await
    }

    pub async fn authenticate<F, Fut>(
        self: &Arc<Self>,
        account_id: String,
        authenticate: F,
    ) -> OperationResult<()>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = OperationResult<()>> + Send + 'static,
    {
        let account = self.account(&account_id)?;
        let completion = {
            let mut control = account
                .control
                .lock()
                .map_err(|error| ProviderError::from(error.to_string()))?;
            if control.deleted {
                return Err(ProviderError::new("notFound", "계정을 찾을 수 없습니다."));
            }
            if let Some(job) = &control.authentication {
                job.completion.clone()
            } else {
                if let Some(job) = control.refresh.take() {
                    job.cancel.send_replace(true);
                }
                control.generation += 1;
                control.next_job += 1;
                let generation = control.generation;
                let job_id = control.next_job;
                let (finished, completion) = watch::channel(None);
                let (cancel, mut cancelled) = watch::channel(false);
                control.authentication = Some(Job {
                    id: job_id,
                    cancel,
                    completion: completion.clone(),
                });
                let worker = Arc::clone(&account);
                tokio::spawn(async move {
                    let result = cancellable(&mut cancelled, Duration::from_secs(5 * 60), async {
                        let _gate = worker.gate.lock().await;
                        authenticate().await
                    })
                    .await;
                    let result = match worker.control.lock() {
                        Ok(mut control) => {
                            let result = if control.generation != generation || control.deleted {
                                Err(ProviderError::cancelled())
                            } else if result.is_ok() {
                                control.snapshot = UsageSnapshot::default();
                                Ok(())
                            } else {
                                result
                            };
                            if control
                                .authentication
                                .as_ref()
                                .is_some_and(|job| job.id == job_id)
                            {
                                control.authentication = None;
                            }
                            result
                        }
                        Err(error) => Err(ProviderError::from(error.to_string())),
                    };
                    finished.send_replace(Some(result));
                });
                completion
            }
        };
        wait_result(completion).await
    }

    pub async fn cancel_authentication(&self, account_id: &str) -> OperationResult<()> {
        let account = self.account(account_id)?;
        let completion = {
            let control = account
                .control
                .lock()
                .map_err(|error| ProviderError::from(error.to_string()))?;
            control.authentication.as_ref().map(|job| {
                job.cancel.send_replace(true);
                job.completion.clone()
            })
        };
        if let Some(completion) = completion {
            let _ = wait_result(completion).await;
        }
        Ok(())
    }

    pub async fn remove<F>(&self, account_id: &str, remove: F) -> OperationResult<()>
    where
        F: FnOnce() -> Result<(), String>,
    {
        let account = self.account(account_id)?;
        let (refresh, authentication) = {
            let mut control = account
                .control
                .lock()
                .map_err(|error| ProviderError::from(error.to_string()))?;
            control.deleted = true;
            control.generation += 1;
            let refresh = control.refresh.as_ref().map(|job| {
                job.cancel.send_replace(true);
                job.completion.clone()
            });
            let authentication = control.authentication.as_ref().map(|job| {
                job.cancel.send_replace(true);
                job.completion.clone()
            });
            (refresh, authentication)
        };
        if let Some(completion) = refresh {
            let _ = wait_result(completion).await;
        }
        if let Some(completion) = authentication {
            let _ = wait_result(completion).await;
        }
        let _gate = account.gate.lock().await;
        let result = remove().map_err(|message| ProviderError::new("storage", message));
        let mut control = account
            .control
            .lock()
            .map_err(|error| ProviderError::from(error.to_string()))?;
        if result.is_ok() {
            control.snapshot = UsageSnapshot::default();
        } else {
            control.deleted = false;
        }
        result
    }
}

fn finish_refresh(
    snapshot: &mut UsageSnapshot,
    result: OperationResult<AccountUsage>,
    provider: ProviderId,
    directory: &std::path::Path,
    account_id: &str,
) -> OperationResult<AccountUsage> {
    let now = Utc::now();
    if result
        .as_ref()
        .is_err_and(|error| error.code == "cancelled")
    {
        return result;
    }
    snapshot.checked_at = Some(now.to_rfc3339());
    match result {
        Ok(mut usage) => {
            let recovery_sample = snapshot.usage.as_ref().is_some_and(|previous| {
                timestamp(&previous.fetched_at)
                    .zip(timestamp(&usage.fetched_at))
                    .is_some_and(|(previous, current)| {
                        current.signed_duration_since(previous).num_seconds()
                            >= ttl(snapshot, provider)
                    })
            });
            if provider == ProviderId::Claude && recovery_sample {
                snapshot.successful_refreshes = snapshot.successful_refreshes.saturating_add(1);
                if snapshot.successful_refreshes >= 3 {
                    snapshot.poll_interval_seconds =
                        (ttl(snapshot, provider) as u64 / 2).max(CLAUDE_POLL_SECONDS);
                    snapshot.successful_refreshes = 0;
                }
            }
            if let Some(previous) = &snapshot.usage {
                if usage.email.is_none() {
                    usage.email.clone_from(&previous.email);
                }
                if usage.plan.is_none() {
                    usage.plan.clone_from(&previous.plan);
                }
            }
            usage.stale = false;
            usage.error = None;
            usage.checked_at = snapshot.checked_at.clone();
            usage.next_retry_at = None;
            snapshot.usage = Some(usage);
            snapshot.failure_count = 0;
            snapshot.retry_at = None;
            snapshot.last_error = None;
        }
        Err(mut error) => {
            snapshot.failure_count = snapshot.failure_count.saturating_add(1);
            snapshot.successful_refreshes = 0;
            let claude_interval = if provider == ProviderId::Claude {
                let interval = ttl(snapshot, provider) as u64;
                if error.code == "rateLimited" {
                    snapshot.poll_interval_seconds = (interval * 2).min(CLAUDE_MAX_POLL_SECONDS);
                }
                Some(interval)
            } else {
                None
            };
            snapshot.retry_at = if error.code == "authRequired" {
                None
            } else {
                Some(retry_deadline(
                    &error,
                    snapshot.failure_count,
                    now,
                    claude_interval,
                ))
            };
            error.retry_at = snapshot.retry_at.clone();
            snapshot.last_error = Some(error);
        }
    }
    let mut result = snapshot_view(snapshot, provider).ok_or_else(|| {
        snapshot
            .last_error
            .clone()
            .unwrap_or_else(|| ProviderError::temporary("사용량을 받지 못했습니다."))
    });
    if let Err(message) = store::save_snapshot(directory, account_id, snapshot) {
        if let Ok(usage) = &mut result {
            let warning = format!("마지막 사용량을 저장하지 못했습니다: {message}");
            if usage.error.is_none() {
                usage.error = Some(ProviderError::new("storage", warning.clone()));
            }
            usage.warning = Some(match usage.warning.take() {
                Some(previous) => format!("{previous} {warning}"),
                None => warning,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn fixture() -> (tempfile::TempDir, Arc<UsageState>) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("accounts.json"), r#"[{"id":"one","provider":"claude","label":"test","createdAt":"2026-01-01T00:00:00Z"},{"id":"two","provider":"claude","label":"test","createdAt":"2026-01-01T00:00:00Z"}]"#).unwrap();
        let state = Arc::new(UsageState {
            directory: Some(directory.path().into()),
            ..UsageState::default()
        });
        (directory, state)
    }

    fn sample(id: &str) -> AccountUsage {
        AccountUsage {
            account_id: id.into(),
            email: Some("test@example.com".into()),
            plan: None,
            metrics: Vec::new(),
            fetched_at: Utc::now().to_rfc3339(),
            source_url: "test".into(),
            warning: None,
            checked_at: None,
            next_retry_at: None,
            stale: false,
            error: None,
        }
    }

    #[tokio::test]
    async fn concurrent_forced_refreshes_share_one_result() {
        let (_directory, state) = fixture();
        let calls = Arc::new(AtomicUsize::new(0));
        let fetch = || {
            let calls = Arc::clone(&calls);
            move || async move {
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
                Ok(sample("one"))
            }
        };
        let (first, second) = tokio::join!(
            state.refresh("one".into(), ProviderId::Claude, true, fetch()),
            state.refresh("one".into(), ProviderId::Claude, true, fetch())
        );
        assert_eq!(first.unwrap().fetched_at, second.unwrap().fetched_at);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn restart_preserves_success_time_and_retry_deadline() {
        let (directory, state) = fixture();
        let first = state
            .refresh("one".into(), ProviderId::Claude, true, || async {
                let mut usage = sample("one");
                usage.fetched_at = (Utc::now() - chrono::Duration::minutes(2)).to_rfc3339();
                Ok(usage)
            })
            .await
            .unwrap();
        let limited = state
            .refresh("one".into(), ProviderId::Claude, true, || async {
                Err(ProviderError::new("rateLimited", "limited"))
            })
            .await
            .unwrap();
        assert!(limited.stale);
        assert_eq!(limited.fetched_at, first.fetched_at);
        let restarted = Arc::new(UsageState {
            directory: Some(directory.path().into()),
            ..UsageState::default()
        });
        let restored = restarted
            .refresh("one".into(), ProviderId::Claude, true, || async {
                panic!("retry deadline must prevent HTTP calls")
            })
            .await
            .unwrap();
        assert_eq!(restored.next_retry_at, limited.next_retry_at);
        assert_eq!(restored.fetched_at, first.fetched_at);
        assert_eq!(restored.error.unwrap().code, "rateLimited");
    }

    #[tokio::test(start_paused = true)]
    async fn slow_account_does_not_hold_other_accounts_and_times_out() {
        let (_directory, state) = fixture();
        let (slow, fast) = tokio::join!(
            state.refresh("one".into(), ProviderId::Claude, true, || async {
                std::future::pending().await
            }),
            tokio::time::timeout(
                Duration::from_secs(1),
                state.refresh("two".into(), ProviderId::Claude, true, || async {
                    Ok(sample("two"))
                })
            ),
        );
        assert_eq!(slow.unwrap_err().code, "temporary");
        assert_eq!(fast.unwrap().unwrap().account_id, "two");
    }

    #[tokio::test]
    async fn duplicate_authentication_waits_and_cancellation_releases_it() {
        let (_directory, state) = fixture();
        let calls = Arc::new(AtomicUsize::new(0));
        let fetch = || {
            let calls = Arc::clone(&calls);
            move || async move {
                calls.fetch_add(1, Ordering::SeqCst);
                std::future::pending().await
            }
        };
        let cancel = async {
            tokio::task::yield_now().await;
            state.cancel_authentication("one").await.unwrap();
        };
        let (first, second, _) = tokio::join!(
            state.authenticate("one".into(), fetch()),
            state.authenticate("one".into(), fetch()),
            cancel
        );
        assert_eq!(first.unwrap_err().code, "cancelled");
        assert_eq!(second.unwrap_err().code, "cancelled");
        assert!(calls.load(Ordering::SeqCst) <= 1);
    }

    #[tokio::test]
    async fn deleting_an_account_cancels_work_and_rejects_late_refresh() {
        let (_directory, state) = fixture();
        let deletion = async {
            tokio::task::yield_now().await;
            state.remove("one", || Ok(())).await.unwrap();
        };
        let (refresh, _) = tokio::join!(
            state.refresh("one".into(), ProviderId::Claude, true, || async {
                std::future::pending().await
            }),
            deletion,
        );
        assert_eq!(refresh.unwrap_err().code, "cancelled");
        assert_eq!(
            state
                .refresh("one".into(), ProviderId::Claude, true, || async {
                    Ok(sample("one"))
                })
                .await
                .unwrap_err()
                .code,
            "notFound"
        );
    }

    #[test]
    fn retry_after_takes_precedence_over_exponential_fallback() {
        let now = Utc::now();
        let until = (now + chrono::Duration::hours(2)).to_rfc3339();
        let mut error = ProviderError::new("rateLimited", "limited");
        error.retry_at = Some(until.clone());
        assert_eq!(retry_deadline(&error, 1, now, None), until);
        assert_eq!(retry_deadline(&error, 1, now, Some(300)), until);
        error.retry_at = None;
        let first = timestamp(&retry_deadline(&error, 1, now, None)).unwrap();
        let second = timestamp(&retry_deadline(&error, 2, now, None)).unwrap();
        assert!(second > first);
    }

    #[test]
    fn short_retry_after_does_not_shorten_local_backoff() {
        let now = Utc::now();
        let mut error = ProviderError::new("rateLimited", "limited");
        error.retry_at = Some((now + chrono::Duration::seconds(1)).to_rfc3339());
        for (failures, interval, minimum) in [(2, None, 180), (1, Some(300), 300)] {
            let until = timestamp(&retry_deadline(&error, failures, now, interval)).unwrap();
            assert!(until >= now + chrono::Duration::seconds(minimum));
            assert!(until <= now + chrono::Duration::seconds(minimum + 10));
        }
    }

    #[tokio::test]
    async fn claude_manual_refresh_has_a_floor_and_automatic_refresh_waits_longer() {
        let (_directory, state) = fixture();
        state
            .refresh("one".into(), ProviderId::Claude, true, || async {
                Ok(sample("one"))
            })
            .await
            .unwrap();
        state
            .refresh("one".into(), ProviderId::Claude, true, || async {
                panic!("a second manual refresh must reuse the recent result")
            })
            .await
            .unwrap();
        let account = state.account("one").unwrap();
        {
            let mut control = account.control.lock().unwrap();
            control.snapshot.usage.as_mut().unwrap().fetched_at =
                (Utc::now() - chrono::Duration::seconds(70)).to_rfc3339();
        }
        state
            .refresh("one".into(), ProviderId::Claude, false, || async {
                panic!("automatic refresh must not return to the former one-minute cadence")
            })
            .await
            .unwrap();
        let refreshed = state
            .refresh("one".into(), ProviderId::Claude, true, || async {
                let mut usage = sample("one");
                usage.plan = Some("manually refreshed".into());
                Ok(usage)
            })
            .await
            .unwrap();
        assert_eq!(refreshed.plan.as_deref(), Some("manually refreshed"));
    }

    #[test]
    fn alternating_limits_and_successes_preserve_adaptive_cadence_across_restart() {
        let (directory, _state) = fixture();
        let mut snapshot = UsageSnapshot::default();
        let mut usage = sample("one");
        usage.fetched_at = (Utc::now() - chrono::Duration::minutes(2)).to_rfc3339();
        finish_refresh(
            &mut snapshot,
            Ok(usage.clone()),
            ProviderId::Claude,
            directory.path(),
            "one",
        )
        .unwrap();
        for (minimum, next_interval) in [(300, 600), (600, 1200), (1200, 1800), (1800, 1800)] {
            let now = Utc::now();
            let limited = finish_refresh(
                &mut snapshot,
                Err(ProviderError::new("rateLimited", "limited")),
                ProviderId::Claude,
                directory.path(),
                "one",
            )
            .unwrap();
            assert_eq!(limited.fetched_at, usage.fetched_at);
            assert!(
                timestamp(limited.next_retry_at.as_deref().unwrap()).unwrap()
                    >= now + chrono::Duration::seconds(minimum)
            );
            snapshot = store::load_snapshot(directory.path(), "one").unwrap();
            assert_eq!(snapshot.poll_interval_seconds, next_interval);
            assert!(cached_result(&snapshot, ProviderId::Claude, true).is_some());
            snapshot.usage.as_mut().unwrap().fetched_at =
                (Utc::now() - chrono::Duration::seconds(next_interval as i64 + 1)).to_rfc3339();
            usage.fetched_at = Utc::now().to_rfc3339();
            finish_refresh(
                &mut snapshot,
                Ok(usage.clone()),
                ProviderId::Claude,
                directory.path(),
                "one",
            )
            .unwrap();
            assert_eq!(snapshot.poll_interval_seconds, next_interval);
            assert_eq!(snapshot.successful_refreshes, 1);
            assert!(snapshot.retry_at.is_none());
        }
        for _ in 0..2 {
            snapshot.usage.as_mut().unwrap().fetched_at =
                (Utc::now() - chrono::Duration::seconds(1801)).to_rfc3339();
            usage.fetched_at = Utc::now().to_rfc3339();
            finish_refresh(
                &mut snapshot,
                Ok(usage.clone()),
                ProviderId::Claude,
                directory.path(),
                "one",
            )
            .unwrap();
        }
        assert_eq!(snapshot.poll_interval_seconds, 900);
        assert_eq!(snapshot.successful_refreshes, 0);
        assert_eq!(
            store::load_snapshot(directory.path(), "one")
                .unwrap()
                .poll_interval_seconds,
            900
        );
    }

    #[tokio::test]
    async fn limited_claude_account_does_not_block_another_account() {
        let (_directory, state) = fixture();
        let limited = state
            .refresh("one".into(), ProviderId::Claude, true, || async {
                Err(ProviderError::new("rateLimited", "limited"))
            })
            .await
            .unwrap_err();
        assert_eq!(limited.code, "rateLimited");
        let normal = state
            .refresh("two".into(), ProviderId::Claude, false, || async {
                Ok(sample("two"))
            })
            .await
            .unwrap();
        assert_eq!(normal.account_id, "two");
        assert!(normal.error.is_none());
    }

    #[tokio::test]
    async fn temporary_failure_after_a_limit_keeps_the_adaptive_wait() {
        let (_directory, state) = fixture();
        state
            .refresh("one".into(), ProviderId::Claude, true, || async {
                Err(ProviderError::new("rateLimited", "limited"))
            })
            .await
            .unwrap_err();
        let account = state.account("one").unwrap();
        account.control.lock().unwrap().snapshot.retry_at =
            Some((Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
        let now = Utc::now();
        let temporary = state
            .refresh("one".into(), ProviderId::Claude, false, || async {
                Err(ProviderError::temporary("network unavailable"))
            })
            .await
            .unwrap_err();
        assert!(
            timestamp(temporary.retry_at.as_deref().unwrap()).unwrap()
                >= now + chrono::Duration::seconds(600)
        );
        state
            .refresh("one".into(), ProviderId::Claude, false, || async {
                panic!("a network error must not collapse an existing rate-limit cooldown")
            })
            .await
            .unwrap_err();
    }

    #[test]
    fn rapid_manual_successes_do_not_shorten_the_adaptive_interval() {
        let (directory, _state) = fixture();
        let mut snapshot = UsageSnapshot {
            usage: Some(sample("one")),
            poll_interval_seconds: 1800,
            ..UsageSnapshot::default()
        };
        for _ in 0..3 {
            snapshot.usage.as_mut().unwrap().fetched_at =
                (Utc::now() - chrono::Duration::seconds(61)).to_rfc3339();
            finish_refresh(
                &mut snapshot,
                Ok(sample("one")),
                ProviderId::Claude,
                directory.path(),
                "one",
            )
            .unwrap();
        }
        assert_eq!(snapshot.poll_interval_seconds, 1800);
        assert_eq!(snapshot.successful_refreshes, 0);
    }

    #[test]
    fn old_snapshot_uses_default_cadence_and_storage_errors_remain_actionable() {
        let (directory, _state) = fixture();
        let mut snapshot: UsageSnapshot = serde_json::from_str(r#"{"failureCount":0}"#).unwrap();
        assert_eq!(ttl(&snapshot, ProviderId::Claude), 300);
        assert_eq!(ttl(&snapshot, ProviderId::Codex), 30);
        std::fs::write(directory.path().join("usage"), b"blocks snapshot directory").unwrap();
        let usage = finish_refresh(
            &mut snapshot,
            Ok(sample("one")),
            ProviderId::Claude,
            directory.path(),
            "one",
        )
        .unwrap();
        assert_eq!(usage.error.unwrap().code, "storage");
        assert!(snapshot.last_error.is_none());
        assert!(snapshot.retry_at.is_none());
    }

    #[tokio::test]
    async fn damaged_optional_snapshot_does_not_block_login_or_removal() {
        let (directory, state) = fixture();
        std::fs::create_dir(directory.path().join("usage")).unwrap();
        std::fs::write(
            directory.path().join("usage").join("one.json"),
            b"broken cache",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("usage").join("two.json"),
            b"broken cache",
        )
        .unwrap();
        state
            .authenticate("one".into(), || async { Ok(()) })
            .await
            .unwrap();
        assert!(state.cached("one", ProviderId::Claude).unwrap().is_none());
        state.remove("two", || Ok(())).await.unwrap();
    }

    #[tokio::test]
    async fn login_supersedes_pending_refresh_before_new_usage_is_requested() {
        let (_directory, state) = fixture();
        let login = async {
            tokio::task::yield_now().await;
            state
                .authenticate("one".into(), || async { Ok(()) })
                .await
                .unwrap();
            state
                .refresh("one".into(), ProviderId::Claude, true, || async {
                    Ok(sample("one"))
                })
                .await
                .unwrap()
        };
        let (old, current) = tokio::join!(
            state.refresh("one".into(), ProviderId::Claude, true, || async {
                std::future::pending().await
            }),
            login
        );
        assert_eq!(old.unwrap_err().code, "cancelled");
        assert_eq!(current.account_id, "one");
    }

    #[tokio::test]
    async fn authentication_error_stops_automatic_requests_until_login_completes() {
        let (_directory, state) = fixture();
        let expired = state
            .refresh("one".into(), ProviderId::Claude, true, || async {
                Err(ProviderError::auth_required("expired"))
            })
            .await
            .unwrap_err();
        assert_eq!(expired.code, "authRequired");
        assert_eq!(
            state
                .refresh("one".into(), ProviderId::Claude, true, || async {
                    panic!("invalid credentials must not be retried automatically")
                })
                .await
                .unwrap_err()
                .code,
            "authRequired"
        );
        state
            .authenticate("one".into(), || async { Ok(()) })
            .await
            .unwrap();
        assert!(state
            .refresh("one".into(), ProviderId::Claude, true, || async {
                Ok(sample("one"))
            })
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn deletion_waits_until_the_inflight_future_has_cleaned_up() {
        struct Cleanup(Arc<AtomicBool>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let (_directory, state) = fixture();
        let cleaned = Arc::new(AtomicBool::new(false));
        let started = Arc::new(tokio::sync::Notify::new());
        let worker_cleaned = Arc::clone(&cleaned);
        let worker_started = Arc::clone(&started);
        let refresh = state.refresh("one".into(), ProviderId::Claude, true, move || async move {
            let _cleanup = Cleanup(worker_cleaned);
            worker_started.notify_one();
            std::future::pending().await
        });
        let deletion = async {
            started.notified().await;
            state
                .remove("one", || {
                    assert!(cleaned.load(Ordering::SeqCst));
                    Ok(())
                })
                .await
                .unwrap();
        };
        let (result, _) = tokio::join!(refresh, deletion);
        assert_eq!(result.unwrap_err().code, "cancelled");
    }
}
