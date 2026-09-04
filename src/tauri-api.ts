import { invoke } from "@tauri-apps/api/core"

import type { Account, AccountUsage, ProviderId } from "./shared/types"

export const usageViewerApi = {
  listAccounts: (): Promise<Account[]> => invoke("list_accounts"),
  addAccount: (provider: ProviderId, label: string): Promise<Account> => invoke("add_account", { provider, label }),
  renameAccount: (accountId: string, label: string): Promise<Account> => invoke("rename_account", { accountId, label }),
  removeAccount: (accountId: string): Promise<void> => invoke("remove_account", { accountId }),
  authenticateAccount: (accountId: string): Promise<void> => invoke("authenticate_account", { accountId }),
  openProviderPortal: (accountId: string): Promise<void> => invoke("open_provider_portal", { accountId }),
  refreshAccount: (accountId: string, force = false): Promise<AccountUsage> => invoke("refresh_account", { accountId, force }),
}

export type UsageViewerApi = typeof usageViewerApi
