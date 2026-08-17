// Assemble the README GIFs from the frames captured by
// e2e/readme-captures.spec.ts (screenshots/readme-frames/<name>/*.png →
// docs/assets/<name>.gif). Pure Node — no ffmpeg/imagemagick needed:
//   node scripts/build-readme-gifs.mjs /path/with/node_modules
// The single argument is a directory whose node_modules contains gifenc and
// pngjs (they are not project dependencies; any scratch dir with
// `npm i gifenc pngjs` works).
//
// Two frame-naming conventions:
//   NNNN-<ms>.png  — CDP screencast frames; the <ms> offsets become real
//                    per-frame delays (frames closer than MIN_DELAY ms are
//                    merged so hundreds of typing frames stay compact).
//   anything else  — a discrete keyframe slideshow; fixed delay per GIF.
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
const SLIDESHOW_DELAYS = { themes: 1600 };
const MIN_DELAY = 70; // ms — merge screencast frames arriving faster than this
const LAST_FRAME_LINGER = 1800;

for (const name of readdirSync(FRAME_ROOT)) {
  const dir = join(FRAME_ROOT, name);
  const files = readdirSync(dir).filter(f => f.endsWith('.png')).sort();
  if (files.length === 0) continue;

  const stamped = files.map(f => {
    const m = /^\d+-(\d+)\.png$/.exec(f);
    return { file: f, ms: m ? Number(m[1]) : null };
  });
  const isScreencast = stamped.every(s => s.ms !== null);

  // Pick frames + their display delays.
  const picked = [];
  if (isScreencast) {
    for (const s of stamped) {
      if (picked.length === 0) {
        picked.push({ file: s.file, ms: s.ms });
        continue;
      }
      const prev = picked[picked.length - 1];
      if (s.ms - prev.ms < MIN_DELAY) {
        prev.replacedBy = s.file; // keep the newest look for this time slot
        continue;
      }
      picked.push({ file: s.file, ms: s.ms });
    }
  } else {
    const fixed = SLIDESHOW_DELAYS[name] ?? 1200;
    for (const s of stamped) picked.push({ file: s.file, ms: null, fixed });
  }

  const gif = GIFEncoder();
  for (const [i, p] of picked.entries()) {
    const file = p.replacedBy ?? p.file;
    const png = PNG.sync.read(readFileSync(join(dir, file)));
    const palette = quantize(png.data, 256);
    const indexed = applyPalette(png.data, palette);
    let delay;
    if (p.ms === null) {
      delay = i === picked.length - 1 ? p.fixed * 2 : p.fixed;
    } else {
      const next = picked[i + 1];
      delay = next ? Math.min(next.ms - p.ms, 2500) : LAST_FRAME_LINGER;
    }
    gif.writeFrame(indexed, png.width, png.height, { palette, delay });
  }
  gif.finish();
  const out = join(OUT_DIR, `${name}.gif`);
  writeFileSync(out, gif.bytes());
  console.log(
    `${out}: ${picked.length}/${files.length} frames, ${(gif.bytes().length / 1024 / 1024).toFixed(2)} MiB`,
  );
}
