import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"

import { WidgetAccountCard, withCachedWidgetUsage } from "./UsageWidget"
import type { AccountUsage, ProviderError } from "./shared/types"

const failure: ProviderError = { code: "authRequired", message: "다시 로그인해 주세요.", retryAt: null, endpoint: null, status: 400 }
const account = { id: "one", provider: "claude" as const, label: "클로드", createdAt: "2026-10-01T00:00:00Z", error: null }
const usage: AccountUsage = {
  accountId: "one", email: null, plan: "MAX",
  metrics: [{ id: "weekly", label: "주간", usedPercent: 100, resetText: null, resetsAt: null, detail: null }],
  fetchedAt: "2026-10-04T09:24:42Z", checkedAt: null, nextRetryAt: null,
  stale: false, error: null, sourceUrl: "test", warning: null,
}

describe("widget account issues", () => {
  it("clears a previous rate-limit warning when another window has refreshed the shared cache successfully", () => {
    const limited = { ...failure, code: "rateLimited", status: 429 }
    const previous = { ...account, usage: { ...usage, stale: true, error: limited }, error: limited }
    const recovered = withCachedWidgetUsage(previous, { ...usage, fetchedAt: "2026-10-06T09:38:42Z" })
    const html = renderToStaticMarkup(<WidgetAccountCard account={recovered} onOpenMain={() => {}} />)
    expect(recovered.error).toBeNull()
    expect(html).not.toContain("조회 제한")
    expect(html).not.toContain("이전 조회값")
    expect(html).toContain("100%")
  })

  it("keeps actual rate limits visible when the shared cache still reports them", () => {
    const limited = { ...failure, code: "rateLimited", status: 429 }
    const cached = withCachedWidgetUsage({ ...account, usage }, { ...usage, stale: true, error: limited })
    const html = renderToStaticMarkup(<WidgetAccountCard account={cached} onOpenMain={() => {}} />)
    expect(html).toContain("조회 제한")
    expect(html).toContain("이전 조회값")
  })

  it("does not dismiss a request failure merely because no cache is available", () => {
    const previous = { ...account, usage: null, error: failure }
    expect(withCachedWidgetUsage(previous, null).error).toEqual(failure)
  })

  it("marks saved usage as previous values when its authentication has expired", () => {
    const html = renderToStaticMarkup(<WidgetAccountCard account={{ ...account, usage: { ...usage, error: failure, stale: true } }} onOpenMain={() => {}} />)
    expect(html).toContain("재로그인 필요")
    expect(html).toContain("이전 조회값")
    expect(html).toContain("100%")
    expect(html).toContain('role="status"')
    expect(html).toContain("문제를 전체 화면에서 확인")
  })

  it("shows request failures even when there is no saved usage", () => {
    const html = renderToStaticMarkup(<WidgetAccountCard account={{ ...account, usage: null, error: { ...failure, code: "network" } }} onOpenMain={() => {}} />)
    expect(html).toContain("조회 실패")
    expect(html).not.toContain("이전 조회값")
  })

  it("marks stale quotas as previous but keeps current quotas current during profile warnings", () => {
    const stale = renderToStaticMarkup(<WidgetAccountCard account={{ ...account, usage: { ...usage, stale: true } }} onOpenMain={() => {}} />)
    const warning = renderToStaticMarkup(<WidgetAccountCard account={{ ...account, usage: { ...usage, warning: "프로필 확인 실패" } }} onOpenMain={() => {}} />)
    expect(stale).toContain("갱신 대기")
    expect(stale).toContain("이전 조회값")
    expect(warning).toContain("확인 필요")
    expect(warning).not.toContain("이전 조회값")
  })

  it("keeps healthy accounts free of issue and previous-value labels", () => {
    const html = renderToStaticMarkup(<WidgetAccountCard account={{ ...account, usage }} onOpenMain={() => {}} />)
    expect(html).not.toContain('role="status"')
    expect(html).not.toContain("이전 조회값")
  })
})
