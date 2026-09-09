const fs = require("node:fs")
const path = require("node:path")
const os = require("node:os")

const root = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), ".claude")
const settingsFile = path.join(root, "settings.json")
const original = fs.readFileSync(settingsFile, "utf8")
const settings = JSON.parse(original)
const collector = path.join(root, "usage-viewer", "collector.cjs")
const quote = (value) => `'${value.replaceAll("\\", "/").replaceAll("'", "'\\''")}'`
const command = `${quote(process.execPath)} ${quote(collector)}`
if (settings.statusLine && settings.statusLine.command !== `${command} usage`) {
  throw new Error("기존 statusLine 설정이 있어 자동 설치하지 않았습니다. 기존 명령과 수집 명령을 연결해야 합니다.")
}
settings.hooks ??= {}
settings.hooks.SessionStart ??= []
if (!settings.hooks.SessionStart.some((entry) => entry.hooks?.some((hook) => hook.command === `${command} start`))) {
  settings.hooks.SessionStart.push({ hooks: [{ type: "command", command: `${command} start`, timeout: 5 }] })
}
settings.statusLine = { type: "command", command: `${command} usage`, refreshInterval: 5 }
fs.mkdirSync(path.dirname(collector), { recursive: true })
fs.copyFileSync(path.join(__dirname, "claude-local-usage.cjs"), collector)
const backup = `${settingsFile}.usage-viewer-${Date.now()}.bak`
fs.writeFileSync(backup, original, { flag: "wx", mode: 0o600 })
const temporary = `${settingsFile}.usage-viewer.tmp`
fs.writeFileSync(temporary, `${JSON.stringify(settings, null, 2)}\n`, { mode: 0o600 })
if (fs.readFileSync(settingsFile, "utf8") !== original) {
  fs.unlinkSync(temporary)
  throw new Error("설정이 동시에 변경되어 설치를 중단했습니다.")
}
fs.renameSync(temporary, settingsFile)
console.log("Claude 로컬 수집 설정을 추가했습니다. 새 Claude Code 세션에서 첫 응답 후 수집합니다. 기존 설정 백업을 보존했습니다.")
