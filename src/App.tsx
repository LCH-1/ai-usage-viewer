import { FormEvent, useCallback, useEffect, useMemo, useState } from "react"

import { PROVIDERS, PROVIDER_ORDER } from "./shared/providers"
import type { Account, AccountView, ProviderId, UsageMetric } from "./shared/types"

const EMPTY_ACCOUNTS: AccountView[] = []

function toView(account: Account): AccountView {
  return { ...account, usage: null, error: null, loading: false }
}

function readableError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error)
  return message.replace(/^Error invoking remote method '[^']+': Error: /, "")
}

function Metric({ metric }: { metric: UsageMetric }) {
  const tone = metric.usedPercent >= 90 ? "danger" : metric.usedPercent >= 70 ? "warning" : "safe"
  return (
    <div className="metric">
      <div className="metric-head">
        <span>{metric.label}</span>
        <strong>{metric.usedPercent}%</strong>
      </div>
      <div className="track" aria-label={`${metric.label} ${metric.usedPercent}%`}>
        <div className={`fill ${tone}`} style={{ width: `${metric.usedPercent}%` }} />
      </div>
      <div className="metric-foot">
        <span>{metric.resetText ?? "초기화 정보 없음"}</span>
        {metric.detail && metric.detail !== `${metric.usedPercent}%` ? <span>{metric.detail}</span> : null}
      </div>
    </div>
  )
}

function AccountCard({ account, onRefresh, onAuthenticate, onPortal, onRemove }: {
  account: AccountView
  onRefresh: (id: string) => void
  onAuthenticate: (id: string) => void
  onPortal: (id: string) => void
  onRemove: (id: string) => void
}) {
  const identity = account.usage?.email ?? account.label
  const plan = account.usage?.plan
  return (
    <article className="account-card">
      <div className="account-head">
        <div className="identity">
          <strong title={identity}>{identity}</strong>
          {plan ? <span className="plan">{plan}</span> : <span className="plan muted">확인 필요</span>}
        </div>
        <div className="card-actions">
          <button className="icon-button" onClick={() => onAuthenticate(account.id)} disabled={account.loading} title="기본 브라우저에서 로그인">◇</button>
          <button className="icon-button" onClick={() => onPortal(account.id)} title="사용량 페이지 열기">↗</button>
          <button className="icon-button" onClick={() => onRefresh(account.id)} disabled={account.loading} title="새로고침">
            <span className={account.loading ? "spin" : ""}>↻</span>
          </button>
          <button className="icon-button danger-text" onClick={() => onRemove(account.id)} title="계정 제거">×</button>
        </div>
      </div>

      {account.error ? <div className="notice error">{account.error}</div> : null}
      {account.usage?.warning ? <div className="notice">{account.usage.warning}</div> : null}
      {account.usage?.metrics.length ? (
        <div className="metrics">
          {account.usage.metrics.map((metric) => <Metric key={metric.id} metric={metric} />)}
        </div>
      ) : !account.error && !account.loading ? (
        <button className="login-callout" onClick={() => onAuthenticate(account.id)}>
          기본 브라우저에서 이 계정으로 로그인하기
        </button>
      ) : null}
      {account.loading ? <div className="auth-progress"><div className="loader small" />브라우저 로그인 또는 사용량 확인 중</div> : null}
      {account.usage ? (
        <time className="updated" dateTime={account.usage.fetchedAt}>
          {new Date(account.usage.fetchedAt).toLocaleString("ko-KR")} 확인
        </time>
      ) : null}
    </article>
  )
}

