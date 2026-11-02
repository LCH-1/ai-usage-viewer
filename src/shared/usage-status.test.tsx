import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"

import { AccountCard } from "../App"
import { WidgetAccountCard } from "../UsageWidget"
import { usageStatus } from "./usage-status"
import type { AccountView } from "./types"

const account: AccountView = {
  id: "one", provider: "claude", label: "계정", createdAt: "2026-10-01T00:00:00Z",
  error: null, refreshing: false, authenticating: false, cancelling: false, removing: false,
  usage: {
    accountId: "one", email: null, plan: "MAX", fetchedAt: "2026-10-06T11:30:00Z",
    checkedAt: "2026-10-06T11:31:00Z", nextRetryAt: "2026-10-06T11:41:00Z",
    metrics: [{ id: "weekly", label: "주간", usedPercent: 41, resetText: null, resetsAt: null, detail: null }],
    stale: true, error: { code: "rateLimited", message: "limited", retryAt: null, status: 429, endpoint: "claude.usage" },
    sourceUrl: "test", warning: null,
  },
}

describe("shared usage status", () => {
  it.each([
    ["accessDenied", "구독·권한 확인"],
    ["subscriptionRequired", "구독 비활성"],
    ["authRequired", "구독·권한 확인"],
  ])("hides old paid quotas for %s in both windows", (code, label) => {
    const denied = { ...account, error: { ...account.usage!.error!, code, message: "구독을 확인하세요.", status: 403 } }
    for (const html of [
      renderToStaticMarkup(<AccountCard account={denied} onEdit={() => {}} onCancelAuthentication={() => {}} />),
      renderToStaticMarkup(<WidgetAccountCard account={denied} onOpenMain={() => {}} />),
    ]) {
      expect(html).toContain(label)
      expect(html).not.toContain("재로그인 필요")
      expect(html).not.toContain("MAX")
      expect(html).not.toContain("41%")
      expect(html).not.toContain("이전 조회값")
      if (code === "subscriptionRequired") expect(html).toContain("FREE")
    }
  })

  it("renders a weekly-only account without a session placeholder", () => {
    const healthy = { ...account, usage: { ...account.usage!, stale: false, error: null } }
    for (const html of [
      renderToStaticMarkup(<AccountCard account={healthy} onEdit={() => {}} onCancelAuthentication={() => {}} />),
      renderToStaticMarkup(<WidgetAccountCard account={healthy} onOpenMain={() => {}} />),
    ]) {
      expect(html).toContain("주간")
      expect(html).not.toContain("5시간")
      expect(html).not.toContain("재로그인 필요")
    }
  })

  it("shows the same actual limit, retry time and previous-value label in both windows", () => {
    const { issue, previousLabel } = usageStatus(account)
    const main = renderToStaticMarkup(<AccountCard account={account} onEdit={() => {}} onCancelAuthentication={() => {}} />)
    const widget = renderToStaticMarkup(<WidgetAccountCard account={account} onOpenMain={() => {}} />)
    for (const html of [main, widget]) {
      expect(html).toContain(issue!.label)
      expect(html).toContain(issue!.detail)
      expect(html).toContain("자동 재시도")
      expect(html).toContain(previousLabel)
      expect(html).toContain("41%")
    }
  })

  it("clears warnings in both windows after a healthy shared result", () => {
    const healthy = { ...account, usage: { ...account.usage!, stale: false, error: null, nextRetryAt: null } }
    expect(usageStatus(healthy).issue).toBeNull()
    for (const html of [
      renderToStaticMarkup(<AccountCard account={healthy} onEdit={() => {}} onCancelAuthentication={() => {}} />),
      renderToStaticMarkup(<WidgetAccountCard account={healthy} onOpenMain={() => {}} />),
    ]) {
      expect(html).not.toContain("조회 제한")
      expect(html).not.toContain("이전 조회값")
    }
  })

  it("distinguishes expired authentication from a delayed usage lookup", () => {
    const expired = { ...account, error: { ...account.usage!.error!, code: "authRequired", message: "로그인 만료" } }
    expect(usageStatus(expired).issue?.label).toBe("재로그인 필요")
    expect(usageStatus(expired).issue?.detail).not.toContain("자동 재시도")
    expect(usageStatus({ ...account, error: { ...expired.error, code: "temporary" } }).issue?.label).toBe("조회 대기")
  })
})
