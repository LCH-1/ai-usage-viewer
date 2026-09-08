import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react"
import { isTauri } from "@tauri-apps/api/core"
import { getCurrentWindow } from "@tauri-apps/api/window"

import { AccountRequestCoordinator } from "./shared/account-requests"
import { canRefreshAccount, providerError } from "./shared/account-state"
import { PROVIDERS, PROVIDER_ORDER } from "./shared/providers"
import type { Account, AccountView, ProviderId, UpdateInfo, UsageMetric } from "./shared/types"

const EMPTY_ACCOUNTS: AccountView[] = []

function getApplicationWindow() {
  return isTauri() ? getCurrentWindow() : null
}

function toView(account: Account): AccountView {
  return { ...account, usage: null, error: null, refreshing: false, authenticating: false, cancelling: false, removing: false }
}

function readableError(error: unknown): string {
  return providerError(error).message
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
        <span title={metric.resetText ?? undefined}>{metric.resetText ?? "초기화 정보 없음"}</span>
        {metric.detail && metric.detail !== `${metric.usedPercent}%` ? <span>{metric.detail}</span> : null}
      </div>
    </div>
  )
}

function ProviderIcon({ provider }: { provider: ProviderId }) {
  const source = provider === "claude"
    ? "./provider-icons/claude-symbol.svg"
    : provider === "cursor" ? "./provider-icons/cursor-dark.svg" : "./provider-icons/codex.svg"
  return <span className={`provider-icon ${provider}-icon`} aria-hidden="true"><img src={source} alt="" /></span>
}

function LoginIcon() {
  return <svg className="action-icon" viewBox="0 0 20 20" aria-hidden="true"><path d="M8 4H4.8A1.8 1.8 0 0 0 3 5.8v8.4A1.8 1.8 0 0 0 4.8 16H8M12.5 6.5 16 10l-3.5 3.5M7 10h9" /></svg>
}

function EditIcon() {
  return <svg className="action-icon" viewBox="0 0 20 20" aria-hidden="true"><path d="m13.9 3.6 2.5 2.5M5 15l2.9-.6 8.5-8.5a1.77 1.77 0 0 0-2.5-2.5L5.4 11.9 5 15Z" /></svg>
}

function ExternalLinkIcon() {
  return <svg className="action-icon" viewBox="0 0 20 20" aria-hidden="true"><path d="M11 4h5v5M16 4l-7 7M15 11v3.2a1.8 1.8 0 0 1-1.8 1.8H5.8A1.8 1.8 0 0 1 4 14.2V6.8A1.8 1.8 0 0 1 5.8 5H9" /></svg>
}

function WindowTitlebar({ version }: { version: string | null }) {
  return (
    <div className="window-titlebar" data-tauri-drag-region onDoubleClick={() => void getApplicationWindow()?.toggleMaximize()}>
      <div className="window-title" data-tauri-drag-region><img src="./icon.png" alt="" /><span data-tauri-drag-region>Usage Viewer</span>{version ? <span className="app-version" data-tauri-drag-region>v{version}</span> : null}</div>
      <div className="window-controls">
        <button type="button" onDoubleClick={(event) => event.stopPropagation()} onClick={() => void getApplicationWindow()?.minimize()} title="최소화" aria-label="최소화"><svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2 8.5h8" /></svg></button>
        <button type="button" onDoubleClick={(event) => event.stopPropagation()} onClick={() => void getApplicationWindow()?.toggleMaximize()} title="최대화" aria-label="최대화"><svg viewBox="0 0 12 12" aria-hidden="true"><rect x="2.5" y="2.5" width="7" height="7" rx=".5" /></svg></button>
        <button type="button" className="window-close" onDoubleClick={(event) => event.stopPropagation()} onClick={() => void getApplicationWindow()?.close()} title="닫기" aria-label="닫기"><svg viewBox="0 0 12 12" aria-hidden="true"><path d="m2.5 2.5 7 7m0-7-7 7" /></svg></button>
      </div>
    </div>
  )
}