function AddAccountDialog({ open, onClose, onAdd }: {
  open: boolean
  onClose: () => void
  onAdd: (provider: ProviderId, label: string) => Promise<void>
}) {
  const [provider, setProvider] = useState<ProviderId>("claude")
  const [label, setLabel] = useState("")
  const [saving, setSaving] = useState(false)
  if (!open) return null

  async function submit(event: FormEvent) {
    event.preventDefault()
    setSaving(true)
    try {
      await onAdd(provider, label)
      setLabel("")
      onClose()
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <form className="dialog" onSubmit={submit}>
        <div className="dialog-title">
          <div><span className="eyebrow">NEW ACCOUNT</span><h2>계정 추가</h2></div>
          <button type="button" className="icon-button" onClick={onClose}>×</button>
        </div>
        <label className="field-label">플랫폼</label>
        <div className="provider-picker">
          {PROVIDER_ORDER.map((id) => (
            <button type="button" key={id} className={provider === id ? "provider-option selected" : "provider-option"} onClick={() => setProvider(id)}>
              <span className="provider-dot" style={{ background: PROVIDERS[id].color }} />{PROVIDERS[id].name}
            </button>
          ))}
        </div>
        <label className="field-label" htmlFor="account-label">구분 이름</label>
        <input id="account-label" value={label} onChange={(event) => setLabel(event.target.value)} placeholder="예: 개인 Claude, 회사 Claude" autoFocus />
        <p className="field-help">추가 후 기본 브라우저가 열립니다. 같은 Chrome 프로필이라면 기존 계정에서 로그아웃한 뒤 등록할 계정으로 로그인하세요.</p>
        <div className="dialog-actions">
          <button type="button" className="secondary-button" onClick={onClose}>취소</button>
          <button type="submit" className="primary-button" disabled={saving}>{saving ? "추가 중…" : "추가하고 로그인"}</button>
        </div>
      </form>
    </div>
  )
}

export function App() {
  const [accounts, setAccounts] = useState<AccountView[]>(EMPTY_ACCOUNTS)
  const [ready, setReady] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [refreshingAll, setRefreshingAll] = useState(false)
  const [banner, setBanner] = useState<string | null>(null)

  const refreshAccount = useCallback(async (accountId: string) => {
    setAccounts((current) => current.map((item) => item.id === accountId ? { ...item, loading: true, error: null } : item))
    try {
      const usage = await window.usageViewer.refreshAccount(accountId)
      setAccounts((current) => current.map((item) => item.id === accountId ? { ...item, usage, loading: false, error: null } : item))
    } catch (error) {
      setAccounts((current) => current.map((item) => item.id === accountId ? { ...item, loading: false, error: readableError(error) } : item))
    }
  }, [])

  const authenticateAccount = useCallback(async (accountId: string) => {
    setAccounts((current) => current.map((item) => item.id === accountId ? { ...item, loading: true, error: null } : item))
    setBanner("기본 브라우저에서 로그인을 마치세요. 같은 프로필의 다른 계정은 먼저 로그아웃하면 됩니다.")
    try {
      await window.usageViewer.authenticateAccount(accountId)
      setBanner("로그인이 저장되었습니다. 이 계정은 브라우저에서 로그아웃해도 계속 확인할 수 있습니다.")
      await refreshAccount(accountId)
    } catch (error) {
      setAccounts((current) => current.map((item) => item.id === accountId ? { ...item, loading: false, error: readableError(error) } : item))
    }
  }, [refreshAccount])

  useEffect(() => {
    let active = true
    void window.usageViewer.listAccounts().then((items) => {
      if (!active) return
      setAccounts(items.map(toView))
      setReady(true)
      items.forEach((account) => void refreshAccount(account.id))
    }).catch((error) => {
      if (!active) return
      setBanner(readableError(error))
      setReady(true)
    })
    return () => { active = false }
  }, [refreshAccount])

  useEffect(() => {
    if (!ready || accounts.length === 0) return
    const timer = window.setInterval(() => accounts.forEach((account) => void refreshAccount(account.id)), 5 * 60 * 1000)
    return () => window.clearInterval(timer)
  }, [accounts.length, ready, refreshAccount])

  const grouped = useMemo(() => PROVIDER_ORDER.map((provider) => ({
    provider,
    accounts: accounts.filter((account) => account.provider === provider),
  })).filter((group) => group.accounts.length > 0), [accounts])

  async function addNewAccount(provider: ProviderId, label: string) {
    try {
      const account = await window.usageViewer.addAccount(provider, label)
      setAccounts((current) => [...current, toView(account)])
      window.setTimeout(() => void authenticateAccount(account.id), 0)
    } catch (error) {
      setBanner(readableError(error))
      throw error
    }
  }

  async function refreshAll() {
    setRefreshingAll(true)
    await Promise.all(accounts.map((account) => refreshAccount(account.id)))
    setRefreshingAll(false)
  }

  async function remove(id: string) {
    const account = accounts.find((item) => item.id === id)
    if (!account || !window.confirm(`${account.label} 계정과 암호화된 로그인 정보를 제거할까요?`)) return
    try {
      await window.usageViewer.removeAccount(id)
      setAccounts((current) => current.filter((item) => item.id !== id))
    } catch (error) {
      setBanner(readableError(error))
    }
  }

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand"><div className="brand-mark"><img src="/icon.png" alt="" /></div><div><span className="eyebrow">AI LIMITS</span><h1>Usage Viewer</h1></div></div>
        <div className="top-actions">
          <button className="secondary-button compact" onClick={refreshAll} disabled={!accounts.length || refreshingAll}><span className={refreshingAll ? "spin" : ""}>↻</span> 전체 새로고침</button>
          <button className="primary-button compact" onClick={() => setDialogOpen(true)}>＋ 계정</button>
        </div>
      </header>
      {banner ? <button className="banner" onClick={() => setBanner(null)}>{banner}<span>×</span></button> : null}
      {!ready ? <div className="empty-state"><div className="loader" /><p>계정을 불러오는 중입니다</p></div> : null}
      {ready && accounts.length === 0 ? (
        <section className="empty-state">
          <div className="empty-orbit"><span>＋</span></div><h2>첫 계정을 연결하세요</h2>
          <p>같은 플랫폼 계정도 원하는 만큼 추가할 수 있습니다. 로그인은 Windows 기본 브라우저에서 진행됩니다.</p>
          <button className="primary-button" onClick={() => setDialogOpen(true)}>계정 추가</button>
        </section>
      ) : null}
      <div className="provider-list">
        {grouped.map(({ provider, accounts: providerAccounts }) => (
          <section className="provider-section" key={provider}>
            <div className="section-head">
              <div className="section-title"><span className="provider-dot large" style={{ background: PROVIDERS[provider].color }} /><h2>{PROVIDERS[provider].name}</h2><span className="count">{providerAccounts.length}개</span></div>
              <button className="text-button" onClick={() => setDialogOpen(true)}>계정 추가</button>
            </div>
            <div className="cards">
              {providerAccounts.map((account) => (
                <AccountCard key={account.id} account={account} onRefresh={(id) => void refreshAccount(id)} onAuthenticate={(id) => void authenticateAccount(id)} onPortal={(id) => void window.usageViewer.openProviderPortal(id)} onRemove={(id) => void remove(id)} />
              ))}
            </div>
          </section>
        ))}
      </div>
      <footer><span>로그인 토큰은 계정별로 Windows 암호화 저장소에 보관됩니다.</span><span>5분마다 자동 갱신</span></footer>
      <AddAccountDialog open={dialogOpen} onClose={() => setDialogOpen(false)} onAdd={addNewAccount} />
    </main>
  )
}
