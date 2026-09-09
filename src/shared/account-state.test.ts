import { describe, expect, it } from "vitest"

import { canRefreshAccount, providerError, resetNeedsVerification } from "./account-state"
import type { AccountView } from "./types"

const account: AccountView = {
  id: "one",
  provider: "claude",
  label: "One",
  createdAt: "2026-09-07T00:00:00Z",
  usage: null,
  error: null,
  refreshing: false,
  authenticating: false,
  cancelling: false,
  removing: false,
}

describe("account recovery state", () => {
  it("waits for the provider retry time and keeps authentication errors out of automatic polling", () => {
    const now = Date.parse("2026-09-07T00:00:00Z")
    const error = providerError({ code: "rateLimited", message: "Wait", retryAt: "2026-09-07T00:01:00Z" })
    expect(canRefreshAccount({ ...account, error }, now)).toBe(false)
    expect(canRefreshAccount({ ...account, error }, now, true)).toBe(true)
    expect(canRefreshAccount({ ...account, authenticating: true }, now, true)).toBe(false)
    expect(canRefreshAccount({ ...account, error }, now + 60_000)).toBe(true)
    expect(canRefreshAccount({ ...account, error: { ...error, code: "authRequired", retryAt: null } }, now)).toBe(false)
    expect(canRefreshAccount({ ...account, authenticating: true }, now)).toBe(false)
    expect(canRefreshAccount({ ...account, cancelling: true }, now)).toBe(false)
    expect(canRefreshAccount({ ...account, removing: true }, now)).toBe(false)
  })

  it("treats a past reset as unverified without changing the previous percentage", () => {
    const metric = { id: "weekly", label: "주간", usedPercent: 100, resetText: "초기화", resetsAt: "2026-09-07T00:00:00Z", detail: null }
    expect(resetNeedsVerification(metric, Date.parse("2026-09-07T00:00:01Z"))).toBe(true)
    expect(resetNeedsVerification({ ...metric, resetsAt: null }, Date.now())).toBe(false)
    expect(metric.usedPercent).toBe(100)
  })

  it("preserves structured authentication errors while accepting older string failures", () => {
    expect(providerError({ code: "authRequired", message: "로그인 필요", status: 401 }).code).toBe("authRequired")
    expect(providerError("저장 실패").message).toBe("저장 실패")
    expect(providerError(new Error("offline")).message).toBe("offline")
  })
})
