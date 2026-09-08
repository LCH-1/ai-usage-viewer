import { invoke } from "@tauri-apps/api/core"
import { getVersion } from "@tauri-apps/api/app"

import type { Account, AccountUsage, ProviderId, UpdateInfo } from "./shared/types"

export const usageViewerApi = {
  showMainWindow: (): Promise<void> => invoke("show_main_window"),
  hideWidget: (): Promise<void> => invoke("hide_widget"),
  getAppVersion: (): Promise<string> => getVersion(),
  listAccounts: (): Promise<Account[]> => invoke("list_accounts"),
  getCachedUsage: (accountId: string): Promise<AccountUsage | null> => invoke("get_cached_usage", { accountId }),
  addAccount: (provider: ProviderId, label: string): Promise<Account> => invoke("add_account", { provider, label }),
  renameAccount: (accountId: string, label: string): Promise<Account> => invoke("rename_account", { accountId, label }),
  removeAccount: (accountId: string): Promise<void> => invoke("remove_account", { accountId }),
  authenticateAccount: (accountId: string): Promise<void> => invoke("authenticate_account", { accountId }),
  cancelAuthentication: (accountId: string): Promise<void> => invoke("cancel_authentication", { accountId }),
  openProviderPortal: (accountId: string): Promise<void> => invoke("open_provider_portal", { accountId }),
  checkForUpdate: (): Promise<UpdateInfo> => invoke("check_for_update"),
  openLatestRelease: (): Promise<void> => invoke("open_latest_release"),
  refreshAccount: (accountId: string, force = false): Promise<AccountUsage> => invoke("refresh_account", { accountId, force }),
}

export type UsageViewerApi = typeof usageViewerApi