function AccountCard({ account, onCancelAuthentication, onEdit }: {
  account: AccountView
  onCancelAuthentication: (id: string) => void
  onEdit: (id: string) => void
}) {
  const plan = account.usage?.plan
  const visibleError = account.error && !["rateLimited", "temporary", "cancelled", "authenticating"].includes(account.error.code) ? account.error : null
  return (
    <article className="account-card">
      <div className="account-head">
        <div className="identity">
          <div className="identity-primary">
            <strong title={account.label}>{account.label}</strong>
            {plan ? <span className="plan">{plan}</span> : <span className="plan muted">확인 필요</span>}
            {account.usage?.email ? <span className="account-email" title={account.usage.email}>{account.usage.email}</span> : null}
          </div>
        </div>
        <div className="card-actions">
          <button className="icon-button" onClick={() => onEdit(account.id)} disabled={account.removing} title="계정 수정"><EditIcon /></button>
        </div>
      </div>

      {visibleError ? <div className="notice error" role="status">{visibleError.message}</div> : null}
      {account.usage?.metrics.length ? (
        <div className="metrics">
          {account.usage.metrics.map((metric) => <Metric key={metric.id} metric={metric} />)}
        </div>
      ) : !account.usage && !account.error && !account.refreshing && !account.authenticating && !account.cancelling && !account.removing ? (
        <button className="login-callout" onClick={() => onEdit(account.id)}>
          계정 수정에서 로그인하기
        </button>
      ) : null}
      {account.authenticating || account.cancelling ? (
        <div className="auth-progress" role="status"><div className="loader small" />{account.cancelling ? "로그인 취소 중" : "브라우저에서 로그인 중"}<button className="text-button" onClick={() => onCancelAuthentication(account.id)} disabled={account.cancelling || account.removing}>로그인 취소</button></div>
      ) : account.refreshing && !account.usage ? <div className="auth-progress" role="status"><div className="loader small" />사용량 불러오는 중</div> : null}
    </article>
  )
}

