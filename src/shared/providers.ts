import type { ProviderId } from "./types"

export interface ProviderDefinition {
  id: ProviderId
  name: string
  color: string
  usageUrl: string
}

export const PROVIDERS: Record<ProviderId, ProviderDefinition> = {
  claude: {
    id: "claude",
    name: "Claude",
    color: "#d97757",
    usageUrl: "https://claude.ai/settings/usage",
  },
  codex: {
    id: "codex",
    name: "Codex",
    color: "#6ee7b7",
    usageUrl: "https://chatgpt.com/codex/settings/usage",
  },
  cursor: {
    id: "cursor",
    name: "Cursor",
    color: "#a78bfa",
    usageUrl: "https://cursor.com/dashboard/spending",
  },
}

export const PROVIDER_ORDER: ProviderId[] = ["claude", "codex", "cursor"]
