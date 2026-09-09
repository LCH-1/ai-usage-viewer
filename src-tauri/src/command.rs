use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};

use crate::models::ProviderError;

async fn first_existing(candidates: Vec<Option<PathBuf>>) -> Option<PathBuf> {
    for candidate in candidates.into_iter().flatten() {
        if tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
            return Some(candidate);
        }
    }
    None
}

async fn find_on_path(name: &str) -> Option<PathBuf> {
    let command = if cfg!(windows) { "where.exe" } else { "which" };
    let output = Command::new(command)
        .arg(name)
        .creation_flags_if_windows()
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
}

trait WindowsCommandExt {
    fn creation_flags_if_windows(&mut self) -> &mut Self;
}

impl WindowsCommandExt for Command {
    fn creation_flags_if_windows(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.as_std_mut().creation_flags(0x08000000);
        }
        self
    }
}

pub async fn find_claude_executable() -> Result<PathBuf, String> {
    let user_profile = std::env::var_os("USERPROFILE").map(PathBuf::from);
    first_existing(vec![
        std::env::var_os("CLAUDE_CLI_PATH").map(PathBuf::from),
        user_profile.map(|path| path.join(".local").join("bin").join("claude.exe")),
    ])
    .await
    .or(find_on_path(if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    })
    .await)
    .ok_or("Claude Code CLI를 찾을 수 없습니다. Claude Code를 설치하고 다시 시도하세요.".into())
}

pub async fn find_codex_executable() -> Result<PathBuf, String> {
    if !cfg!(windows) {
        return first_existing(vec![std::env::var_os("CODEX_CLI_PATH").map(PathBuf::from)])
            .await
            .or(find_on_path("codex").await)
            .ok_or("Codex CLI 경로를 찾을 수 없습니다. 설치 경로와 PATH를 확인하세요.".into());
    }
    let explicit = std::env::var_os("CODEX_CLI_PATH").map(PathBuf::from);
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|value| {
            std::env::split_paths(&value)
                .filter(|path| path.is_absolute())
                .collect()
        })
        .unwrap_or_default();
    directories.extend(std::env::var_os("NVM_SYMLINK").map(PathBuf::from));
    directories.extend(std::env::var_os("APPDATA").map(|path| PathBuf::from(path).join("npm")));
    tokio::task::spawn_blocking(move || find_windows_codex(explicit.as_deref(), &directories))
        .await
        .ok()
        .flatten()
        .ok_or("Windows용 Codex CLI 실행 파일을 찾지 못했습니다. 설치 후 앱을 완전히 종료하고 다시 실행하거나 CODEX_CLI_PATH에 실행 파일 경로를 지정하세요. WSL에만 설치된 CLI는 지원하지 않습니다.".into())
}

fn find_windows_codex(explicit: Option<&Path>, directories: &[PathBuf]) -> Option<PathBuf> {
    if let Some(executable) = explicit.and_then(resolve_windows_codex) {
        return Some(executable);
    }
    let mut checked = std::collections::HashSet::new();
    for directory in directories {
        if !checked.insert(directory) {
            continue;
        }
        for name in ["codex.exe", "codex.cmd", "codex.ps1", "codex"] {
            if let Some(executable) = resolve_windows_codex(&directory.join(name)) {
                return Some(executable);
            }
        }
    }
    None
}

fn resolve_windows_codex(candidate: &Path) -> Option<PathBuf> {
    if !candidate.is_file() {
        return None;
    }
    if candidate
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Some(candidate.to_owned());
    }
    let prefix = candidate.parent()?;
    let modules = if prefix.file_name().is_some_and(|name| name == ".bin") {
        prefix.parent()?.to_owned()
    } else {
        prefix.join("node_modules")
    };
    let package = modules.join("@openai").join("codex");
    let package = std::fs::canonicalize(&package).unwrap_or(package);
    let targets = if cfg!(target_arch = "aarch64") {
        [
            ("arm64", "aarch64-pc-windows-msvc"),
            ("x64", "x86_64-pc-windows-msvc"),
        ]
    } else {
        [
            ("x64", "x86_64-pc-windows-msvc"),
            ("arm64", "aarch64-pc-windows-msvc"),
        ]
    };
    for (arch, triple) in targets {
        let platform_package = format!("codex-win32-{arch}");
        let mut vendors = vec![package.join("vendor")];
        // Node resolves optional platform packages in either nested or hoisted node_modules.
        for ancestor in package.ancestors().take(8) {
            vendors.push(
                ancestor
                    .join("node_modules")
                    .join("@openai")
                    .join(&platform_package)
                    .join("vendor"),
            );
        }
        for vendor in vendors {
            for binary_directory in ["bin", "codex"] {
                let executable = vendor.join(triple).join(binary_directory).join("codex.exe");
                if executable.is_file() {
                    return Some(executable);
                }
            }
        }
    }
    None
}

