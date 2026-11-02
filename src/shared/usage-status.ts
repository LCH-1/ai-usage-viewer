import { formatDateTime } from "./date"
import type { AccountUsage, ProviderError } from "./types"

interface UsageState {
  usage: AccountUsage | null
  error: ProviderError | null
}

export function usageStatus({ usage, error: requestError }: UsageState) {
  const error = requestError ?? usage?.error
  const subscriptionRequired = error?.code === "subscriptionRequired"
  const accessDenied = error?.code === "accessDenied" || (error?.code === "authRequired" && error.status === 403 && error.endpoint === "claude.usage")
  const hideUsage = subscriptionRequired || accessDenied
  const plan = hideUsage ? subscriptionRequired ? "FREE" : null : usage?.plan
  const previousValues = Boolean(usage?.stale || (error && !["cancelled", "authenticating"].includes(error.code)))
  const retry = formatDateTime(error?.retryAt ?? usage?.nextRetryAt)
  const retryText = retry ? `${retry} 자동 재시도` : "자동으로 다시 확인합니다."
  const issue = subscriptionRequired ? { label: "구독 비활성", detail: error!.message, tone: "warning" }
    : accessDenied ? { label: "구독·권한 확인", detail: "구독 상태 또는 계정 권한으로 사용량을 조회할 수 없습니다. 계정의 구독 상태를 확인해 주세요.", tone: "warning" }
    : error?.code === "authRequired" ? { label: "재로그인 필요", detail: error.message, tone: "error" }
    : error?.code === "rateLimited" ? { label: "조회 제한", detail: `사용량 조회 요청이 일시적으로 제한됐습니다. ${retryText}`, tone: "warning" }
    : error?.code === "temporary" ? { label: "조회 대기", detail: retryText, tone: "warning" }
    : error && !["cancelled", "authenticating"].includes(error.code) ? { label: "조회 실패", detail: error.message, tone: "error" }
    : usage?.stale ? { label: "갱신 대기", detail: retryText, tone: "warning" }
    : usage?.warning ? { label: "확인 필요", detail: usage.warning, tone: "warning" } : null
  const fetched = formatDateTime(usage?.fetchedAt)
  return { issue, plan, hideUsage, previousValues: previousValues && !hideUsage, previousLabel: fetched ? `이전 조회값 · ${fetched}` : "이전 조회값" }
}
