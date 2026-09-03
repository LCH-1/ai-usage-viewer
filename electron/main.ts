import { app, BrowserWindow, ipcMain, session, shell } from "electron"
import { join } from "node:path"

import { addAccount, listAccounts, removeAccount } from "./account-store"
import { PROVIDERS } from "../src/shared/providers"
import { parseUsagePage } from "../src/shared/usage-parser"
import type { Account, ProviderId } from "../src/shared/types"

const isDevelopment = Boolean(process.env.VITE_DEV_SERVER_URL)
const usageWindows = new Map<string, BrowserWindow>()

function partitionName(account: Account): string {
  return `persist:usage-viewer-${account.provider}-${account.id}`
}

async function findAccount(accountId: string): Promise<Account> {
  const account = (await listAccounts()).find((item) => item.id === accountId)
  if (!account) throw new Error("계정을 찾을 수 없습니다.")
  return account
}

function isProviderId(value: string): value is ProviderId {
  return value === "claude" || value === "codex" || value === "cursor"
}

function createUsageWindow(account: Account, show: boolean): BrowserWindow {
  const existing = usageWindows.get(account.id)
  if (existing && !existing.isDestroyed()) {
    if (show) existing.show()
    return existing
  }

  const definition = PROVIDERS[account.provider]
  const window = new BrowserWindow({
    width: 1100,
    height: 820,
    minWidth: 760,
    minHeight: 600,
    show,
    title: `${definition.name} · ${account.label}`,
    autoHideMenuBar: true,
    webPreferences: {
      partition: partitionName(account),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  })

  window.webContents.setWindowOpenHandler(({ url }) => {
    const host = new URL(url).hostname
    if (definition.loginHosts.some((allowed) => host === allowed || host.endsWith(`.${allowed}`))) {
      return {
        action: "allow",
        overrideBrowserWindowOptions: {
          autoHideMenuBar: true,
          webPreferences: {
            partition: partitionName(account),
            contextIsolation: true,
            nodeIntegration: false,
            sandbox: true,
          },
        },
      }
    }
    void shell.openExternal(url)
    return { action: "deny" }
  })
  window.on("closed", () => usageWindows.delete(account.id))
  usageWindows.set(account.id, window)
  return window
}

async function loadUsagePage(account: Account, show: boolean): Promise<BrowserWindow> {
  const window = createUsageWindow(account, show)
  const definition = PROVIDERS[account.provider]
  if (window.webContents.getURL() !== definition.usageUrl) {
    await window.loadURL(definition.usageUrl)
  }
  if (show) window.show()
  return window
}

async function refreshUsage(accountId: string) {
  const account = await findAccount(accountId)
  const window = await loadUsagePage(account, false)
  await new Promise((resolve) => setTimeout(resolve, 1800))
  const currentUrl = window.webContents.getURL()
  const pageText = await window.webContents.executeJavaScript("document.body?.innerText ?? ''", true) as string
  return parseUsagePage(account.provider, account.id, pageText, currentUrl)
}

function registerIpc(): void {
  ipcMain.handle("accounts:list", () => listAccounts())
  ipcMain.handle("accounts:add", async (_event, provider: string, label: string) => {
    if (!isProviderId(provider)) throw new Error("지원하지 않는 플랫폼입니다.")
    const account = await addAccount(provider, String(label ?? ""))
    await loadUsagePage(account, true)
    return account
  })
  ipcMain.handle("accounts:remove", async (_event, accountId: string) => {
    const account = await removeAccount(accountId)
    if (!account) return
    const window = usageWindows.get(account.id)
    if (window && !window.isDestroyed()) window.destroy()
    await session.fromPartition(partitionName(account)).clearStorageData()
  })
  ipcMain.handle("accounts:open", async (_event, accountId: string) => {
    const account = await findAccount(accountId)
    await loadUsagePage(account, true)
  })
  ipcMain.handle("usage:refresh", (_event, accountId: string) => refreshUsage(accountId))
}

async function createMainWindow(): Promise<void> {
  const window = new BrowserWindow({
    width: 520,
    height: 780,
    minWidth: 420,
    minHeight: 620,
    backgroundColor: "#070809",
    title: "Usage Viewer",
    autoHideMenuBar: true,
    webPreferences: {
      preload: join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  })

  window.webContents.setWindowOpenHandler(({ url }) => {
    void shell.openExternal(url)
    return { action: "deny" }
  })

  if (isDevelopment) {
    await window.loadURL(process.env.VITE_DEV_SERVER_URL!)
  } else {
    await window.loadFile(join(__dirname, "../../dist/index.html"))
  }
}

app.whenReady().then(async () => {
  registerIpc()
  await createMainWindow()
  app.on("activate", async () => {
    if (BrowserWindow.getAllWindows().length === 0) await createMainWindow()
  })
})

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit()
})
