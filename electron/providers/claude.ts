import { mkdtemp, readFile, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { findClaudeExecutable, runCommand } from "../command"
import { loadCredential, saveCredential, type ClaudeCredential } from "../credential-store"
import { parseClaudeUsageMetrics, type ClaudeUsagePayload } from "../../src/shared/claude-usage"
import type { AccountUsage } from "../../src/shared/types"

const CLIENT_ID = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
const TOKEN_URL = "https://platform.claude.com/v1/oauth/token"
const PROFILE_URL = "https://api.anthropic.com/api/oauth/profile"
const USAGE_URL = "https://api.anthropic.com/api/oauth/usage"
const OAUTH_BETA = "oauth-2025-04-20"

interface ClaudeProfilePayload {
  account?: { email?: string }
  organization?: { name?: string }
}

function headers(accessToken: string): Record<string, string> {
  return {
    Authorization: `Bearer ${accessToken}`,
    Accept: "application/json",
    "Content-Type": "application/json",
    "anthropic-beta": OAUTH_BETA,
  }
}

async function requestJson<T>(url: string, accessToken: string): Promise<T> {
  const response = await fetch(url, { headers: headers(accessToken) })
  if (!response.ok) throw new Error(`Claude API 요청 실패 (${response.status})`)
  return response.json() as Promise<T>
}

async function refreshCredential(credential: ClaudeCredential): Promise<ClaudeCredential> {
  if (credential.oauth.expiresAt > Date.now() + 60_000) return credential
  const response = await fetch(TOKEN_URL, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      grant_type: "refresh_token",
      refresh_token: credential.oauth.refreshToken,
      client_id: CLIENT_ID,
      scope: "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload",
    }),
  })
  if (!response.ok) throw new Error("Claude 로그인 갱신에 실패했습니다. 다시 로그인하세요.")
  const body = await response.json() as { access_token: string; refresh_token?: string; expires_in: number }
  return {
    ...credential,
    oauth: {
      ...credential.oauth,
      accessToken: body.access_token,
      refreshToken: body.refresh_token ?? credential.oauth.refreshToken,
      expiresAt: Date.now() + body.expires_in * 1000,
    },
  }
}

export async function authenticateClaude(accountId: string): Promise<void> {
  const directory = await mkdtemp(join(tmpdir(), "usage-viewer-claude-"))
  try {
    const executable = await findClaudeExecutable()
    await runCommand(executable, ["auth", "login", "--claudeai"], { ...process.env, CLAUDE_CONFIG_DIR: directory })
    const raw = JSON.parse(await readFile(join(directory, ".credentials.json"), "utf8")) as {
      claudeAiOauth?: ClaudeCredential["oauth"]
    }
    if (!raw.claudeAiOauth?.accessToken || !raw.claudeAiOauth.refreshToken) {
      throw new Error("Claude 로그인 정보를 가져오지 못했습니다.")
    }
    await saveCredential(accountId, { provider: "claude", oauth: raw.claudeAiOauth })
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
}

export async function getClaudeUsage(accountId: string): Promise<AccountUsage> {
  const stored = await loadCredential(accountId)
  if (stored.provider !== "claude") throw new Error("저장된 Claude 로그인 정보가 올바르지 않습니다.")
  const credential = await refreshCredential(stored)
  if (credential.oauth.accessToken !== stored.oauth.accessToken) await saveCredential(accountId, credential)
  const [usage, profile] = await Promise.all([
    requestJson<ClaudeUsagePayload>(USAGE_URL, credential.oauth.accessToken),
    requestJson<ClaudeProfilePayload>(PROFILE_URL, credential.oauth.accessToken).catch((): ClaudeProfilePayload => ({})),
  ])
  const metrics = parseClaudeUsageMetrics(usage)
  return {
    accountId,
    email: profile.account?.email ?? null,
    plan: credential.oauth.subscriptionType?.toUpperCase() ?? null,
    metrics,
    fetchedAt: new Date().toISOString(),
    sourceUrl: USAGE_URL,
    warning: metrics.length ? null : "Claude 사용량 항목을 받지 못했습니다.",
  }
}
