import type { AccountUsage, ProviderId, UsageMetric } from "./types"

const EMAIL_PATTERN = /[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/i
const PERCENT_PATTERN = /(\d{1,3}(?:\.\d+)?)\s*%/
const MONEY_PATTERN = /\$\s*(\d+(?:\.\d+)?)\s*(?:\/|of)\s*\$\s*(\d+(?:\.\d+)?)/i

interface MetricRule {
  id: string
  label: string
  aliases: string[]
}

const RULES: Record<ProviderId, MetricRule[]> = {
  claude: [
    { id: "five-hour", label: "5시간", aliases: ["five hour", "5 hour", "5-hour", "current session", "현재 세션"] },
    { id: "weekly", label: "주간", aliases: ["weekly", "week", "7 day", "7-day", "주간"] },
    { id: "opus", label: "Opus 주간", aliases: ["opus", "오퍼스"] },
  ],
  codex: [
    { id: "five-hour", label: "5시간", aliases: ["five hour", "5 hour", "5-hour", "session", "세션"] },
    { id: "weekly", label: "주간", aliases: ["weekly", "week", "7 day", "7-day", "주간"] },
    { id: "credits", label: "크레딧", aliases: ["credits", "credit", "크레딧"] },
  ],
  cursor: [
    { id: "included", label: "포함 사용량", aliases: ["included usage", "included", "포함 사용량"] },
    { id: "cursor-models", label: "Cursor 모델", aliases: ["cursor models", "cursor model"] },
    { id: "other-models", label: "기타 모델", aliases: ["other models", "third-party", "other model"] },
    { id: "on-demand", label: "추가 사용량", aliases: ["on-demand", "usage-based", "additional usage"] },
  ],
}

function normalizeLines(text: string): string[] {
  return text
    .replace(/\u00a0/g, " ")
    .split(/\r?\n/)
    .map((line) => line.replace(/\s+/g, " ").trim())
    .filter(Boolean)
}

function clampPercent(value: number): number {
  return Math.max(0, Math.min(100, Math.round(value * 10) / 10))
}

function extractReset(lines: string[], start: number): string | null {
  const candidates = lines.slice(start, start + 10)
  return candidates.find((line) => /reset|resets|remaining|left|초기화|남음/i.test(line)) ?? null
}

function extractMetric(lines: string[], rule: MetricRule): UsageMetric | null {
  const index = lines.findIndex((line) => rule.aliases.some((alias) => line.toLowerCase().includes(alias)))
  if (index < 0) return null

  const candidates = lines.slice(index, index + 10)
  for (const line of candidates) {
    const percent = line.match(PERCENT_PATTERN)
    if (percent) {
      return {
        id: rule.id,
        label: rule.label,
        usedPercent: clampPercent(Number(percent[1])),
        resetText: extractReset(lines, index),
        detail: line,
      }
    }

    const money = line.match(MONEY_PATTERN)
    if (money) {
      const used = Number(money[1])
      const limit = Number(money[2])
      return {
        id: rule.id,
        label: rule.label,
        usedPercent: limit > 0 ? clampPercent((used / limit) * 100) : 0,
        resetText: extractReset(lines, index),
        detail: `$${used} / $${limit}`,
      }
    }
  }

  return null
}

function extractPlan(text: string): string | null {
  const match = text.match(/\b(Max|Pro Plus|Pro\+|Pro|Plus|Team|Business|Enterprise|Free|Ultra)\b/i)
  return match ? match[1].toUpperCase() : null
}

export function parseUsagePage(provider: ProviderId, accountId: string, text: string, sourceUrl: string): AccountUsage {
  const lines = normalizeLines(text)
  const metrics = RULES[provider]
    .map((rule) => extractMetric(lines, rule))
    .filter((metric): metric is UsageMetric => metric !== null)
    .filter((metric, index, all) => all.findIndex((candidate) => candidate.id === metric.id) === index)

  return {
    accountId,
    email: text.match(EMAIL_PATTERN)?.[0] ?? null,
    plan: extractPlan(text),
    metrics,
    fetchedAt: new Date().toISOString(),
    sourceUrl,
    warning: metrics.length === 0 ? "사용량 항목을 찾지 못했습니다. 계정 페이지를 열어 로그인 상태와 화면을 확인해 주세요." : null,
  }
}
