import { randomBytes, randomUUID, createHash } from "node:crypto"
import { shell } from "electron"

import { loadCredential, saveCredential, type CursorCredential } from "../credential-store"
import { formatResetDate } from "../../src/shared/date"
import type { AccountUsage, UsageMetric } from "../../src/shared/types"

const LOGIN_URL = "https://cursor.com/loginDeepControl"
const API_BASE = "https://api2.cursor.sh"
const CLIENT_ID = "KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB"

interface CursorUsagePayload {
  billingCycleStart?: number | string
  billingCycleEnd?: number | string
  planUsage?: {
    totalPercentUsed?: number
    autoPercentUsed?: number
    apiPercentUsed?: number
    limit?: number
    remaining?: number
  }
}

interface CursorPlanPayload {
  planInfo?: { planName?: string }
  planName?: string
}

interface CursorProfilePayload {
  sub?: string
  email?: string
}

function base64Url(buffer: Buffer): string {
  return buffer.toString("base64").replace(/=/g, "").replace(/\+/g, "-").replace(/\//g, "_")
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds))
}

function cursorUserId(accessToken: string): string | null {
  const encodedPayload = accessToken.split(".")[1]
  if (!encodedPayload) return null
  try {
    const payload = JSON.parse(Buffer.from(encodedPayload, "base64url").toString("utf8")) as { sub?: string }
    return payload.sub?.match(/user_[A-Za-z0-9]+/)?.[0] ?? null
  } catch {
    return null
  }
}

async function getCursorProfile(accessToken: string): Promise<CursorProfilePayload> {
  const userId = cursorUserId(accessToken)
  if (!userId) return {}
  const response = await fetch("https://cursor.com/api/auth/me", {
    headers: {
      Accept: "application/json",
      Cookie: `WorkosCursorSessionToken=${encodeURIComponent(`${userId}::${accessToken}`)}`,
    },
  })
  if (!response.ok) return {}
  const profile = await response.json() as CursorProfilePayload
  if (profile.sub && profile.sub !== userId) return {}
  return profile
}

async function refreshCredential(credential: CursorCredential): Promise<CursorCredential> {
  const response = await fetch(`${API_BASE}/oauth/token`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ grant_type: "refresh_token", client_id: CLIENT_ID, refresh_token: credential.refreshToken }),
  })
  if (!response.ok) return credential
  const body = await response.json() as { access_token?: string; refresh_token?: string; shouldLogout?: boolean }
  if (body.shouldLogout) throw new Error("Cursor 로그인이 만료되었습니다. 다시 로그인하세요.")
  return {
    provider: "cursor",
    accessToken: body.access_token ?? credential.accessToken,
    refreshToken: body.refresh_token ?? credential.refreshToken,
  }
}

async function cursorRequest<T>(path: string, accessToken: string): Promise<T> {
  const response = await fetch(`${API_BASE}/${path}`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${accessToken}`,
      "Content-Type": "application/json",
      "Connect-Protocol-Version": "1",
    },
    body: "{}",
  })
  if (!response.ok) throw new Error(`Cursor API 요청 실패 (${response.status})`)
  return response.json() as Promise<T>
}

function usageMetric(id: string, label: string, value: number | undefined, reset: string | null): UsageMetric | null {
  if (typeof value !== "number") return null
  return { id, label, usedPercent: Math.max(0, Math.min(100, Math.round(value * 10) / 10)), resetText: reset, detail: null }
}

export async function authenticateCursor(accountId: string): Promise<void> {
  const verifier = base64Url(randomBytes(32))
  const challenge = base64Url(createHash("sha256").update(verifier).digest())
  const uuid = randomUUID()
  const login = new URL(LOGIN_URL)
  login.searchParams.set("challenge", challenge)
  login.searchParams.set("uuid", uuid)
  login.searchParams.set("mode", "login")
  login.searchParams.set("redirectTarget", "cli")
  await shell.openExternal(login.toString())

  let wait = 1000
  for (let attempt = 0; attempt < 150; attempt += 1) {
    const poll = new URL(`${API_BASE}/auth/poll`)
    poll.searchParams.set("uuid", uuid)
    poll.searchParams.set("verifier", verifier)
    const response = await fetch(poll)
    if (response.ok) {
      const body = await response.json() as { accessToken?: string; refreshToken?: string }
      if (!body.accessToken || !body.refreshToken) throw new Error("Cursor 로그인 토큰을 받지 못했습니다.")
      await saveCredential(accountId, { provider: "cursor", accessToken: body.accessToken, refreshToken: body.refreshToken })
      return
    }
    if (response.status !== 404) throw new Error(`Cursor 로그인 확인 실패 (${response.status})`)
    await delay(wait)
    wait = Math.min(10_000, Math.round(wait * 1.2))
  }
  throw new Error("Cursor 로그인 시간이 초과되었습니다. 다시 시도하세요.")
}

export async function getCursorUsage(accountId: string): Promise<AccountUsage> {
  const stored = await loadCredential(accountId)
  if (stored.provider !== "cursor") throw new Error("저장된 Cursor 로그인 정보가 올바르지 않습니다.")
  const credential = await refreshCredential(stored)
  await saveCredential(accountId, credential)
  const [usage, plan, profile] = await Promise.all([
    cursorRequest<CursorUsagePayload>("aiserver.v1.DashboardService/GetCurrentPeriodUsage", credential.accessToken),
    cursorRequest<CursorPlanPayload>("aiserver.v1.DashboardService/GetPlanInfo", credential.accessToken).catch((): CursorPlanPayload => ({})),
    getCursorProfile(credential.accessToken).catch((): CursorProfilePayload => ({})),
  ])
  const reset = formatResetDate(usage.billingCycleEnd)
  const metrics = [
    usageMetric("included", "포함 사용량", usage.planUsage?.totalPercentUsed, reset),
    usageMetric("auto", "Auto", usage.planUsage?.autoPercentUsed, reset),
    usageMetric("api", "API 모델", usage.planUsage?.apiPercentUsed, reset),
  ].filter((item): item is UsageMetric => item !== null)
  return {
    accountId,
    email: profile.email?.trim() || null,
    plan: plan.planInfo?.planName?.toUpperCase() ?? plan.planName?.toUpperCase() ?? null,
    metrics,
    fetchedAt: new Date().toISOString(),
    sourceUrl: `${API_BASE}/aiserver.v1.DashboardService/GetCurrentPeriodUsage`,
    warning: metrics.length ? null : "Cursor 사용량 항목을 받지 못했습니다.",
  }
}
