use std::fs;
use std::path::PathBuf;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use uuid::Uuid;

use crate::models::{Account, ProviderCredential, ProviderId};

fn data_directory() -> Result<PathBuf, String> {
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
    let path = accounts_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&contents).map_err(|error| error.to_string())
}

fn save_accounts(accounts: &[Account]) -> Result<(), String> {
    let path = accounts_path()?;
    let directory = path.parent().ok_or("계정 저장 경로가 올바르지 않습니다.")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let contents = serde_json::to_string_pretty(accounts).map_err(|error| error.to_string())?;
    fs::write(&temporary, contents).map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

pub fn add_account(provider: ProviderId, label: &str) -> Result<Account, String> {
    let mut accounts = list_accounts()?;
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
    let next_label = label.trim();
    if next_label.is_empty() {
        return Err("계정 이름을 입력해 주세요.".into());
    }
    let mut accounts = list_accounts()?;
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
    let mut accounts = list_accounts()?;
    accounts.retain(|account| account.id != account_id);
    save_accounts(&accounts)?;
    let path = credential_path(account_id)?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    Ok(())
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
    let path = credential_path(account_id)?;
    fs::create_dir_all(
        path.parent()
            .ok_or("자격증명 저장 경로가 올바르지 않습니다.")?,
    )
    .map_err(|error| error.to_string())?;
    let serialized = serde_json::to_vec(credential).map_err(|error| error.to_string())?;
    let mut encrypted = b"auv1".to_vec();
    encrypted.extend(protect(&serialized, true)?);
    fs::write(path, encrypted).map_err(|error| error.to_string())
}

pub fn load_credential(account_id: &str) -> Result<ProviderCredential, String> {
    let path = credential_path(account_id)?;
    if !path.exists() {
        return Err(
            "로그인이 필요합니다. 로그인 버튼을 눌러 기본 브라우저에서 로그인하세요.".into(),
        );
    }
    let encrypted = fs::read(path).map_err(|error| error.to_string())?;
    let serialized = if let Some(payload) = encrypted.strip_prefix(b"auv1") {
        protect(payload, false)?
    } else if encrypted.starts_with(b"v10") {
        decrypt_electron_credential(&encrypted)?
    } else {
        protect(&encrypted, false)?
    };
    serde_json::from_slice(&serialized).map_err(|error| error.to_string())
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
    use super::{credential_path, list_accounts, load_credential};

    #[test]
    fn reads_existing_electron_credentials() {
        for account in list_accounts().expect("accounts should be readable") {
            if credential_path(&account.id)
                .expect("credential path should resolve")
                .exists()
            {
                load_credential(&account.id)
                    .expect("Electron credential should remain decryptable");
            }
        }
    }
}
