export type ProviderId = "claude" | "codex" | "cursor"

export interface Account {
  id: string
  provider: ProviderId
  label: string
  createdAt: string
}

export interface UsageMetric {
  id: string
  label: string
  usedPercent: number
  resetText: string | null
  resetsAt: string | null
  detail: string | null
}

export interface ProviderError {
  code: string
  message: string
  retryAt: string | null
  endpoint: string | null
  status: number | null
}

export interface AccountUsage {
  accountId: string
  email: string | null
  plan: string | null
  metrics: UsageMetric[]
  fetchedAt: string
  checkedAt: string | null
  nextRetryAt: string | null
  stale: boolean
  error: ProviderError | null
  sourceUrl: string
  warning: string | null
}

export interface UpdateInfo {
  currentVersion: string
  latestVersion: string
  releaseUrl: string
  available: boolean
}

export interface AccountView extends Account {
  usage: AccountUsage | null
  error: ProviderError | null
  refreshing: boolean
  authenticating: boolean
  cancelling: boolean
  removing: boolean
}
