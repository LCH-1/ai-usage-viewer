use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::{Child, Command};

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
    let app_data = std::env::var_os("APPDATA").map(PathBuf::from);
    first_existing(vec![
        std::env::var_os("CODEX_CLI_PATH").map(PathBuf::from),
        app_data.map(|path| {
            path.join("npm")
                .join("node_modules")
                .join("@openai")
                .join("codex")
                .join("node_modules")
                .join("@openai")
                .join("codex-win32-x64")
                .join("vendor")
                .join("x86_64-pc-windows-msvc")
                .join("bin")
                .join("codex.exe")
        }),
    ])
    .await
    .or(find_on_path(if cfg!(windows) { "codex.exe" } else { "codex" }).await)
    .ok_or("Codex CLI를 찾을 수 없습니다. Codex CLI를 설치하고 다시 시도하세요.".into())
}

pub async fn run_claude_login(executable: &Path, config_directory: &Path) -> Result<(), String> {
    let status = Command::new(executable)
        .args(["auth", "login", "--claudeai"])
        .env("CLAUDE_CONFIG_DIR", config_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .creation_flags_if_windows()
        .status()
        .await
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Claude 로그인 프로세스가 종료 코드 {}로 끝났습니다.",
            status.code().unwrap_or(-1)
        ))
    }
}

pub fn spawn_codex(executable: &Path, codex_home: &Path) -> Result<Child, String> {
    Command::new(executable)
        .args(["app-server", "-c", "cli_auth_credentials_store=\"file\""])
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags_if_windows()
        .spawn()
        .map_err(|error| error.to_string())
}
