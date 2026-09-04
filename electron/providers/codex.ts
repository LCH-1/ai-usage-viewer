import { shell } from "electron"
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { createInterface } from "node:readline"

import { findCodexExecutable, spawnCommand } from "../command"
import { loadCredential, saveCredential } from "../credential-store"
import type { AccountUsage, UsageMetric } from "../../src/shared/types"

interface RpcMessage {
  id?: number
  method?: string
  result?: unknown
  error?: { message?: string }
  params?: unknown
}

interface PendingRequest {
  resolve: (value: unknown) => void
  reject: (error: Error) => void
}

interface NotificationWaiter {
  method: string
  resolve: (params: unknown) => void
  reject: (error: Error) => void
  timer: NodeJS.Timeout
}

interface CodexAccountResponse {
  account: null | { type: string; email?: string | null; planType?: string }
  requiresOpenaiAuth: boolean
}

interface RateLimitWindow {
  usedPercent: number
  windowDurationMins?: number | null
  resetsAt?: number | null
}

interface RateLimitSnapshot {
  planType?: string | null
  primary?: RateLimitWindow | null
  secondary?: RateLimitWindow | null
  individualLimit?: { remainingPercent: number; resetsAt: number; used: string; limit: string } | null
}

interface RateLimitsResponse {
  rateLimits: RateLimitSnapshot
  rateLimitsByLimitId?: Record<string, RateLimitSnapshot> | null
}

class CodexAppServer {
  private nextId = 1
  private readonly pending = new Map<number, PendingRequest>()
  private readonly waiters = new Set<NotificationWaiter>()
  private readonly child
  private stderr = ""

  constructor(executable: string, codexHome: string) {
    this.child = spawnCommand(executable, ["app-server", "-c", "cli_auth_credentials_store=\"file\""], {
      ...process.env,
      CODEX_HOME: codexHome,
    })
    const lines = createInterface({ input: this.child.stdout })
    lines.on("line", (line) => this.receive(line))
    this.child.stderr.on("data", (chunk: Buffer) => { this.stderr += chunk.toString("utf8") })
    this.child.once("error", (error) => this.failAll(error))
    this.child.once("exit", (code) => {
      if (code && code !== 0) this.failAll(new Error(this.stderr.trim() || `Codex app-server 종료 코드 ${code}`))
    })
  }

  async initialize(): Promise<void> {
    await this.request("initialize", {
      clientInfo: { name: "ai-usage-viewer", title: "Usage Viewer", version: "0.1.3" },
      capabilities: { experimentalApi: false },
    })
    this.notify("initialized")
  }

  request<T>(method: string, params?: unknown): Promise<T> {
    const id = this.nextId
    this.nextId += 1
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: (value) => resolve(value as T), reject })
      this.child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`)
    })
  }

  notify(method: string, params?: unknown): void {
    this.child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method, params })}\n`)
  }

  waitFor<T>(method: string, timeoutMilliseconds = 10 * 60 * 1000): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const waiter: NotificationWaiter = {
        method,
        resolve: (params) => resolve(params as T),
        reject,
        timer: setTimeout(() => {
          this.waiters.delete(waiter)
          reject(new Error("브라우저 로그인 시간이 초과되었습니다."))
        }, timeoutMilliseconds),
      }
      this.waiters.add(waiter)
    })
  }

  async close(): Promise<void> {
    if (this.child.exitCode !== null) return
    await new Promise<void>((resolve) => {
      this.child.once("exit", () => resolve())
      this.child.kill()
      setTimeout(resolve, 2000).unref()
    })
  }

  private receive(line: string): void {
    let message: RpcMessage
    try {
      message = JSON.parse(line) as RpcMessage
    } catch {
      return
    }
    if (typeof message.id === "number") {
      const pending = this.pending.get(message.id)
      if (!pending) return
      this.pending.delete(message.id)
      if (message.error) pending.reject(new Error(message.error.message ?? "Codex app-server 요청 실패"))
      else pending.resolve(message.result)
      return
    }
    if (!message.method) return
    for (const waiter of this.waiters) {
      if (waiter.method !== message.method) continue
      clearTimeout(waiter.timer)
      this.waiters.delete(waiter)
      waiter.resolve(message.params)
    }
  }

  private failAll(error: Error): void {
    for (const pending of this.pending.values()) pending.reject(error)
    this.pending.clear()
    for (const waiter of this.waiters) {
      clearTimeout(waiter.timer)
      waiter.reject(error)
    }
    this.waiters.clear()
  }
}