pub async fn run_claude_login(
    executable: &Path,
    config_directory: &Path,
) -> Result<(), ProviderError> {
    let mut child = Command::new(executable)
        .args(["auth", "login", "--claudeai"])
        .env("CLAUDE_CONFIG_DIR", config_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .creation_flags_if_windows()
        .spawn()
        .map_err(|_| ProviderError::temporary("Claude 로그인 프로세스를 실행하지 못했습니다."))?;
    let status = tokio::time::timeout(Duration::from_secs(5 * 60), child.wait())
        .await
        .map_err(|_| {
            ProviderError::temporary("Claude 로그인 시간이 초과되었습니다. 다시 시도하세요.")
        })?
        .map_err(|_| {
            ProviderError::temporary("Claude 로그인 프로세스의 완료를 확인하지 못했습니다.")
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(ProviderError::auth_required(format!(
            "Claude 로그인 프로세스가 종료 코드 {}로 끝났습니다.",
            status.code().unwrap_or(-1)
        )))
    }
}

pub fn spawn_codex(executable: &Path, codex_home: &Path) -> Result<Child, String> {
    Command::new(executable)
        .args(["app-server", "-c", "cli_auth_credentials_store=\"file\""])
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .creation_flags_if_windows()
        .spawn()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"fixture").unwrap();
    }

    #[test]
    fn resolves_nvm_wrappers_to_nested_native_binary() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("nvm4w").join("nodejs & space");
        let executable = prefix.join("node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe");
        file(&executable);
        for name in ["codex.ps1", "codex.cmd", "codex"] {
            let wrapper = prefix.join(name);
            file(&wrapper);
            let found = resolve_windows_codex(&wrapper).unwrap();
            assert_eq!(
                std::fs::canonicalize(found).unwrap(),
                std::fs::canonicalize(&executable).unwrap()
            );
        }
        assert!(find_windows_codex(None, &[prefix]).is_some());
    }

    #[test]
    fn resolves_hoisted_and_legacy_platform_packages() {
        for relative in [
            "node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe",
            "node_modules/@openai/codex/vendor/x86_64-pc-windows-msvc/codex/codex.exe",
        ] {
            let root = tempfile::tempdir().unwrap();
            let wrapper = root.path().join("codex.cmd");
            file(&wrapper);
            let executable = root.path().join(relative);
            file(&executable);
            let found = resolve_windows_codex(&wrapper).unwrap();
            assert_eq!(
                std::fs::canonicalize(found).unwrap(),
                std::fs::canonicalize(executable).unwrap()
            );
        }
    }

    #[test]
    fn skips_incomplete_installs_and_honors_explicit_native_path() {
        let root = tempfile::tempdir().unwrap();
        let broken = root.path().join("broken");
        file(&broken.join("codex.cmd"));
        let valid = root.path().join("valid");
        let native = valid.join("codex.exe");
        file(&native);
        assert_eq!(
            find_windows_codex(None, &[broken.clone(), valid.clone()]),
            Some(native.clone())
        );
        assert_eq!(
            find_windows_codex(Some(&native), &[broken.clone()]),
            Some(native)
        );
        assert!(find_windows_codex(None, &[broken]).is_none());
        assert!(resolve_windows_codex(&valid).is_none());
    }

    #[test]
    fn resolves_node_modules_bin_wrappers() {
        let root = tempfile::tempdir().unwrap();
        let wrapper = root.path().join("node_modules/.bin/codex.cmd");
        file(&wrapper);
        let executable = root.path().join("node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe");
        file(&executable);
        assert_eq!(
            std::fs::canonicalize(resolve_windows_codex(&wrapper).unwrap()).unwrap(),
            std::fs::canonicalize(executable).unwrap()
        );
    }
}
