import { useCallback, useEffect, useState } from "react"
import { isTauri } from "@tauri-apps/api/core"
import { getCurrentWindow } from "@tauri-apps/api/window"

import { providerError } from "./shared/account-state"
import { PROVIDERS, PROVIDER_ORDER } from "./shared/providers"
import { WidgetRequests } from "./shared/widget-requests"
import { usageStatus } from "./shared/usage-status"
import type { Account, AccountUsage, ProviderError } from "./shared/types"
import "./widget.css"

interface WidgetAccount extends Account {
  usage: AccountUsage | null
  error: ProviderError | null
}

export function withCachedWidgetUsage(account: WidgetAccount, usage: AccountUsage | null): WidgetAccount {
  return { ...account, usage, error: usage ? usage.error : account.error }
}

const ICONS = {
  claude: "./provider-icons/claude-symbol.svg",
  codex: "./provider-icons/codex.svg",
  cursor: "./provider-icons/cursor-dark.svg",
}

export function WidgetAccountCard({ account, onOpenMain }: { account: WidgetAccount; onOpenMain: () => void }) {
  const { issue, plan, hideUsage, previousValues, previousLabel } = usageStatus(account)
  return (
    <article className={`widget-account${issue ? " widget-account-issue" : ""}`}>
      <div className="widget-account-head">
        <img src={ICONS[account.provider]} alt={PROVIDERS[account.provider].name} />
        <strong title={account.usage?.email ? `${account.label} · ${account.usage.email}` : account.label}>{account.label}</strong>
        {plan ? <span className="plan">{plan}</span> : null}
      </div>
      {issue ? <div className="widget-account-status" role="status" title={issue.detail}>
        <span>{issue.label}</span>
        <button type="button" onClick={onOpenMain} aria-label={`${account.label} 문제를 전체 화면에서 확인`}>확인 ↗</button>
      </div> : null}
      {issue ? <p className="widget-status-detail">{issue.detail}</p> : null}
      {previousValues && account.usage?.metrics.length ? <p className="widget-cached-label" title={`마지막 조회: ${account.usage.fetchedAt}`}>{previousLabel}</p> : null}
      {!hideUsage && account.usage?.metrics.length ? <div className="widget-metrics">{account.usage.metrics.map((metric) => (
        <div className="widget-metric" key={metric.id} title={metric.resetText ?? undefined}>
          <span className="widget-metric-label">{metric.label}</span>
          <div className="track" aria-label={`${metric.label} ${metric.usedPercent}%`}><div className={`fill ${metric.usedPercent >= 90 ? "danger" : metric.usedPercent >= 70 ? "warning" : "safe"}`} style={{ width: `${metric.usedPercent}%` }} /></div>
          <strong>{metric.usedPercent}%</strong>
        </div>
      ))}</div> : !hideUsage ? <p className="widget-no-usage">아직 표시할 사용량이 없습니다.</p> : null}
    </article>
  )
}

