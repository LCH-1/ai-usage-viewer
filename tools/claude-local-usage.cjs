const fs = require("node:fs")
const path = require("node:path")
const os = require("node:os")
const crypto = require("node:crypto")

const digest = (value) => crypto.createHash("sha256").update(value).digest("hex")
const readJson = (file) => {
  if (fs.statSync(file).size > 128 * 1024) throw new Error("oversized metadata")
  return JSON.parse(fs.readFileSync(file, "utf8"))
}
const writeJson = (file, data) => {
  fs.mkdirSync(path.dirname(file), { recursive: true })
  const temporary = `${file}.${process.pid}.tmp`
  try {
    fs.writeFileSync(temporary, JSON.stringify(data), { mode: 0o600 })
    fs.renameSync(temporary, file)
  } finally {
    if (fs.existsSync(temporary)) fs.unlinkSync(temporary)
  }
}

function identity(root) {
  const accountFile = fs.existsSync(path.join(root, ".claude.json")) ? path.join(root, ".claude.json") : path.join(root, "..", ".claude.json")
  const account = readJson(accountFile).oauthAccount
  const credential = readJson(path.join(root, ".credentials.json")).claudeAiOauth
  if (!account?.accountUuid || !account.emailAddress || !credential?.accessToken) throw new Error("missing identity")
  return {
    email: account.emailAddress.trim().toLowerCase(),
    accountUuid: account.accountUuid,
    organizationUuid: account.organizationUuid,
    credential: digest(credential.accessToken),
  }
}

function responseTime(file, after) {
  const fd = fs.openSync(file, "r")
  try {
    const size = fs.fstatSync(fd).size
    const offset = Math.max(0, size - 512 * 1024)
    const buffer = Buffer.alloc(size - offset)
    fs.readSync(fd, buffer, 0, buffer.length, offset)
    const lines = buffer.toString("utf8").split("\n")
    if (offset) lines.shift()
    for (const line of lines.reverse()) {
      let event
      try { event = JSON.parse(line) } catch { continue }
      if (event.type === "assistant" && Date.parse(event.timestamp) >= after) return event.timestamp
    }
    return null
  } finally {
    fs.closeSync(fd)
  }
}

function capture(mode, data, root, now = Date.now()) {
  if (!data.session_id || !["start", "usage"].includes(mode)) return null
  const current = identity(root)
  const directory = path.join(root, "usage-viewer")
  const bindingFile = path.join(directory, "sessions", `${digest(data.session_id)}.json`)
  if (mode === "start") {
    // A resumed session must produce a new response before its cached limits are used.
    writeJson(bindingFile, { ...current, startedAt: now })
    return null
  }
  const binding = readJson(bindingFile)
  if (JSON.stringify(current) !== JSON.stringify({ email: binding.email, accountUuid: binding.accountUuid, organizationUuid: binding.organizationUuid, credential: binding.credential })) return null
  const rateLimits = data.rate_limits
  if (!rateLimits || !data.transcript_path) return null
  const observedAt = responseTime(data.transcript_path, binding.startedAt)
  const age = now - Date.parse(observedAt)
  if (!observedAt || age < 0 || age >= 60_000) return null
  const windows = {}
  for (const key of ["five_hour", "seven_day"]) {
    const window = rateLimits[key]
    if (typeof window?.used_percentage !== "number" || !Number.isFinite(window.used_percentage)) continue
    if (typeof window.resets_at !== "number" || window.resets_at * 1000 <= now) continue
    windows[key] = { utilization: window.used_percentage, resets_at: new Date(window.resets_at * 1000).toISOString() }
  }
  if (!Object.keys(windows).length) return null
  const latestFile = path.join(directory, `${digest(current.email)}.json`)
  if (fs.existsSync(latestFile) && Date.parse(readJson(latestFile).observedAt) > Date.parse(observedAt)) return null
  const result = { version: 1, email: current.email, accountUuid: current.accountUuid, organizationUuid: current.organizationUuid, observedAt, windows }
  if (JSON.stringify(identity(root)) !== JSON.stringify(current)) return null
  writeJson(latestFile, result)
  return result
}

if (require.main === module) {
  try {
    const input = fs.readFileSync(0, "utf8")
    if (input.length > 1024 * 1024) process.exit(0)
    const root = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), ".claude")
    const result = capture(process.argv[2], JSON.parse(input), root)
    if (process.argv[2] === "usage") {
      const windows = result?.windows
      process.stdout.write(windows ? Object.entries(windows).map(([key, value]) => `${key === "five_hour" ? "5h" : "7d"} ${value.utilization}%`).join(" · ") : "Claude Code")
    }
  } catch {
    // Local collection is optional and must never interrupt Claude Code.
  }
}

module.exports = { capture, digest }
