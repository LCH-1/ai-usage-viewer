import { contextBridge, ipcRenderer } from "electron"

import type { Account, AccountUsage, ProviderId } from "../src/shared/types"

const api = {
  listAccounts: (): Promise<Account[]> => ipcRenderer.invoke("accounts:list"),
  addAccount: (provider: ProviderId, label: string): Promise<Account> => ipcRenderer.invoke("accounts:add", provider, label),
  renameAccount: (accountId: string, label: string): Promise<Account> => ipcRenderer.invoke("accounts:rename", accountId, label),
  removeAccount: (accountId: string): Promise<void> => ipcRenderer.invoke("accounts:remove", accountId),
  authenticateAccount: (accountId: string): Promise<void> => ipcRenderer.invoke("accounts:authenticate", accountId),
  openProviderPortal: (accountId: string): Promise<void> => ipcRenderer.invoke("accounts:portal", accountId),
  refreshAccount: (accountId: string): Promise<AccountUsage> => ipcRenderer.invoke("usage:refresh", accountId),
}

contextBridge.exposeInMainWorld("usageViewer", api)

export type UsageViewerApi = typeof api
