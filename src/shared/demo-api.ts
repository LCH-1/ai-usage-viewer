import type { Account, AccountUsage, ProviderId } from "./types"
import { version } from "../../package.json"

const accounts: Account[] = [
  { id: "claude-personal", provider: "claude", label: "claude.one@example.com", createdAt: "2026-09-03T00:00:00Z" },
  { id: "claude-work", provider: "claude", label: "claude.two@example.com", createdAt: "2026-09-03T00:00:00Z" },
  { id: "codex", provider: "codex", label: "codex@example.com", createdAt: "2026-09-03T00:00:00Z" },
  { id: "cursor", provider: "cursor", label: "cursor@example.com", createdAt: "2026-09-03T00:00:00Z" },
]

const usage: Record<string, AccountUsage> = {
  "claude-personal": sample("claude-personal", "claude.one@example.com", "MAX", [["five-hour", "5시간", 100, "1시간 15분 후 초기화"], ["weekly", "주간", 52, "4일 후 초기화"]]),
  "claude-work": sample("claude-work", "claude.two@example.com", "MAX", [["five-hour", "5시간", 71, "3시간 35분 후 초기화"], ["weekly", "주간", 24, "5일 후 초기화"]]),
  codex: sample("codex", "codex@example.com", "PRO", [["weekly", "주간", 100, "4일 후 초기화"]]),
  cursor: sample("cursor", "cursor@example.com", "PRO", [["included", "포함 사용량", 64, "9월 18일 초기화"], ["on-demand", "추가 사용량", 8, "이번 결제 주기"]]),
}

function sample(accountId: string, email: string, plan: string, metrics: Array<[string, string, number, string]>): AccountUsage {
  return {
    accountId,
    email,
    plan,
    metrics: metrics.map(([id, label, usedPercent, resetText]) => ({ id, label, usedPercent, resetText, resetsAt: null, detail: null })),
    fetchedAt: "2026-09-03T10:00:00Z",
    checkedAt: "2026-09-03T10:00:00Z",
    nextRetryAt: null,
    stale: false,
    error: null,
    sourceUrl: "demo",
    warning: null,
  }
}

export function installDemoApi(): void {
  const api = {
    showMainWindow: async () => { window.location.search = "?demo" },
    hideWidget: async () => undefined,
    getAppVersion: async () => version,
    listAccounts: async () => accounts,
    getCachedUsage: async (accountId: string) => usage[accountId] ?? null,
    addAccount: async (provider: ProviderId, label: string) => ({ id: `demo-${Date.now()}`, provider, label, createdAt: new Date().toISOString() }),
    renameAccount: async (accountId: string, label: string) => {
      const account = accounts.find((item) => item.id === accountId)
      if (!account) throw new Error("계정을 찾을 수 없습니다.")
      account.label = label
      return account
    },
    removeAccount: async (_accountId: string) => undefined,
    authenticateAccount: async (_accountId: string) => undefined,
    cancelAuthentication: async (_accountId: string) => undefined,
    openProviderPortal: async (_accountId: string) => undefined,
    checkForUpdate: async () => ({ currentVersion: "0.1.6", latestVersion: "0.1.7", releaseUrl: "https://github.com/LCH-1/ai-usage-viewer/releases/latest", available: true }),
    openLatestRelease: async () => undefined,
    refreshAccount: async (accountId: string, _force = false) => usage[accountId] ?? sample(accountId, "demo@example.com", "PRO", []),
  }
  window.usageViewer = api
}