function EditAccountDialog({ account, onClose, onSave, onRemove, onPortal, onAuthenticate, onCancelAuthentication }: {
  account: AccountView
  onClose: () => void
  onSave: (accountId: string, label: string) => Promise<boolean>
  onRemove: (accountId: string) => Promise<boolean>
  onPortal: (accountId: string) => void
  onAuthenticate: (accountId: string) => void
  onCancelAuthentication: (accountId: string) => void
}) {
  const [label, setLabel] = useState(account.label)
  const [saving, setSaving] = useState(false)

  async function submit(event: FormEvent) {
    event.preventDefault()
    const nextLabel = label.trim()
    if (!nextLabel) return
    setSaving(true)
    try {
      if (await onSave(account.id, nextLabel)) onClose()
    } finally {
      setSaving(false)
    }
  }

  async function remove() {
    setSaving(true)
    try {
      if (await onRemove(account.id)) onClose()
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <form className="dialog" onSubmit={submit}>
        <div className="dialog-title">
          <div><span className="eyebrow">EDIT ACCOUNT</span><h2>계정 수정</h2></div>
          <button type="button" className="icon-button" onClick={onClose}>×</button>
        </div>
        <div className="edit-account-provider">
          <div className="edit-account-identity">
            <ProviderIcon provider={account.provider} />
            <div className="edit-account-details"><strong>{PROVIDERS[account.provider].name}</strong>{account.usage?.email ? <span title={account.usage.email}>{account.usage.email}</span> : null}</div>
          </div>
          <div className="account-login-actions">
            <button type="button" className="provider-link-button" title="사용량 보러가기" aria-label="사용량 보러가기" onClick={() => onPortal(account.id)}><ExternalLinkIcon />사용량</button>
            <button type="button" className="provider-link-button" onClick={() => onAuthenticate(account.id)} disabled={saving || account.authenticating || account.cancelling || account.removing}><LoginIcon />다시 로그인</button>
          </div>
        </div>
        <p className="field-help portal-help">브라우저에 로그인된 계정의 페이지가 열립니다. 확인할 계정: <strong>{account.usage?.email ?? account.label}</strong>. 다른 계정이 보이면 브라우저에서 계정을 전환하세요.</p>
        {account.authenticating || account.cancelling ? <div className="auth-progress" role="status"><div className="loader small" />{account.cancelling ? "로그인 취소 중" : "브라우저에서 로그인 중"}<button type="button" className="text-button" onClick={() => onCancelAuthentication(account.id)} disabled={account.cancelling || saving}>로그인 취소</button></div> : null}
        <label className="field-label" htmlFor="edit-account-label">계정 이름</label>
        <input id="edit-account-label" value={label} onChange={(event) => setLabel(event.target.value)} autoFocus />
        <div className="dialog-actions split-actions">
          <button type="button" className="danger-button" onClick={() => void remove()} disabled={saving}>계정 삭제</button>
          <div>
            <button type="button" className="secondary-button" onClick={onClose}>취소</button>
            <button type="submit" className="primary-button" disabled={saving || !label.trim()}>저장</button>
          </div>
        </div>
      </form>
    </div>
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
  const [error, setError] = useState<string | null>(null)
  if (!open) return null

  async function submit(event: FormEvent) {
    event.preventDefault()
    setSaving(true)
    setError(null)
    try {
      await onAdd(provider, label)
      setLabel("")
      onClose()
    } catch (error) {
      setError(readableError(error))
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
              <ProviderIcon provider={id} />{PROVIDERS[id].name}
            </button>
          ))}
        </div>
        <label className="field-label" htmlFor="account-label">구분 이름</label>
        <input id="account-label" value={label} onChange={(event) => setLabel(event.target.value)} placeholder="예: 개인 Claude, 회사 Claude" autoFocus />
        <p className="field-help">추가 후 기본 브라우저가 열립니다. 같은 Chrome 프로필이라면 기존 계정에서 로그아웃한 뒤 등록할 계정으로 로그인하세요.</p>
        {error ? <div className="notice error" role="alert">{error}</div> : null}
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
  const accountsRef = useRef<AccountView[]>(EMPTY_ACCOUNTS)
  const [requests] = useState(() => new AccountRequestCoordinator())
  const [now, setNow] = useState(Date.now)
  const [ready, setReady] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editingAccountId, setEditingAccountId] = useState<string | null>(null)
  const [refreshingAll, setRefreshingAll] = useState(false)
  const [banner, setBanner] = useState<string | null>(null)
  const [appVersion, setAppVersion] = useState<string | null>(null)
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null)
  const [dismissedUpdateVersion, setDismissedUpdateVersion] = useState(() => window.localStorage.getItem("dismissed-update-version"))

  const updateAccounts = useCallback((update: (current: AccountView[]) => AccountView[]) => {
    const next = update(accountsRef.current)
    accountsRef.current = next
    setAccounts(next)
  }, [])

  const refreshAccount = useCallback((accountId: string, force = false) => {
    const account = accountsRef.current.find((item) => item.id === accountId)
    if (!account || !canRefreshAccount(account, Date.now())) return Promise.resolve()
    return requests.refresh(accountId, async (isCurrent) => {
      updateAccounts((current) => current.map((item) => item.id === accountId ? { ...item, refreshing: true } : item))
      try {
        const usage = await window.usageViewer.refreshAccount(accountId, force)
        if (!isCurrent()) return
        updateAccounts((current) => current.map((item) => item.id === accountId ? { ...item, usage, refreshing: false, error: usage.error } : item))
      } catch (error) {
        if (!isCurrent()) return
        updateAccounts((current) => current.map((item) => item.id === accountId ? { ...item, refreshing: false, error: providerError(error) } : item))
      }
    })
  }, [requests, updateAccounts])

  const authenticateAccount = useCallback(async (accountId: string) => {
    const account = accountsRef.current.find((item) => item.id === accountId)
    if (!account || account.cancelling || account.removing) return
    let succeeded = false
    await requests.authenticate(accountId, async (isCurrent) => {
      updateAccounts((current) => current.map((item) => item.id === accountId ? { ...item, authenticating: true, refreshing: false, error: null } : item))
      setBanner(`${account.label}: 기본 브라우저에서 로그인을 마치세요. 등록할 계정이 맞는지 확인해 주세요.`)
      try {
        await window.usageViewer.authenticateAccount(accountId)
        if (!isCurrent()) return
        succeeded = true
        updateAccounts((current) => current.map((item) => item.id === accountId ? { ...item, usage: null, authenticating: false, error: null } : item))
        setBanner(`${account.label}: 로그인이 저장되었습니다. 사용량을 확인합니다.`)
      } catch (error) {
        if (!isCurrent()) return
        updateAccounts((current) => current.map((item) => item.id === accountId ? { ...item, authenticating: false, error: providerError(error) } : item))
        setBanner(`${account.label}: ${readableError(error)}`)
      }
    })
    if (succeeded) await refreshAccount(accountId, true)
  }, [requests, refreshAccount, updateAccounts])

  const cancelAuthentication = useCallback(async (accountId: string) => {
    const account = accountsRef.current.find((item) => item.id === accountId)
    if (!account || account.cancelling || account.removing) return
    requests.pause(accountId)
    updateAccounts((current) => current.map((item) => item.id === accountId ? { ...item, authenticating: false, refreshing: false, cancelling: true } : item))
    try {
      await window.usageViewer.cancelAuthentication(accountId)
      setBanner(`${account.label}: 로그인을 취소했습니다.`)
    } catch (error) {
      updateAccounts((current) => current.map((item) => item.id === accountId ? { ...item, error: providerError(error) } : item))
      setBanner(readableError(error))
    } finally {
      updateAccounts((current) => current.map((item) => item.id === accountId ? { ...item, cancelling: false } : item))
      requests.resume(accountId)
    }
  }, [requests, updateAccounts])

  useEffect(() => {
    let active = true
    void window.usageViewer.listAccounts().then((items) => {
      if (!active) return
      updateAccounts(() => items.map(toView))
      setReady(true)
      items.forEach((account) => {
        void requests.refresh(account.id, async (isCurrent) => {
          updateAccounts((current) => current.map((item) => item.id === account.id ? { ...item, refreshing: true } : item))
          try {
            const usage = await window.usageViewer.getCachedUsage(account.id)
            if (!active || !isCurrent()) return
            updateAccounts((current) => current.map((item) => item.id === account.id ? { ...item, usage, error: usage?.error ?? null, refreshing: false } : item))
          } catch (error) {
            if (!active || !isCurrent()) return
            updateAccounts((current) => current.map((item) => item.id === account.id ? { ...item, error: providerError(error), refreshing: false } : item))
          }
        }).then(() => { if (active) void refreshAccount(account.id) })
      })
    }).catch((error) => {
      if (!active) return
      setBanner(readableError(error))
      setReady(true)
    })
    return () => { active = false }
  }, [requests, refreshAccount, updateAccounts])

  useEffect(() => {
    if (!ready) return
    let active = true
    let checkingVisibility = false
    async function poll() {
      setNow(Date.now())
      if (checkingVisibility || document.visibilityState === "hidden") return
      checkingVisibility = true
      try {
        const currentWindow = getApplicationWindow()
        if (currentWindow && (!(await currentWindow.isVisible()) || await currentWindow.isMinimized())) return
        if (active) accountsRef.current.forEach((account) => void refreshAccount(account.id))
      } catch (error) {
        if (active) setBanner(readableError(error))
      } finally {
        checkingVisibility = false
      }
    }
    const timer = window.setInterval(() => void poll(), 10 * 1000)
    const onVisible = () => { void poll() }
    window.addEventListener("focus", onVisible)
    document.addEventListener("visibilitychange", onVisible)
    return () => {
      active = false
      window.clearInterval(timer)
      window.removeEventListener("focus", onVisible)
      document.removeEventListener("visibilitychange", onVisible)
    }
  }, [ready, refreshAccount])

  useEffect(() => {
    let active = true
    void window.usageViewer.getAppVersion()
      .then((version) => { if (active) setAppVersion(version) })
      .catch(() => undefined)
    return () => { active = false }
  }, [])

  useEffect(() => {
    let active = true
    function check() {
      void window.usageViewer.checkForUpdate()
        .then((info) => { if (active) setUpdateInfo(info) })
        .catch(() => undefined)
    }
    check()
    const timer = window.setInterval(check, 6 * 60 * 60 * 1000)
    return () => {
      active = false
      window.clearInterval(timer)
    }
  }, [])

  const grouped = useMemo(() => PROVIDER_ORDER.map((provider) => ({
    provider,
    accounts: accounts.filter((account) => account.provider === provider),
  })).filter((group) => group.accounts.length > 0), [accounts])

  async function addNewAccount(provider: ProviderId, label: string) {
    try {
      const account = await window.usageViewer.addAccount(provider, label)
      updateAccounts((current) => [...current, toView(account)])
      window.setTimeout(() => void authenticateAccount(account.id), 0)
    } catch (error) {
      setBanner(readableError(error))
      throw error
    }
  }

  async function refreshAll() {
    if (refreshingAll) return
    setRefreshingAll(true)
    try {
      await Promise.all(accountsRef.current.map((account) => refreshAccount(account.id, true)))
    } finally {
      setRefreshingAll(false)
    }
  }

  async function remove(id: string): Promise<boolean> {
    const account = accountsRef.current.find((item) => item.id === id)
    if (!account || !window.confirm(`${account.label} 계정과 암호화된 로그인 정보를 제거할까요?`)) return false
    requests.pause(id)
    updateAccounts((current) => current.map((item) => item.id === id ? { ...item, removing: true, authenticating: false, refreshing: false } : item))
    try {
      await window.usageViewer.removeAccount(id)
      updateAccounts((current) => current.filter((item) => item.id !== id))
      return true
    } catch (error) {
      requests.resume(id)
      updateAccounts((current) => current.map((item) => item.id === id ? { ...item, removing: false, error: providerError(error) } : item))
      setBanner(readableError(error))
      return false
    }
  }

  async function rename(id: string, label: string): Promise<boolean> {
    try {
      const renamed = await window.usageViewer.renameAccount(id, label)
      updateAccounts((current) => current.map((item) => item.id === id ? { ...item, label: renamed.label } : item))
      return true
    } catch (error) {
      setBanner(readableError(error))
      return false
    }
  }

  const editingAccount = editingAccountId ? accounts.find((account) => account.id === editingAccountId) ?? null : null
  const updateAvailable = updateInfo?.available && updateInfo.latestVersion !== dismissedUpdateVersion

  function dismissUpdate() {
    if (!updateInfo) return
    window.localStorage.setItem("dismissed-update-version", updateInfo.latestVersion)
    setDismissedUpdateVersion(updateInfo.latestVersion)
  }

  return (
    <div className="app-frame">
      <WindowTitlebar version={appVersion} />
      <div className="app-scroll">
      <main className="app-shell">
      <header className="topbar">
        <div className="brand"><div className="brand-mark"><img src="./icon.png" alt="" /></div><div><span className="eyebrow">AI LIMITS</span><h1>Usage Viewer</h1></div></div>
        <div className="top-actions">
          <button className="secondary-button compact" onClick={() => void refreshAll()} disabled={refreshingAll || !accounts.some((account) => canRefreshAccount(account, now))}><span className={refreshingAll ? "spin" : ""}>↻</span> 새로고침</button>
          <button className="primary-button compact" onClick={() => setDialogOpen(true)}>＋ 계정</button>
        </div>
      </header>
      {updateAvailable ? (
        <aside className="update-banner">
          <div><strong>새 버전 {updateInfo.latestVersion}이 있습니다</strong><span>현재 버전 {updateInfo.currentVersion}</span></div>
          <div className="update-actions">
            <button className="update-link" onClick={() => void window.usageViewer.openLatestRelease().catch((error) => setBanner(readableError(error)))}>업데이트 받기</button>
            <button className="update-dismiss" onClick={dismissUpdate} title="이번 버전 알림 닫기" aria-label="이번 버전 알림 닫기">×</button>
          </div>
        </aside>
      ) : null}
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
              <div className="section-title"><ProviderIcon provider={provider} /><h2>{PROVIDERS[provider].name}</h2><span className="count">{providerAccounts.length}개</span></div>
            </div>
            <div className="cards">
              {providerAccounts.map((account) => (
                <AccountCard key={account.id} account={account} onCancelAuthentication={(id) => void cancelAuthentication(id)} onEdit={setEditingAccountId} />
              ))}
            </div>
          </section>
        ))}
      </div>
      <AddAccountDialog open={dialogOpen} onClose={() => setDialogOpen(false)} onAdd={addNewAccount} />
      {editingAccount ? <EditAccountDialog key={editingAccount.id} account={editingAccount} onClose={() => setEditingAccountId(null)} onSave={rename} onRemove={remove} onAuthenticate={(id) => void authenticateAccount(id)} onCancelAuthentication={(id) => void cancelAuthentication(id)} onPortal={(id) => void window.usageViewer.openProviderPortal(id).catch((error) => setBanner(readableError(error)))} /> : null}
      </main>
      </div>
    </div>
  )
}
