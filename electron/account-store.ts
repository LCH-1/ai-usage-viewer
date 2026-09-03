import { app } from "electron"
import { randomUUID } from "node:crypto"
import { readFile, rename, writeFile } from "node:fs/promises"
import { join } from "node:path"

import type { Account, ProviderId } from "../src/shared/types"

function storePath(): string {
  return join(app.getPath("userData"), "accounts.json")
}

export async function listAccounts(): Promise<Account[]> {
  try {
    return JSON.parse(await readFile(storePath(), "utf8")) as Account[]
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code
    if (code === "ENOENT") return []
    throw error
  }
}

async function saveAccounts(accounts: Account[]): Promise<void> {
  const path = storePath()
  const temporary = `${path}.tmp`
  await writeFile(temporary, JSON.stringify(accounts, null, 2), "utf8")
  await rename(temporary, path)
}

export async function addAccount(provider: ProviderId, label: string): Promise<Account> {
  const accounts = await listAccounts()
  const account: Account = {
    id: randomUUID(),
    provider,
    label: label.trim() || `${provider} 계정 ${accounts.filter((item) => item.provider === provider).length + 1}`,
    createdAt: new Date().toISOString(),
  }
  await saveAccounts([...accounts, account])
  return account
}

export async function removeAccount(accountId: string): Promise<Account | null> {
  const accounts = await listAccounts()
  const account = accounts.find((item) => item.id === accountId) ?? null
  if (!account) return null
  await saveAccounts(accounts.filter((item) => item.id !== accountId))
  return account
}
