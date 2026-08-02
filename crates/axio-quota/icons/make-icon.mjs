// Generates icons/icon.ico.
//
// Kept next to its output so the asset is reproducible rather than a binary nobody can
// regenerate. `node icons/make-icon.mjs` from the crate root.
//
// The mark is a ring: a quota is a gauge, and a ring reads as one at 16px where anything
// with interior detail turns to mush. Amber is axio's accent (--accent #f59e0b in
// apps/site), and the ring is left open at the top-right so it does not read as a full
// circle — a full ring at 100% would say the opposite of what the app is for.

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const SIZE = 32;
const OUT = fileURLToPath(new URL("./icon.ico", import.meta.url));

const ACCENT = { r: 0xf5, g: 0x9e, b: 0x0b };
const OUTER = SIZE / 2 - 1.5;
const INNER = OUTER - 5;

// BGRA, bottom-up, as a BMP inside an ICO wants it.
const xor = Buffer.alloc(SIZE * SIZE * 4);
for (let row = 0; row < SIZE; row += 1) {
  for (let col = 0; col < SIZE; col += 1) {
    // Bottom-up: row 0 of the buffer is the bottom row of the image.
    const y = SIZE - 1 - row;
    const dx = col - (SIZE - 1) / 2;
    const dy = y - (SIZE - 1) / 2;
    const distance = Math.hypot(dx, dy);

    // Anti-aliased annulus: coverage falls off over one pixel at each edge.
    let coverage =
      clamp(OUTER - distance + 0.5) * clamp(distance - INNER + 0.5);

    // The gap, from noon clockwise to about two o'clock.
    const angle = Math.atan2(-dy, dx);
    if (angle > Math.PI / 6 && angle < Math.PI / 2) coverage = 0;

    const offset = (row * SIZE + col) * 4;
    const alpha = Math.round(coverage * 255);
    xor[offset] = ACCENT.b;
    xor[offset + 1] = ACCENT.g;
    xor[offset + 2] = ACCENT.r;
    xor[offset + 3] = alpha;
  }
}

function clamp(value) {
  return Math.max(0, Math.min(1, value));
}

// 1bpp AND mask, rows padded to 4 bytes. All zero: the alpha channel does the masking,
// but the field is not optional in the format.
const maskRowBytes = Math.ceil(SIZE / 8 / 4) * 4;
const and = Buffer.alloc(maskRowBytes * SIZE);

const header = Buffer.alloc(40);
header.writeUInt32LE(40, 0); // biSize
header.writeInt32LE(SIZE, 4); // biWidth
header.writeInt32LE(SIZE * 2, 8); // biHeight: XOR and AND stacked
header.writeUInt16LE(1, 12); // biPlanes
header.writeUInt16LE(32, 14); // biBitCount
header.writeUInt32LE(0, 16); // biCompression = BI_RGB

const image = Buffer.concat([header, xor, and]);

const dir = Buffer.alloc(6);
dir.writeUInt16LE(0, 0);
dir.writeUInt16LE(1, 2); // type: icon
dir.writeUInt16LE(1, 4); // one image

const entry = Buffer.alloc(16);
entry.writeUInt8(SIZE, 0);
entry.writeUInt8(SIZE, 1);
entry.writeUInt8(0, 2); // no palette
entry.writeUInt8(0, 3);
entry.writeUInt16LE(1, 4); // planes
entry.writeUInt16LE(32, 6); // bit count
entry.writeUInt32LE(image.length, 8);
entry.writeUInt32LE(6 + 16, 12); // offset past the directory

mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, Buffer.concat([dir, entry, image]));
console.log(`wrote ${OUT} (${6 + 16 + image.length} bytes)`);
