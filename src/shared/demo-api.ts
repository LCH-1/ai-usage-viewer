import type { Account, AccountUsage, ProviderId } from "./types"

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
    metrics: metrics.map(([id, label, usedPercent, resetText]) => ({ id, label, usedPercent, resetText, detail: null })),
    fetchedAt: "2026-09-03T10:00:00Z",
    sourceUrl: "demo",
    warning: null,
  }
}

export function installDemoApi(): void {
  const api = {
    listAccounts: async () => accounts,
    addAccount: async (provider: ProviderId, label: string) => ({ id: `demo-${Date.now()}`, provider, label, createdAt: new Date().toISOString() }),
    removeAccount: async (_accountId: string) => undefined,
    openAccount: async (_accountId: string) => undefined,
    refreshAccount: async (accountId: string) => usage[accountId] ?? sample(accountId, "demo@example.com", "PRO", []),
  }
  window.usageViewer = api
}
