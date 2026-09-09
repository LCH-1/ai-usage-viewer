use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::models::{AccountUsage, ProviderCredential, ProviderId};
use crate::{providers, store};

const MAX_JSON: u64 = 128 * 1024;
const MAX_TAIL: u64 = 512 * 1024;
pub const MAX_AGE_SECONDS: i64 = 60;

pub fn date(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

pub fn fresh(usage: &AccountUsage, now: DateTime<Utc>) -> bool {
    date(&usage.fetched_at).is_some_and(|at| {
        let age = now.signed_duration_since(at).num_milliseconds();
        (0..MAX_AGE_SECONDS * 1000).contains(&age)
    }) && usage.metrics.iter().all(|metric| {
        metric
            .resets_at
            .as_deref()
            .and_then(date)
            .is_none_or(|reset| reset > now)
    })
}

fn read_json(path: &Path) -> Option<Value> {
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_JSON {
        return None;
    }
    serde_json::from_reader(file).ok()
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn identity(auth: &Value) -> Option<(String, String)> {
    let account = auth.pointer("/tokens/account_id")?.as_str()?;
    let token = auth.pointer("/tokens/id_token")?.as_str()?;
    let claims: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?).ok()?).ok()?;
    let subject = claims.get("sub")?.as_str()?;
    if account.is_empty() || subject.is_empty() {
        return None;
    }
    Some((account.into(), subject.into()))
}

fn read_tail(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let offset = size.saturating_sub(MAX_TAIL);
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_TAIL).read_to_end(&mut bytes).ok()?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if offset > 0 {
        text = text.split_once('\n')?.1.to_owned();
    }
    Some(text)
}

fn session_started(path: &Path) -> Option<DateTime<Utc>> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_JSON)
        .read_to_end(&mut bytes)
        .ok()?;
    let first = String::from_utf8_lossy(&bytes);
    let meta: Value = serde_json::from_str(first.lines().next()?).ok()?;
    if meta.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    date(meta.pointer("/payload/timestamp")?.as_str()?)
}

fn codex_event(account_id: &str, event: &Value, now: DateTime<Utc>) -> Option<AccountUsage> {
    if event.get("type")?.as_str()? != "event_msg"
        || event.pointer("/payload/type")?.as_str()? != "token_count"
    {
        return None;
    }
    let limits = event.pointer("/payload/rate_limits")?;
    if limits.get("limit_id")?.as_str()? != "codex" {
        return None;
    }
    let window = |name: &str| {
        let value = &limits[name];
        json!({"usedPercent": value["used_percent"], "windowDurationMins": value["window_minutes"], "resetsAt": value["resets_at"]})
    };
    let mut usage = providers::codex::parse_usage(account_id, &Value::Null, &json!({"rateLimits": {
        "primary": window("primary"), "secondary": window("secondary"), "planType": limits["plan_type"]
    }})).ok()?;
    usage.fetched_at = event.get("timestamp")?.as_str()?.into();
    usage.source_url = "local-codex://sessions".into();
    fresh(&usage, now).then_some(usage)
}

fn codex(
    root: &Path,
    account_id: &str,
    auth_file: &str,
    now: DateTime<Utc>,
) -> Option<AccountUsage> {
    let saved: Value = serde_json::from_str(auth_file).ok()?;
    let path = root.join("auth.json");
    let local = read_json(&path)?;
    if identity(&saved)? != identity(&local)? {
        return None;
    }
    let authenticated: DateTime<Utc> = fs::metadata(&path).ok()?.modified().ok()?.into();
    let mut files = Vec::new();
    for day in [now, now - chrono::Duration::days(1)] {
        let directory = root
            .join("sessions")
            .join(day.format("%Y/%m/%d").to_string());
        for entry in fs::read_dir(directory)
            .into_iter()
            .flatten()
            .take(2048)
            .flatten()
        {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
                continue;
            };
            let modified: DateTime<Utc> = modified.into();
            if now.signed_duration_since(modified).num_seconds() < MAX_AGE_SECONDS {
                files.push((modified, path));
            }
        }
    }
    files.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let mut latest: Option<AccountUsage> = None;
    for (_, path) in files.into_iter().take(8) {
        // Rollouts have no owner field: never reuse sessions predating the current login.
        if session_started(&path).is_none_or(|started| started < authenticated) {
            continue;
        }
        let Some(tail) = read_tail(&path) else {
            continue;
        };
        for line in tail.lines().rev() {
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(usage) = codex_event(account_id, &event, now) else {
                continue;
            };
            if latest
                .as_ref()
                .is_none_or(|previous| date(&previous.fetched_at) < date(&usage.fetched_at))
            {
                latest = Some(usage);
            }
            break;
        }
    }
    // An account switch during the file reads invalidates the sample as well.
    if read_json(&path)? != local {
        return None;
    }
    latest
}

fn claude(
    root: &Path,
    account_id: &str,
    previous: &AccountUsage,
    now: DateTime<Utc>,
) -> Option<AccountUsage> {
    let email = previous.email.as_deref()?.trim().to_lowercase();
    if email.is_empty() {
        return None;
    }
    let file = format!("{:x}.json", Sha256::digest(email.as_bytes()));
    let snapshot = read_json(&root.join("usage-viewer").join(file))?;
    if snapshot.get("version")?.as_u64()? != 1
        || snapshot.get("email")?.as_str()? != email
        || snapshot.get("accountUuid")?.as_str()?.is_empty()
    {
        return None;
    }
    let metrics = providers::claude::parse_metrics(snapshot.get("windows")?);
    if metrics.is_empty() {
        return None;
    }
    let usage = AccountUsage {
        account_id: account_id.into(),
        email: Some(email),
        plan: previous.plan.clone(),
        metrics,
        fetched_at: snapshot.get("observedAt")?.as_str()?.into(),
        source_url: "local-claude://statusline".into(),
        warning: None,
        checked_at: None,
        next_retry_at: None,
        stale: false,
        error: None,
    };
    fresh(&usage, now).then_some(usage)
}

