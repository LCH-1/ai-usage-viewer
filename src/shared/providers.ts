import type { ProviderId } from "./types"

export interface ProviderDefinition {
  id: ProviderId
  name: string
  color: string
  usageUrl: string
  loginHosts: string[]
}

export const PROVIDERS: Record<ProviderId, ProviderDefinition> = {
  claude: {
    id: "claude",
    name: "Claude",
    color: "#d97757",
    usageUrl: "https://claude.ai/settings/usage",
    loginHosts: ["claude.ai", "accounts.google.com"],
  },
  codex: {
    id: "codex",
    name: "Codex",
    color: "#6ee7b7",
    usageUrl: "https://chatgpt.com/codex/settings/usage",
    loginHosts: ["chatgpt.com", "auth.openai.com", "accounts.google.com", "login.microsoftonline.com"],
  },
  cursor: {
    id: "cursor",
    name: "Cursor",
    color: "#a78bfa",
    usageUrl: "https://cursor.com/dashboard/spending",
    loginHosts: ["cursor.com", "authenticator.cursor.sh", "accounts.google.com", "github.com"],
  },
}

export const PROVIDER_ORDER: ProviderId[] = ["claude", "codex", "cursor"]
