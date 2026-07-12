// 零依赖生成一张纯色 1024×1024 PNG 作为应用图标源图（占位；视觉细节留到后续切片打磨）。
// 用法：node src-tauri/gen-icon.cjs  → 产出 src-tauri/app-icon.png
//       npm run tauri -- icon src-tauri/app-icon.png  → 生成 src-tauri/icons/ 全套
const zlib = require('zlib');
const fs = require('fs');
const path = require('path');

const W = 1024;
const H = 1024;
// 近似设计 token 的强调色（深底上的蓝），仅占位。
const [R, G, B, A] = [0x6e, 0x8e, 0xff, 0xff];

const crcTable = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = crcTable[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([len, body, crc]);
}

const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(W, 0);
ihdr.writeUInt32BE(H, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type RGBA
const row = Buffer.alloc(1 + W * 4);
for (let x = 0; x < W; x++) {
  row[1 + x * 4] = R;
  row[1 + x * 4 + 1] = G;
  row[1 + x * 4 + 2] = B;
  row[1 + x * 4 + 3] = A;
}
const raw = Buffer.concat(Array.from({ length: H }, () => row));
const idat = zlib.deflateSync(raw, { level: 9 });
const png = Buffer.concat([
  sig,
  chunk('IHDR', ihdr),
  chunk('IDAT', idat),
  chunk('IEND', Buffer.alloc(0)),
]);
const out = path.join(__dirname, 'app-icon.png');
fs.writeFileSync(out, png);
console.log('wrote', out, png.length, 'bytes');
