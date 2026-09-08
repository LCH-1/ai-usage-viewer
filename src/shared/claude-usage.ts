import { formatResetDateTime } from "./date"
import type { UsageMetric } from "./types"

export interface ClaudeWindow {
  utilization?: number
  resets_at?: string
}

interface ClaudeScopedLimit {
  kind?: string
  group?: string
  percent?: number
  utilization?: number
  resets_at?: string
  scope?: {
    model?: {
      display_name?: string
      model_id?: string | null
    }
  }
}

export interface ClaudeUsagePayload {
  five_hour?: ClaudeWindow
  seven_day?: ClaudeWindow
  seven_day_sonnet?: ClaudeWindow
  seven_day_opus?: ClaudeWindow
  limits?: ClaudeScopedLimit[]
}

function metric(id: string, label: string, usedPercent: number | undefined, resetsAt: string | undefined): UsageMetric | null {
  if (typeof usedPercent !== "number") return null
  return {
    id,
    label,
    usedPercent: Math.max(0, Math.min(100, Math.round(usedPercent * 10) / 10)),
    resetText: formatResetDateTime(resetsAt),
    resetsAt: resetsAt ?? null,
    detail: null,
  }
}

function modelMetricId(displayName: string): string {
  const normalized = displayName.trim().toLowerCase()
  if (normalized.includes("sonnet")) return "sonnet"
  if (normalized.includes("opus")) return "opus"
  if (normalized.includes("fable")) return "fable"
  return `model-${normalized.replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "")}`
}

export function parseClaudeUsageMetrics(payload: ClaudeUsagePayload): UsageMetric[] {
  const metrics = new Map<string, UsageMetric>()
  const legacy = [
    metric("five-hour", "5시간", payload.five_hour?.utilization, payload.five_hour?.resets_at),
    metric("weekly", "주간", payload.seven_day?.utilization, payload.seven_day?.resets_at),
    metric("sonnet", "Sonnet 주간", payload.seven_day_sonnet?.utilization, payload.seven_day_sonnet?.resets_at),
    metric("opus", "Opus 주간", payload.seven_day_opus?.utilization, payload.seven_day_opus?.resets_at),
  ]
  for (const item of legacy) if (item) metrics.set(item.id, item)

  for (const limit of payload.limits ?? []) {
    if (limit.kind !== "weekly_scoped" && limit.group !== "weekly") continue
    const displayName = limit.scope?.model?.display_name?.trim()
    if (!displayName) continue
    const item = metric(modelMetricId(displayName), `${displayName} 주간`, limit.percent ?? limit.utilization, limit.resets_at)
    if (item) metrics.set(item.id, item)
  }

  return [...metrics.values()]
}