export function UsageWidget() {
  const [accounts, setAccounts] = useState<WidgetAccount[]>([])
  const [version, setVersion] = useState("")
  const [ready, setReady] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [listing, setListing] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [requests] = useState(() => new WidgetRequests())

  const pause = useCallback(() => {
    requests.pause()
    setRefreshing(false)
    setListing(false)
  }, [requests])

  const refreshAccount = useCallback(async (accountId: string, force: boolean) => {
    const ticket = requests.beginAccount(accountId)
    if (!ticket) return
    setRefreshing(true)
    try {
      if (isTauri() && !(await getCurrentWindow().isVisible())) {
        if (requests.isCurrent(ticket)) pause()
        return
      }
      if (!requests.isCurrent(ticket)) return
      const usage = await window.usageViewer.refreshAccount(accountId, force)
      if (!requests.isCurrent(ticket)) return
      setAccounts((items) => items.map((item) => item.id === accountId ? { ...item, usage, error: usage.error } : item))
    } catch (error) {
      if (!requests.isCurrent(ticket)) return
      const failure = providerError(error)
      if (failure.code === "notFound") {
        setAccounts((items) => items.filter((item) => item.id !== accountId))
      } else if (failure.code !== "cancelled") {
        setAccounts((items) => items.map((item) => item.id === accountId ? { ...item, error: failure } : item))
      }
    } finally {
      requests.finishAccount(accountId, ticket)
      if (requests.isCurrent(ticket)) setRefreshing(requests.refreshing)
    }
  }, [pause, requests])

  const refresh = useCallback(async (force = false) => {
    if (isTauri()) {
      try {
        if (!(await getCurrentWindow().isVisible())) { pause(); return }
      } catch { return }
    }
    requests.resume()
    const ticket = requests.beginListing()
    if (!ticket) return
    setListing(true)
    try {
      if (isTauri() && !(await getCurrentWindow().isVisible())) {
        if (requests.isCurrent(ticket)) pause()
        return
      }
      if (!requests.isCurrent(ticket)) return
      const listed = await window.usageViewer.listAccounts()
      if (!requests.isCurrent(ticket)) return
      const ordered = PROVIDER_ORDER.flatMap((provider) => listed.filter((account) => account.provider === provider))
      setAccounts((items) => ordered.map((account) => {
        const previous = items.find((item) => item.id === account.id)
        return { ...account, usage: previous?.usage ?? null, error: previous?.error ?? null }
      }))
      setReady(true)
      setMessage(null)
      await Promise.all(ordered.map(async (account) => {
        if (!requests.isCurrent(ticket) || requests.hasAccount(account.id)) return
        try {
          const usage = await window.usageViewer.getCachedUsage(account.id)
          if (!requests.isCurrent(ticket)) return
          setAccounts((items) => items.map((item) => item.id === account.id ? withCachedWidgetUsage(item, usage) : item))
        } catch (error) {
          if (!requests.isCurrent(ticket)) return
          const failure = providerError(error)
          if (failure.code === "notFound") {
            setAccounts((items) => items.filter((item) => item.id !== account.id))
            return
          }
          if (failure.code !== "cancelled") {
            setAccounts((items) => items.map((item) => item.id === account.id ? { ...item, error: failure } : item))
          }
        }
        if (requests.isCurrent(ticket)) void refreshAccount(account.id, force)
      }))
    } catch {
      if (requests.isCurrent(ticket)) {
        setReady(true)
        setMessage("계정을 불러오지 못했습니다. 전체 화면에서 확인해 주세요.")
      }
    } finally {
      requests.finishListing(ticket)
      if (requests.isCurrent(ticket)) setListing(false)
    }
  }, [pause, refreshAccount, requests])

  const hide = useCallback(() => {
    pause()
    void window.usageViewer.hideWidget().catch(() => {
      requests.resume()
      setMessage("창을 닫지 못했습니다. 다시 시도해 주세요.")
    })
  }, [pause, requests])

  useEffect(() => {
    let active = true
    requests.resume()
    void window.usageViewer.getAppVersion().then((value) => {
      if (active) setVersion(value)
    }).catch(() => undefined)
    void refresh()
    const onFocus = () => { requests.resume(); void refresh() }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") hide()
    }
    const timer = window.setInterval(() => void refresh(), 5_000)
    window.addEventListener("focus", onFocus)
    window.addEventListener("keydown", onKeyDown)
    let unlisten: (() => void) | undefined
    if (isTauri()) {
      void getCurrentWindow().onFocusChanged(({ payload }) => {
        if (!active) return
        if (payload) onFocus()
      }).then((remove) => {
        if (active) unlisten = remove
        else remove()
      }).catch(() => undefined)
    }
    return () => {
      active = false
      requests.pause()
      unlisten?.()
      window.clearInterval(timer)
      window.removeEventListener("focus", onFocus)
      window.removeEventListener("keydown", onKeyDown)
    }
  }, [hide, refresh, requests])

  function openMain() {
    pause()
    void window.usageViewer.showMainWindow().catch(() => {
      requests.resume()
      setMessage("전체 화면을 열지 못했습니다. 다시 시도해 주세요.")
    })
  }

  return (
    <section className="usage-widget" aria-label="간략 사용량">
      <header className="widget-header">
        <div className="widget-brand"><img src="./icon.png" alt="" /><strong>Usage Viewer</strong>{version ? <span>v{version}</span> : null}</div>
        <div className="widget-actions">
          <button type="button" className="icon-button" title="새로고침" aria-label="사용량 새로고침" disabled={listing} onClick={() => void refresh(true)}><span className={refreshing || listing ? "spin" : ""}>↻</span></button>
          <button type="button" className="icon-button" title="닫기" aria-label="간략 사용량 닫기" onClick={hide}>×</button>
        </div>
      </header>
      <div className="widget-body">
        {message ? <p className="widget-empty" role="status">{message}</p> : null}
        {!ready ? <div className="widget-empty"><div className="loader small" />사용량 불러오는 중</div> : null}
        {ready && !message && accounts.length === 0 ? <div className="widget-empty">연결된 계정이 없습니다.<button type="button" className="secondary-button compact" onClick={openMain}>계정 연결하기</button></div> : null}
        {accounts.map((account) => <WidgetAccountCard key={account.id} account={account} onOpenMain={openMain} />)}
      </div>
      <footer className="widget-footer"><button type="button" onClick={openMain}>전체 화면 열기 <span aria-hidden="true">↗</span></button></footer>
    </section>
  )
}
