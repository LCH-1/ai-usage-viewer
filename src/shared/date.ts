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
  return date ? `${formatDateTime(date)} 초기화` : null
}

export function formatDateTime(value: unknown): string | null {
  const date = value instanceof Date ? value : parseTimestamp(value)
  if (!date) return null
  return new Intl.DateTimeFormat("ko-KR", {
    year: "numeric",
    month: "numeric",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(date)
}