pub fn read(
    account_id: &str,
    provider: ProviderId,
    previous: Option<&AccountUsage>,
) -> Option<AccountUsage> {
    if provider == ProviderId::Claude {
        let root = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| home().map(|home| home.join(".claude")))?;
        return claude(&root, account_id, previous?, Utc::now());
    }
    if provider != ProviderId::Codex {
        return None;
    }
    let ProviderCredential::Codex { auth_file } = store::load_credential(account_id).ok()? else {
        return None;
    };
    let root = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".codex")))?;
    codex(&root, account_id, &auth_file, Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(now: DateTime<Utc>) -> Value {
        json!({"type":"event_msg","timestamp":now.to_rfc3339(),"payload":{"type":"token_count","rate_limits":{
            "limit_id":"codex","primary":{"used_percent":23,"window_minutes":10080,"resets_at":(now+chrono::Duration::days(1)).timestamp()},"plan_type":"pro"
        }}})
    }

    #[test]
    fn accepts_real_quota_but_rejects_old_future_and_other_model_samples() {
        let now = Utc::now();
        let usage = codex_event("one", &event(now), now).unwrap();
        assert_eq!(usage.metrics[0].used_percent, 23.0);
        assert_eq!(usage.metrics[0].label, "주간");
        assert!(codex_event("one", &event(now - chrono::Duration::seconds(60)), now).is_none());
        assert!(codex_event("one", &event(now + chrono::Duration::seconds(1)), now).is_none());
        let mut other = event(now);
        other["payload"]["rate_limits"]["limit_id"] = json!("other");
        assert!(codex_event("one", &other, now).is_none());
        let mut reset = event(now);
        reset["payload"]["rate_limits"]["primary"]["resets_at"] = json!(now.timestamp());
        assert!(codex_event("one", &reset, now).is_none());
    }

    #[test]
    fn identity_requires_both_user_and_subscription_account() {
        let auth = |account: &str, subject: &str| json!({"tokens":{"account_id":account,"id_token":format!("x.{}.x",URL_SAFE_NO_PAD.encode(json!({"sub":subject}).to_string()))}});
        assert_eq!(identity(&auth("a", "u")), identity(&auth("a", "u")));
        assert_ne!(identity(&auth("a", "u")), identity(&auth("a", "other")));
        assert_ne!(identity(&auth("a", "u")), identity(&auth("other", "u")));
        assert!(identity(&json!({"tokens":{"account_id":"a"}})).is_none());
    }

    #[test]
    fn codex_scanner_ignores_previous_login_sessions_and_partial_writes() {
        let root = tempfile::tempdir().unwrap();
        let auth = json!({"tokens":{"account_id":"one","id_token":format!("x.{}.x",URL_SAFE_NO_PAD.encode(r#"{"sub":"user"}"#))}});
        fs::write(root.path().join("auth.json"), auth.to_string()).unwrap();
        let now = Utc::now() + chrono::Duration::seconds(3);
        let directory = root
            .path()
            .join("sessions")
            .join(now.format("%Y/%m/%d").to_string());
        fs::create_dir_all(&directory).unwrap();
        let session = directory.join("rollout.jsonl");
        let write_session = |started: DateTime<Utc>| {
            let meta = json!({"type":"session_meta","payload":{"timestamp":started.to_rfc3339()}});
            fs::write(&session, format!("{meta}\n{}\n{{incomplete", event(now))).unwrap();
        };
        write_session(now - chrono::Duration::days(1));
        assert!(codex(root.path(), "one", &auth.to_string(), now).is_none());
        write_session(now - chrono::Duration::seconds(1));
        let usage = codex(root.path(), "one", &auth.to_string(), now).unwrap();
        assert_eq!(usage.account_id, "one");
        assert_eq!(usage.source_url, "local-codex://sessions");
        let mut other = auth.clone();
        other["tokens"]["account_id"] = json!("two");
        assert!(codex(root.path(), "two", &other.to_string(), now).is_none());
    }

    #[test]
    fn claude_sidecar_requires_matching_email_and_recent_observation() {
        let root = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let mut previous = codex_event("one", &event(now), now).unwrap();
        previous.email = Some("One@Example.invalid".into());
        let directory = root.path().join("usage-viewer");
        fs::create_dir(&directory).unwrap();
        let path = directory.join(format!("{:x}.json", Sha256::digest(b"one@example.invalid")));
        let mut data = json!({"version":1,"email":"one@example.invalid","accountUuid":"one","observedAt":now.to_rfc3339(),"windows":{"five_hour":{"utilization":42,"resets_at":(now+chrono::Duration::hours(1)).to_rfc3339()}}});
        fs::write(&path, data.to_string()).unwrap();
        let usage = claude(root.path(), "one", &previous, now).unwrap();
        assert_eq!(usage.metrics[0].used_percent, 42.0);
        assert!(claude(
            root.path(),
            "one",
            &previous,
            now + chrono::Duration::seconds(60)
        )
        .is_none());
        data["email"] = json!("two@example.invalid");
        fs::write(&path, data.to_string()).unwrap();
        assert!(claude(root.path(), "one", &previous, now).is_none());
    }
}
