const { test } = require("node:test")
const assert = require("node:assert/strict")
const fs = require("node:fs")
const os = require("node:os")
const path = require("node:path")
const { spawnSync } = require("node:child_process")
const { capture, digest } = require("./claude-local-usage.cjs")

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "usage-viewer-collector-"))
  t.after(() => fs.rmSync(root, { recursive: true, force: true }))
  const write = (file, data) => fs.writeFileSync(path.join(root, file), JSON.stringify(data))
  write(".claude.json", { oauthAccount: { emailAddress: "one@example.invalid", accountUuid: "account-one", organizationUuid: "org-one" } })
  write(".credentials.json", { claudeAiOauth: { accessToken: "fixture-token" } })
  const now = Date.parse("2026-09-09T01:00:00Z")
  const input = { session_id: "session-one", transcript_path: path.join(root, "transcript.jsonl"), rate_limits: { five_hour: { used_percentage: 27, resets_at: now / 1000 + 3600 }, seven_day: { used_percentage: 43, resets_at: now / 1000 + 86400 } } }
  write("transcript.jsonl", { type: "assistant", timestamp: new Date(now + 1000).toISOString(), message: { content: "private fixture text" } })
  return { root, write, now, input }
}

test("captures only quota and identity from a newly bound session", (t) => {
  const { root, now, input } = fixture(t)
  capture("start", input, root, now)
  const result = capture("usage", input, root, now + 2000)
  assert.equal(result.windows.five_hour.utilization, 27)
  assert.equal(result.observedAt, new Date(now + 1000).toISOString())
  const text = fs.readFileSync(path.join(root, "usage-viewer", `${digest(result.email)}.json`), "utf8")
  assert.ok(!text.includes("fixture-token"))
  assert.ok(!text.includes("private fixture text"))
  assert.ok(!text.includes("transcript"))
})

test("idle callbacks do not renew freshness and resume cannot reuse old limits", (t) => {
  const { root, now, input } = fixture(t)
  capture("start", input, root, now)
  assert.ok(capture("usage", input, root, now + 2000))
  assert.equal(capture("usage", input, root, now + 61_000), null)
  capture("start", input, root, now + 70_000)
  assert.equal(capture("usage", input, root, now + 71_000), null)
})

test("account switch and token replacement invalidate the session binding", (t) => {
  const { root, write, now, input } = fixture(t)
  capture("start", input, root, now)
  write(".credentials.json", { claudeAiOauth: { accessToken: "other-token" } })
  assert.equal(capture("usage", input, root, now + 2000), null)
  write(".credentials.json", { claudeAiOauth: { accessToken: "fixture-token" } })
  write(".claude.json", { oauthAccount: { emailAddress: "two@example.invalid", accountUuid: "account-two" } })
  assert.equal(capture("usage", input, root, now + 2000), null)
})

test("missing quota or missing binding falls back without producing a snapshot", (t) => {
  const { root, now, input } = fixture(t)
  assert.throws(() => capture("usage", input, root, now + 2000))
  capture("start", input, root, now)
  assert.equal(capture("usage", { ...input, rate_limits: undefined }, root, now + 2000), null)
  assert.equal(capture("usage", { ...input, rate_limits: { five_hour: { used_percentage: 0, resets_at: now / 1000 } } }, root, now + 2000), null)
})

test("installer preserves hooks, backs up settings and is idempotent", (t) => {
  const { root, write } = fixture(t)
  const initial = { permissions: { allow: ["Read"] }, hooks: { SessionStart: [{ hooks: [{ type: "command", command: "existing-hook" }] }] } }
  write("settings.json", initial)
  const run = () => spawnSync(process.execPath, [path.join(__dirname, "install-claude-local-usage.cjs")], { env: { ...process.env, CLAUDE_CONFIG_DIR: root }, encoding: "utf8" })
  assert.equal(run().status, 0)
  assert.equal(run().status, 0)
  const settings = JSON.parse(fs.readFileSync(path.join(root, "settings.json"), "utf8"))
  assert.deepEqual(settings.permissions, initial.permissions)
  assert.equal(settings.hooks.SessionStart.length, 2)
  assert.equal(settings.hooks.SessionStart[0].hooks[0].command, "existing-hook")
  assert.equal(settings.statusLine.refreshInterval, 5)
  assert.ok(fs.readdirSync(root).some((name) => name.endsWith(".bak")))
  settings.statusLine.command = "custom-hud"
  write("settings.json", settings)
  assert.notEqual(run().status, 0)
  assert.equal(JSON.parse(fs.readFileSync(path.join(root, "settings.json"), "utf8")).statusLine.command, "custom-hud")
})
