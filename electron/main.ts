import { app, BrowserWindow, ipcMain, Menu, shell, Tray } from "electron"
import { join } from "node:path"

import { addAccount, listAccounts, removeAccount, renameAccount } from "./account-store"
import { deleteCredential } from "./credential-store"
import { authenticateClaude, getClaudeUsage } from "./providers/claude"
import { authenticateCodex, getCodexUsage } from "./providers/codex"
import { authenticateCursor, getCursorUsage } from "./providers/cursor"
import { PROVIDERS } from "../src/shared/providers"
import type { Account, AccountUsage, ProviderId } from "../src/shared/types"

const isDevelopment = Boolean(process.env.VITE_DEV_SERVER_URL)
const activeAuthentications = new Map<string, Promise<void>>()
let mainWindow: BrowserWindow | null = null
let tray: Tray | null = null
let isQuitting = false

function appIconPath(): string {
  return isDevelopment ? join(app.getAppPath(), "public", "icon.png") : join(app.getAppPath(), "dist", "icon.png")
}

function showMainWindow(): void {
  if (!mainWindow) return
  mainWindow.show()
  mainWindow.focus()
}

async function findAccount(accountId: string): Promise<Account> {
  const account = (await listAccounts()).find((item) => item.id === accountId)
  if (!account) throw new Error("계정을 찾을 수 없습니다.")
  return account
}

function isProviderId(value: string): value is ProviderId {
  return value === "claude" || value === "codex" || value === "cursor"
}

async function authenticate(account: Account): Promise<void> {
  const running = activeAuthentications.get(account.id)
  if (running) return running
  const operation = (async () => {
    if (account.provider === "claude") await authenticateClaude(account.id)
    if (account.provider === "codex") await authenticateCodex(account.id)
    if (account.provider === "cursor") await authenticateCursor(account.id)
  })().finally(() => activeAuthentications.delete(account.id))
  activeAuthentications.set(account.id, operation)
  return operation
}

async function refreshUsage(account: Account): Promise<AccountUsage> {
  if (account.provider === "claude") return getClaudeUsage(account.id)
  if (account.provider === "codex") return getCodexUsage(account.id)
  return getCursorUsage(account.id)
}

function registerIpc(): void {
  ipcMain.handle("accounts:list", () => listAccounts())
  ipcMain.handle("accounts:add", (_event, provider: string, label: string) => {
    if (!isProviderId(provider)) throw new Error("지원하지 않는 플랫폼입니다.")
    return addAccount(provider, String(label ?? ""))
  })
  ipcMain.handle("accounts:rename", (_event, accountId: string, label: string) => renameAccount(accountId, String(label ?? "")))
  ipcMain.handle("accounts:remove", async (_event, accountId: string) => {
    const account = await removeAccount(accountId)
    if (account) await deleteCredential(account.id)
  })
  ipcMain.handle("accounts:authenticate", async (_event, accountId: string) => {
    await authenticate(await findAccount(accountId))
  })
  ipcMain.handle("accounts:portal", async (_event, accountId: string) => {
    const account = await findAccount(accountId)
    await shell.openExternal(PROVIDERS[account.provider].usageUrl)
  })
  ipcMain.handle("usage:refresh", async (_event, accountId: string) => refreshUsage(await findAccount(accountId)))
}

async function createMainWindow(): Promise<void> {
  const window = new BrowserWindow({
    width: 520,
    height: 780,
    minWidth: 420,
    minHeight: 620,
    backgroundColor: "#070809",
    title: "Usage Viewer",
    icon: appIconPath(),
    autoHideMenuBar: true,
    webPreferences: {
      preload: join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  })
  mainWindow = window

  window.on("close", (event) => {
    if (isQuitting) return
    event.preventDefault()
    window.hide()
  })
  window.on("closed", () => {
    if (mainWindow === window) mainWindow = null
  })

  window.webContents.setWindowOpenHandler(({ url }) => {
    void shell.openExternal(url)
    return { action: "deny" }
  })

  if (isDevelopment) await window.loadURL(process.env.VITE_DEV_SERVER_URL!)
  else await window.loadFile(join(__dirname, "../../dist/index.html"))
}

app.whenReady().then(async () => {
  registerIpc()
  await createMainWindow()
  tray = new Tray(appIconPath())
  tray.setToolTip("Usage Viewer")
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: "Usage Viewer 열기", click: showMainWindow },
    { type: "separator" },
    { label: "종료", click: () => app.quit() },
  ]))
  tray.on("click", showMainWindow)
  app.on("activate", async () => {
    if (BrowserWindow.getAllWindows().length === 0) await createMainWindow()
    else showMainWindow()
  })
})

app.on("before-quit", () => {
  isQuitting = true
})
