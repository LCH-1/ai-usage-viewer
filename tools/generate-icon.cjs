const { mkdirSync, writeFileSync, copyFileSync } = require("node:fs")
const { join } = require("node:path")
const { deflateSync } = require("node:zlib")

const size = 512
const samples = 4
const pixels = Buffer.alloc((size * 4 + 1) * size)

const transparent = [0, 0, 0, 0]
const border = [44, 52, 47, 255]
const tile = [16, 19, 17, 255]
const dark = [11, 15, 13, 255]
const white = [243, 248, 245, 255]
const mint = [98, 230, 164, 255]
const coral = [255, 118, 109, 255]

function insideRoundedRect(x, y, left, top, width, height, radius) {
  const right = left + width
  const bottom = top + height
  if (x < left || x > right || y < top || y > bottom) return false
  if (x >= left + radius && x <= right - radius) return true
  if (y >= top + radius && y <= bottom - radius) return true
  const centerX = x < left + radius ? left + radius : right - radius
  const centerY = y < top + radius ? top + radius : bottom - radius
  return (x - centerX) ** 2 + (y - centerY) ** 2 <= radius ** 2
}

function colorAt(x, y) {
  let color = transparent
  if (insideRoundedRect(x, y, 18, 18, 476, 476, 104)) color = border
  if (insideRoundedRect(x, y, 24, 24, 464, 464, 98)) color = tile

  if (insideRoundedRect(x, y, 86, 128, 340, 106, 53)) color = white
  if (insideRoundedRect(x, y, 106, 148, 300, 66, 33)) color = dark
  if (insideRoundedRect(x, y, 118, 160, 244, 42, 21)) color = mint

  if (insideRoundedRect(x, y, 86, 278, 340, 106, 53)) color = white
  if (insideRoundedRect(x, y, 106, 298, 300, 66, 33)) color = dark
  if (insideRoundedRect(x, y, 118, 310, 128, 42, 21)) color = coral
  return color
}

for (let y = 0; y < size; y += 1) {
  const row = y * (size * 4 + 1)
  pixels[row] = 0
  for (let x = 0; x < size; x += 1) {
    const totals = [0, 0, 0, 0]
    for (let sampleY = 0; sampleY < samples; sampleY += 1) {
      for (let sampleX = 0; sampleX < samples; sampleX += 1) {
        const color = colorAt(x + (sampleX + 0.5) / samples, y + (sampleY + 0.5) / samples)
        for (let channel = 0; channel < 4; channel += 1) totals[channel] += color[channel]
      }
    }
    const index = row + 1 + x * 4
    for (let channel = 0; channel < 4; channel += 1) pixels[index + channel] = Math.round(totals[channel] / samples ** 2)
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
