use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use uuid::Uuid;

use crate::models::{Account, ProviderCredential, ProviderError, ProviderId, UsageSnapshot};

static STORE_LOCK: Mutex<()> = Mutex::new(());

pub fn data_directory() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        let app_data =
            std::env::var_os("APPDATA").ok_or("Windows AppData 경로를 찾을 수 없습니다.")?;
        return Ok(PathBuf::from(app_data).join("ai-usage-viewer"));
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var_os("HOME").ok_or("사용자 홈 경로를 찾을 수 없습니다.")?;
        Ok(PathBuf::from(home).join(".config").join("usage-viewer"))
    }
}

fn accounts_path() -> Result<PathBuf, String> {
    Ok(data_directory()?.join("accounts.json"))
}

fn credential_path(account_id: &str) -> Result<PathBuf, String> {
    Ok(data_directory()?
        .join("credentials")
        .join(format!("{account_id}.bin")))
}

pub fn list_accounts() -> Result<Vec<Account>, String> {
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    read_accounts(&accounts_path()?)
}

fn read_accounts(path: &Path) -> Result<Vec<Account>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&contents).map_err(|error| error.to_string())
}

fn save_accounts(accounts: &[Account]) -> Result<(), String> {
    let contents = serde_json::to_string_pretty(accounts).map_err(|error| error.to_string())?;
    atomic_write(&accounts_path()?, contents.as_bytes())
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), String> {
    let directory = path.parent().ok_or("저장 경로가 올바르지 않습니다.")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    temporary
        .write_all(contents)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn add_account(provider: ProviderId, label: &str) -> Result<Account, String> {
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    let mut accounts = read_accounts(&accounts_path()?)?;
    let default_number = accounts
        .iter()
        .filter(|account| account.provider == provider)
        .count()
        + 1;
    let trimmed = label.trim();
    let account = Account {
        id: Uuid::new_v4().to_string(),
        provider,
        label: if trimmed.is_empty() {
            format!("{} 계정 {default_number}", provider.name())
        } else {
            trimmed.into()
        },
        created_at: Utc::now().to_rfc3339(),
    };
    accounts.push(account.clone());
    save_accounts(&accounts)?;
    Ok(account)
}

pub fn rename_account(account_id: &str, label: &str) -> Result<Account, String> {
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    let next_label = label.trim();
    if next_label.is_empty() {
        return Err("계정 이름을 입력해 주세요.".into());
    }
    let mut accounts = read_accounts(&accounts_path()?)?;
    let account = accounts
        .iter_mut()
        .find(|account| account.id == account_id)
        .ok_or("계정을 찾을 수 없습니다.")?;
    account.label = next_label.into();
    let result = account.clone();
    save_accounts(&accounts)?;
    Ok(result)
}

pub fn remove_account(account_id: &str) -> Result<(), String> {
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    remove_account_at(&data_directory()?, account_id)
}

fn remove_account_at(directory: &Path, account_id: &str) -> Result<(), String> {
    let path = directory.join("accounts.json");
    let mut accounts = read_accounts(&path)?;
    for artifact in [
        directory
            .join("credentials")
            .join(format!("{account_id}.bin")),
        directory.join("usage").join(format!("{account_id}.json")),
    ] {
        match fs::remove_file(artifact) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    accounts.retain(|account| account.id != account_id);
    let contents = serde_json::to_vec_pretty(&accounts).map_err(|error| error.to_string())?;
    atomic_write(&path, &contents)
}

pub fn find_account(account_id: &str) -> Result<Account, String> {
    list_accounts()?
        .into_iter()
        .find(|account| account.id == account_id)
        .ok_or("계정을 찾을 수 없습니다.".into())
}

#[cfg(windows)]
fn protect(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    let success = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                null(),
                null(),
                null_mut(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                null_mut(),
                null(),
                null_mut(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if success == 0 {
        return Err("Windows 암호화 저장소를 사용할 수 없습니다.".into());
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as *mut core::ffi::c_void) };
    Ok(result)
}

#[cfg(not(windows))]
fn protect(_data: &[u8], _encrypt: bool) -> Result<Vec<u8>, String> {
    Err("이 운영체제의 보안 저장소는 아직 지원하지 않습니다.".into())
}

pub fn save_credential(account_id: &str, credential: &ProviderCredential) -> Result<(), String> {
    write_credential(account_id, credential, false)
}

pub fn save_authenticated_credential(
    account_id: &str,
    credential: &ProviderCredential,
) -> Result<(), String> {
    write_credential(account_id, credential, true)
}

fn write_credential(
    account_id: &str,
    credential: &ProviderCredential,
    invalidate_cache: bool,
) -> Result<(), String> {
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    let serialized = serde_json::to_vec(credential).map_err(|error| error.to_string())?;
    let mut encrypted = b"auv1".to_vec();
    encrypted.extend(protect(&serialized, true)?);
    commit_credential(&data_directory()?, account_id, &encrypted, invalidate_cache)
}

fn commit_credential(
    directory: &Path,
    account_id: &str,
    encrypted: &[u8],
    invalidate_cache: bool,
) -> Result<(), String> {
    require_account(directory, account_id)?;
    if invalidate_cache {
        let snapshot = directory.join("usage").join(format!("{account_id}.json"));
        match fs::remove_file(snapshot) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    atomic_write(
        &directory
            .join("credentials")
            .join(format!("{account_id}.bin")),
        encrypted,
    )
}

pub fn load_credential(account_id: &str) -> Result<ProviderCredential, ProviderError> {
    let _guard = STORE_LOCK
        .lock()
        .map_err(|error| ProviderError::new("storage", error.to_string()))?;
    let path = credential_path(account_id)?;
    if !path.exists() {
        return Err(ProviderError::auth_required(
            "로그인이 필요합니다. 로그인 버튼을 눌러 기본 브라우저에서 로그인하세요.",
        ));
    }
    let encrypted = fs::read(path).map_err(|error| error.to_string())?;
    let serialized = if let Some(payload) = encrypted.strip_prefix(b"auv1") {
        protect(payload, false)?
    } else if encrypted.starts_with(b"v10") {
        decrypt_electron_credential(&encrypted)?
    } else {
        protect(&encrypted, false)?
    };
    serde_json::from_slice(&serialized).map_err(|_| {
        ProviderError::auth_required("저장된 로그인 정보를 읽지 못했습니다. 다시 로그인하세요.")
    })
}

fn require_account(directory: &Path, account_id: &str) -> Result<(), String> {
    if read_accounts(&directory.join("accounts.json"))?
        .iter()
        .any(|account| account.id == account_id)
    {
        Ok(())
    } else {
        Err("계정을 찾을 수 없습니다.".into())
    }
}

pub fn load_snapshot(directory: &Path, account_id: &str) -> Result<UsageSnapshot, String> {
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    let path = directory.join("usage").join(format!("{account_id}.json"));
    match fs::read(path) {
        Ok(contents) => serde_json::from_slice(&contents).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(UsageSnapshot::default()),
        Err(error) => Err(error.to_string()),
    }
}

pub fn save_snapshot(
    directory: &Path,
    account_id: &str,
    snapshot: &UsageSnapshot,
) -> Result<(), String> {
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    require_account(directory, account_id)?;
    let contents = serde_json::to_vec(snapshot).map_err(|error| error.to_string())?;
    atomic_write(
        &directory.join("usage").join(format!("{account_id}.json")),
        &contents,
    )
}

fn decrypt_electron_credential(encrypted: &[u8]) -> Result<Vec<u8>, String> {
    if encrypted.len() < 3 + 12 + 16 {
        return Err("기존 Electron 로그인 파일이 손상되었습니다.".into());
    }
    let local_state = fs::read_to_string(data_directory()?.join("Local State"))
        .map_err(|error| error.to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&local_state).map_err(|error| error.to_string())?;
    let encoded_key = value
        .pointer("/os_crypt/encrypted_key")
        .and_then(serde_json::Value::as_str)
        .ok_or("기존 Electron 암호화 키를 찾을 수 없습니다.")?;
    let protected_key = STANDARD
        .decode(encoded_key)
        .map_err(|error| error.to_string())?;
    let protected_key = protected_key
        .strip_prefix(b"DPAPI")
        .ok_or("기존 Electron 암호화 키 형식이 올바르지 않습니다.")?;
    let key = protect(protected_key, false)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|error| error.to_string())?;
    let nonce = Nonce::from_slice(&encrypted[3..15]);
    cipher
        .decrypt(nonce, &encrypted[15..])
        .map_err(|_| "기존 Electron 로그인 파일을 해독하지 못했습니다.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accounts(directory: &Path) {
        atomic_write(&directory.join("accounts.json"), br#"[{"id":"one","provider":"claude","label":"test","createdAt":"2026-01-01T00:00:00Z"}]"#).unwrap();
    }

    #[test]
    fn atomic_write_replaces_complete_file_without_leftover_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credential.bin");
        atomic_write(&path, b"old encrypted fixture").unwrap();
        atomic_write(&path, b"new encrypted fixture").unwrap();
        assert_eq!(fs::read(path).unwrap(), b"new encrypted fixture");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_credential_removal_keeps_account_available_for_retry() {
        let directory = tempfile::tempdir().unwrap();
        accounts(directory.path());
        let credential = directory.path().join("credentials").join("one.bin");
        fs::create_dir_all(&credential).unwrap();
        assert!(remove_account_at(directory.path(), "one").is_err());
        assert_eq!(
            read_accounts(&directory.path().join("accounts.json"))
                .unwrap()
                .len(),
            1
        );
        fs::remove_dir(credential).unwrap();
        remove_account_at(directory.path(), "one").unwrap();
        assert!(read_accounts(&directory.path().join("accounts.json"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn deleted_account_cannot_regain_a_usage_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        accounts(directory.path());
        save_snapshot(directory.path(), "one", &UsageSnapshot::default()).unwrap();
        remove_account_at(directory.path(), "one").unwrap();
        assert!(save_snapshot(directory.path(), "one", &UsageSnapshot::default()).is_err());
        assert!(!directory.path().join("usage").join("one.json").exists());
    }

    #[test]
    fn encrypted_credentials_and_snapshot_are_removed_before_metadata() {
        let directory = tempfile::tempdir().unwrap();
        accounts(directory.path());
        let credential = directory.path().join("credentials").join("one.bin");
        atomic_write(&credential, b"encrypted fixture").unwrap();
        save_snapshot(directory.path(), "one", &UsageSnapshot::default()).unwrap();
        remove_account_at(directory.path(), "one").unwrap();
        assert!(!credential.exists());
        assert!(!directory.path().join("usage").join("one.json").exists());
        assert!(read_accounts(&directory.path().join("accounts.json"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn new_login_invalidates_previous_identity_snapshot_before_committing_credentials() {
        let directory = tempfile::tempdir().unwrap();
        accounts(directory.path());
        commit_credential(directory.path(), "one", b"previous encrypted login", false).unwrap();
        save_snapshot(directory.path(), "one", &UsageSnapshot::default()).unwrap();
        commit_credential(directory.path(), "one", b"new encrypted login", true).unwrap();
        assert!(!directory.path().join("usage").join("one.json").exists());
        assert_eq!(
            fs::read(directory.path().join("credentials").join("one.bin")).unwrap(),
            b"new encrypted login"
        );
    }

    #[test]
    fn failed_snapshot_invalidation_does_not_replace_the_previous_login() {
        let directory = tempfile::tempdir().unwrap();
        accounts(directory.path());
        commit_credential(directory.path(), "one", b"previous encrypted login", false).unwrap();
        fs::create_dir_all(directory.path().join("usage").join("one.json")).unwrap();
        assert!(commit_credential(directory.path(), "one", b"new encrypted login", true).is_err());
        assert_eq!(
            fs::read(directory.path().join("credentials").join("one.bin")).unwrap(),
            b"previous encrypted login"
        );
    }
}
