import { describe, expect, it } from "vitest"

import { parseUsagePage } from "./usage-parser"

describe("parseUsagePage", () => {
  it("parses Claude session and weekly quotas", () => {
    const usage = parseUsagePage("claude", "a1", `
      claude.code@example.com
      Max plan
      Current session
      100% used
      Resets in 1 hour 15 minutes
      Weekly limits
      42% used
      Resets Sep 7
    `, "https://claude.ai/settings/usage")

    expect(usage.email).toBe("claude.code@example.com")
    expect(usage.plan).toBe("MAX")
    expect(usage.metrics).toEqual(expect.arrayContaining([
      expect.objectContaining({ id: "five-hour", usedPercent: 100 }),
      expect.objectContaining({ id: "weekly", usedPercent: 42 }),
    ]))
  })

  it("parses Codex limits independently for another account", () => {
    const usage = parseUsagePage("codex", "b2", `
      codex@example.com
      Pro
      5 hour limit 71%
      Resets in 3 hours
      Weekly limit 20%
      Resets in 4 days
    `, "https://chatgpt.com/codex/settings/usage")

    expect(usage.accountId).toBe("b2")
    expect(usage.plan).toBe("PRO")
    expect(usage.metrics.map((metric) => metric.usedPercent)).toEqual([71, 20])
  })

  it("converts Cursor spending fractions to percentages", () => {
    const usage = parseUsagePage("cursor", "c3", `
      user@example.com
      Pro Plus
      Included Usage
      $14 / $20
      Resets October 1
      On-Demand Usage
      $2.50 / $10
    `, "https://cursor.com/dashboard/spending")

    expect(usage.plan).toBe("PRO PLUS")
    expect(usage.metrics).toEqual(expect.arrayContaining([
      expect.objectContaining({ id: "included", usedPercent: 70 }),
      expect.objectContaining({ id: "on-demand", usedPercent: 25 }),
    ]))
  })

  it("returns a warning instead of inventing data", () => {
    const usage = parseUsagePage("claude", "a1", "Please sign in", "https://claude.ai/login")
    expect(usage.metrics).toEqual([])
    expect(usage.warning).toContain("찾지 못했습니다")
  })
})
