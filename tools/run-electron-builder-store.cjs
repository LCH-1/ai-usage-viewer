const { delimiter, join } = require("node:path")
const { spawnSync } = require("node:child_process")

const executable = join(process.cwd(), "node_modules", ".bin", "electron-builder.cmd")
const env = {
  ...process.env,
  PATH: `${join(process.cwd(), "tools")}${delimiter}${process.env.PATH ?? ""}`,
}
const result = spawnSync(executable, ["--win", "appx", "--publish", "never"], { env, stdio: "inherit", shell: true })

if (result.error) throw result.error
process.exit(result.status ?? 1)
