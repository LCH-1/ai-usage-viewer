const { copyFileSync, mkdirSync, readdirSync, statSync } = require("node:fs")
const { delimiter, dirname, join, resolve } = require("node:path")
const { spawnSync } = require("node:child_process")

const root = resolve(__dirname, "..")
const cli = join(root, "node_modules", ".bin", process.platform === "win32" ? "tauri.cmd" : "tauri")
const environment = { ...process.env }
const command = cli
const args = ["build"]

if (process.platform === "win32") {
  environment.PATH = `${dirname(process.execPath)}${delimiter}${join(environment.USERPROFILE, ".cargo", "bin")}${delimiter}${environment.PATH}`
}

const result = spawnSync(command, args, { cwd: root, env: environment, stdio: "inherit", shell: process.platform === "win32" })
if (result.status !== 0) process.exit(result.status ?? 1)

const release = join(root, "release")
const target = join(root, "src-tauri", "target", "release")
const nsis = join(target, "bundle", "nsis")
const version = require(join(root, "package.json")).version
mkdirSync(release, { recursive: true })

const setup = readdirSync(nsis)
  .map((name) => join(nsis, name))
  .find((path) => statSync(path).isFile() && path.includes(version) && path.toLowerCase().endsWith("-setup.exe"))
if (!setup) throw new Error("Tauri NSIS installer was not created.")

copyFileSync(setup, join(release, `Usage-Viewer-Setup-${version}-x64.exe`))
copyFileSync(join(target, "ai-usage-viewer.exe"), join(release, `Usage-Viewer-Portable-${version}-x64.exe`))
