const { mkdirSync, writeFileSync, copyFileSync } = require("node:fs")
const { join } = require("node:path")
const { deflateSync } = require("node:zlib")

const size = 512
const pixels = Buffer.alloc((size * 4 + 1) * size)

function insideRoundedRect(x, y, inset, radius) {
  const left = inset
  const top = inset
  const right = size - inset - 1
  const bottom = size - inset - 1
  if (x >= left + radius && x <= right - radius) return y >= top && y <= bottom
  if (y >= top + radius && y <= bottom - radius) return x >= left && x <= right
  const cx = x < left + radius ? left + radius : right - radius
  const cy = y < top + radius ? top + radius : bottom - radius
  return (x - cx) ** 2 + (y - cy) ** 2 <= radius ** 2
}

function isMark(x, y) {
  if ((x >= 150 && x <= 198 || x >= 314 && x <= 362) && y >= 132 && y <= 302) return true
  const distance = Math.hypot(x - 256, y - 300)
  return y >= 294 && distance >= 58 && distance <= 106
}

for (let y = 0; y < size; y += 1) {
  const row = y * (size * 4 + 1)
  pixels[row] = 0
  for (let x = 0; x < size; x += 1) {
    const index = row + 1 + x * 4
    const outer = insideRoundedRect(x, y, 18, 104)
    const inner = insideRoundedRect(x, y, 28, 94)
    let rgba = [0, 0, 0, 0]
    if (outer) rgba = inner ? [10, 13, 11, 255] : [51, 68, 58, 255]
    if (inner && isMark(x, y)) {
      const glow = Math.max(0, Math.min(1, (x + y) / 1024))
      rgba = [Math.round(87 + glow * 28), Math.round(219 + glow * 20), Math.round(145 + glow * 18), 255]
    }
    pixels[index] = rgba[0]
    pixels[index + 1] = rgba[1]
    pixels[index + 2] = rgba[2]
    pixels[index + 3] = rgba[3]
  }
}

function crc32(buffer) {
  let crc = 0xffffffff
  for (const byte of buffer) {
    crc ^= byte
    for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1))
  }
  return (crc ^ 0xffffffff) >>> 0
}

function chunk(type, data) {
  const name = Buffer.from(type)
  const output = Buffer.alloc(data.length + 12)
  output.writeUInt32BE(data.length, 0)
  name.copy(output, 4)
  data.copy(output, 8)
  output.writeUInt32BE(crc32(Buffer.concat([name, data])), data.length + 8)
  return output
}

const header = Buffer.alloc(13)
header.writeUInt32BE(size, 0)
header.writeUInt32BE(size, 4)
header[8] = 8
header[9] = 6
const png = Buffer.concat([
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
  chunk("IHDR", header),
  chunk("IDAT", deflateSync(pixels, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
])

const buildDirectory = join(process.cwd(), "build")
const publicDirectory = join(process.cwd(), "public")
mkdirSync(buildDirectory, { recursive: true })
mkdirSync(publicDirectory, { recursive: true })
writeFileSync(join(buildDirectory, "icon.png"), png)
copyFileSync(join(buildDirectory, "icon.png"), join(publicDirectory, "icon.png"))