async function withCodexHome<T>(authFile: string | null, action: (server: CodexAppServer, directory: string) => Promise<T>): Promise<T> {
  const directory = await mkdtemp(join(tmpdir(), "usage-viewer-codex-"))
  let server: CodexAppServer | null = null
  try {
    if (authFile) await writeFile(join(directory, "auth.json"), authFile, { encoding: "utf8", mode: 0o600 })
    const executable = await findCodexExecutable()
    server = new CodexAppServer(executable, directory)
    await server.initialize()
    return await action(server, directory)
  } finally {
    await server?.close()
    await rm(directory, { recursive: true, force: true })
  }
}

function resetText(timestamp: number | null | undefined): string | null {
  return timestamp ? `${new Date(timestamp * 1000).toLocaleString("ko-KR")} 초기화` : null
}

function windowLabel(window: RateLimitWindow, fallback: string): string {
  if (window.windowDurationMins === 300) return "5시간"
  if (window.windowDurationMins === 10_080) return "주간"
  if (window.windowDurationMins) return `${Math.round(window.windowDurationMins / 60)}시간`
  return fallback
}

function windowMetric(id: string, fallback: string, window: RateLimitWindow | null | undefined): UsageMetric | null {
  if (!window) return null
  return {
    id,
    label: windowLabel(window, fallback),
    usedPercent: Math.max(0, Math.min(100, window.usedPercent)),
    resetText: resetText(window.resetsAt),
    detail: null,
  }
}

export async function authenticateCodex(accountId: string): Promise<void> {
  await withCodexHome(null, async (server, directory) => {
    const completed = server.waitFor<{ loginId?: string | null; success: boolean; error?: string | null }>("account/login/completed")
    const login = await server.request<{ type: string; loginId: string; authUrl: string }>("account/login/start", {
      type: "chatgpt",
      useHostedLoginSuccessPage: true,
      appBrand: "codex",
    })
    await shell.openExternal(login.authUrl)
    const result = await completed
    if (!result.success) throw new Error(result.error || "Codex 로그인이 완료되지 않았습니다.")
    await server.request("account/read", { refreshToken: true })
    await new Promise((resolve) => setTimeout(resolve, 150))
    const authFile = await readFile(join(directory, "auth.json"), "utf8")
    await saveCredential(accountId, { provider: "codex", authFile })
  })
}

export async function getCodexUsage(accountId: string): Promise<AccountUsage> {
  const stored = await loadCredential(accountId)
  if (stored.provider !== "codex") throw new Error("저장된 Codex 로그인 정보가 올바르지 않습니다.")
  return withCodexHome(stored.authFile, async (server, directory) => {
    const account = await server.request<CodexAccountResponse>("account/read", { refreshToken: true })
    if (!account.account || account.account.type !== "chatgpt") throw new Error("Codex 로그인이 만료되었습니다. 다시 로그인하세요.")
    const response = await server.request<RateLimitsResponse>("account/rateLimits/read")
    const snapshot = response.rateLimitsByLimitId?.codex ?? response.rateLimits
    const metrics = [
      windowMetric("primary", "기본 한도", snapshot.primary),
      windowMetric("secondary", "보조 한도", snapshot.secondary),
      snapshot.individualLimit ? {
        id: "spend",
        label: "사용 한도",
        usedPercent: Math.max(0, Math.min(100, 100 - snapshot.individualLimit.remainingPercent)),
        resetText: resetText(snapshot.individualLimit.resetsAt),
        detail: `${snapshot.individualLimit.used} / ${snapshot.individualLimit.limit}`,
      } : null,
    ].filter((item): item is UsageMetric => item !== null)
    await new Promise((resolve) => setTimeout(resolve, 150))
    const refreshedAuth = await readFile(join(directory, "auth.json"), "utf8")
    if (refreshedAuth !== stored.authFile) await saveCredential(accountId, { provider: "codex", authFile: refreshedAuth })
    return {
      accountId,
      email: account.account.email ?? null,
      plan: (snapshot.planType ?? account.account.planType)?.toUpperCase() ?? null,
      metrics,
      fetchedAt: new Date().toISOString(),
      sourceUrl: "codex-app-server://account/rateLimits/read",
      warning: metrics.length ? null : "Codex 사용량 항목을 받지 못했습니다.",
    }
  })
}
