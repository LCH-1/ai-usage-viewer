import { parseTimestamp } from "./date"
import type { AccountView, ProviderError, UsageMetric } from "./types"

export function providerError(error: unknown): ProviderError {
  if (typeof error === "object" && error !== null && "code" in error && "message" in error) {
    const value = error as Partial<ProviderError>
    if (typeof value.code === "string" && typeof value.message === "string") {
      return {
        code: value.code,
        message: value.message,
        retryAt: value.retryAt ?? null,
        endpoint: value.endpoint ?? null,
        status: value.status ?? null,
      }
    }
  }
  const message = error instanceof Error ? error.message : String(error)
  return {
    code: "internal",
    message: message.replace(/^Error invoking remote method '[^']+': Error: /, ""),
    retryAt: null,
    endpoint: null,
    status: null,
  }
}

export function nextRetryAt(account: AccountView): string | null {
  return account.error?.retryAt ?? account.usage?.nextRetryAt ?? null
}

export function canRefreshAccount(account: AccountView, now: number): boolean {
  if (account.authenticating || account.cancelling || account.removing || account.error?.code === "authRequired") return false
  const retry = parseTimestamp(nextRetryAt(account))
  return retry === null || retry.getTime() <= now
}

export function resetNeedsVerification(metric: UsageMetric, now: number): boolean {
  const reset = parseTimestamp(metric.resetsAt)
  return reset !== null && reset.getTime() <= now
}
