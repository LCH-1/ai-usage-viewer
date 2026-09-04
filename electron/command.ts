import { execFile, spawn, type ChildProcessWithoutNullStreams } from "node:child_process"
import { access } from "node:fs/promises"
import { join } from "node:path"
import { promisify } from "node:util"

const execFileAsync = promisify(execFile)

async function firstExisting(candidates: Array<string | undefined>): Promise<string | null> {
  for (const candidate of candidates) {
    if (!candidate) continue
    try {
      await access(candidate)
      return candidate
    } catch {
      continue
    }
  }
  return null
}

async function findOnPath(name: string): Promise<string | null> {
  try {
    if (process.platform === "win32") {
      const { stdout } = await execFileAsync("where.exe", [name], { windowsHide: true })
      return stdout.split(/\r?\n/).map((line) => line.trim()).find((line) => line.toLowerCase().endsWith(".exe")) ?? null
    }
    const { stdout } = await execFileAsync("which", [name])
    return stdout.trim().split(/\r?\n/)[0] || null
  } catch {
    return null
  }
}

export async function findClaudeExecutable(): Promise<string> {
  const executable = await firstExisting([
    process.env.CLAUDE_CLI_PATH,
    process.env.USERPROFILE ? join(process.env.USERPROFILE, ".local", "bin", "claude.exe") : undefined,
  ]) ?? await findOnPath(process.platform === "win32" ? "claude.exe" : "claude")
  if (!executable) throw new Error("Claude Code CLI를 찾을 수 없습니다. Claude Code를 설치한 뒤 다시 시도하세요.")
  return executable
}

export async function findCodexExecutable(): Promise<string> {
  const executable = await firstExisting([
    process.env.CODEX_CLI_PATH,
    process.env.APPDATA ? join(process.env.APPDATA, "npm", "node_modules", "@openai", "codex", "node_modules", "@openai", "codex-win32-x64", "vendor", "x86_64-pc-windows-msvc", "bin", "codex.exe") : undefined,
  ]) ?? await findOnPath(process.platform === "win32" ? "codex.exe" : "codex")
  if (!executable) throw new Error("Codex CLI를 찾을 수 없습니다. Codex CLI를 설치한 뒤 다시 시도하세요.")
  return executable
}

export function runCommand(executable: string, args: string[], environment: NodeJS.ProcessEnv): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn(executable, args, {
      env: environment,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    })
    let stderr = ""
    child.stderr.on("data", (chunk: Buffer) => { stderr += chunk.toString("utf8") })
    child.once("error", reject)
    child.once("exit", (code) => {
      if (code === 0) resolve()
      else reject(new Error(stderr.trim() || `로그인 프로세스가 종료 코드 ${code ?? "unknown"}로 끝났습니다.`))
    })
  })
}

export function spawnCommand(executable: string, args: string[], environment: NodeJS.ProcessEnv): ChildProcessWithoutNullStreams {
  return spawn(executable, args, {
    env: environment,
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  })
}
