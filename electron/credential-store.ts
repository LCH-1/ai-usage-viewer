import { app, safeStorage } from "electron"
import { mkdir, readFile, rm, writeFile } from "node:fs/promises"
import { join } from "node:path"

export interface ClaudeCredential {
  provider: "claude"
  oauth: {
    accessToken: string
    refreshToken: string
    expiresAt: number
    scopes?: string[]
    subscriptionType?: string
    rateLimitTier?: string
  }
}

export interface CodexCredential {
  provider: "codex"
  authFile: string
}

export interface CursorCredential {
  provider: "cursor"
  accessToken: string
  refreshToken: string
}

export type ProviderCredential = ClaudeCredential | CodexCredential | CursorCredential

function credentialDirectory(): string {
  return join(app.getPath("userData"), "credentials")
}

function credentialPath(accountId: string): string {
  return join(credentialDirectory(), `${accountId}.bin`)
}

function requireEncryption(): void {
  if (!safeStorage.isEncryptionAvailable()) {
    throw new Error("운영체제 암호화 저장소를 사용할 수 없어 계정 정보를 저장하지 않았습니다.")
  }
}

export async function saveCredential(accountId: string, credential: ProviderCredential): Promise<void> {
  requireEncryption()
  await mkdir(credentialDirectory(), { recursive: true })
  const encrypted = safeStorage.encryptString(JSON.stringify(credential))
  await writeFile(credentialPath(accountId), encrypted)
}

export async function loadCredential(accountId: string): Promise<ProviderCredential> {
  requireEncryption()
  try {
    const encrypted = await readFile(credentialPath(accountId))
    return JSON.parse(safeStorage.decryptString(encrypted)) as ProviderCredential
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      throw new Error("로그인이 필요합니다. 열쇠 버튼을 눌러 기본 브라우저에서 로그인하세요.")
    }
    throw error
  }
}

export async function deleteCredential(accountId: string): Promise<void> {
  await rm(credentialPath(accountId), { force: true })
}
