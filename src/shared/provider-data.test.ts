import { describe, expect, it } from "vitest"

import { parseClaudeUsageMetrics } from "./claude-usage"
import { formatDateTime, formatResetDate, parseTimestamp } from "./date"

describe("parseClaudeUsageMetrics", () => {
  it("keeps legacy limits and adds scoped Fable usage", () => {
    const metrics = parseClaudeUsageMetrics({
      five_hour: { utilization: 23, resets_at: "2026-09-04T12:00:00Z" },
      limits: [{
        kind: "weekly_scoped",
        group: "weekly",
        percent: 47.5,
        resets_at: "2026-09-08T00:00:00Z",
        scope: { model: { display_name: "Fable", model_id: null } },
      }],
    })

    expect(metrics).toEqual(expect.arrayContaining([
      expect.objectContaining({ id: "five-hour", usedPercent: 23 }),
      expect.objectContaining({ id: "fable", label: "Fable 주간", usedPercent: 47.5 }),
    ]))
  })

  it("replaces a legacy model metric with its scoped value", () => {
    const metrics = parseClaudeUsageMetrics({
      seven_day_opus: { utilization: 10 },
      limits: [{ group: "weekly", percent: 35, scope: { model: { display_name: "Opus" } } }],
    })

    expect(metrics.filter((metric) => metric.id === "opus")).toEqual([
      expect.objectContaining({ usedPercent: 35 }),
    ])
  })
})

describe("Cursor reset timestamp", () => {
  it("accepts protobuf seconds and millisecond strings", () => {
    expect(parseTimestamp("1767225600")?.getTime()).toBe(1_767_225_600_000)
    expect(parseTimestamp("1767225600000")?.getTime()).toBe(1_767_225_600_000)
  })

  it("accepts ISO dates and rejects malformed values", () => {
    expect(formatResetDate("2026-10-01T00:00:00Z")).toContain("초기화")
    expect(formatResetDate("not-a-date")).toBeNull()
  })
})

describe("display timestamp", () => {
  it("omits seconds", () => {
    expect(formatDateTime("2026-09-04T09:15:10Z")?.match(/:/g)).toHaveLength(1)
  })
})
