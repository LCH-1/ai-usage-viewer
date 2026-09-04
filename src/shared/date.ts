export function parseTimestamp(value: unknown): Date | null {
  if (typeof value === "number" || (typeof value === "string" && /^\d+(?:\.\d+)?$/.test(value.trim()))) {
    const numeric = Number(value)
    if (!Number.isFinite(numeric)) return null
    const date = new Date(Math.abs(numeric) < 1_000_000_000_000 ? numeric * 1000 : numeric)
    return Number.isNaN(date.getTime()) ? null : date
  }

  if (typeof value !== "string" || !value.trim()) return null
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? null : date
}

export function formatResetDate(value: unknown): string | null {
  const date = parseTimestamp(value)
  return date ? `${date.toLocaleDateString("ko-KR")} 초기화` : null
}

export function formatResetDateTime(value: unknown): string | null {
  const date = parseTimestamp(value)
  return date ? `${date.toLocaleString("ko-KR")} 초기화` : null
}
