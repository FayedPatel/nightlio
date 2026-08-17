// Assemble the README GIFs from the keyframes captured by
// e2e/readme-captures.spec.ts (screenshots/readme-frames/<name>/*.png →
// docs/assets/<name>.gif). Pure Node — no ffmpeg/imagemagick needed:
//   node scripts/build-readme-gifs.mjs /path/with/node_modules
// The single argument is a directory whose node_modules contains gifenc and
// pngjs (they are not project dependencies; any scratch dir with
// `npm i gifenc pngjs` works).
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';

const toolDir = process.argv[2];
if (!toolDir) {
  console.error('usage: node scripts/build-readme-gifs.mjs <dir with node_modules containing gifenc+pngjs>');
  process.exit(1);
}
const requireFrom = createRequire(join(resolve(toolDir), 'noop.js'));
const { GIFEncoder, quantize, applyPalette } = requireFrom('gifenc');
const { PNG } = requireFrom('pngjs');

const FRAME_ROOT = 'screenshots/readme-frames';
const OUT_DIR = 'docs/assets';
// Per-GIF frame delay in ms; last frame lingers longer so the loop reads.
const DELAYS = { 'log-mood': 1400, themes: 1600 };

for (const name of readdirSync(FRAME_ROOT)) {
  const dir = join(FRAME_ROOT, name);
  const files = readdirSync(dir).filter(f => f.endsWith('.png')).sort();
  if (files.length === 0) continue;
  const delay = DELAYS[name] ?? 1200;
  const gif = GIFEncoder();
  for (const [i, file] of files.entries()) {
    const png = PNG.sync.read(readFileSync(join(dir, file)));
    const palette = quantize(png.data, 256);
    const indexed = applyPalette(png.data, palette);
    gif.writeFrame(indexed, png.width, png.height, {
      palette,
      delay: i === files.length - 1 ? delay * 2 : delay,
    });
  }
  gif.finish();
  const out = join(OUT_DIR, `${name}.gif`);
  writeFileSync(out, gif.bytes());
  console.log(`${out}: ${files.length} frames, ${(gif.bytes().length / 1024).toFixed(0)} KiB`);
}
