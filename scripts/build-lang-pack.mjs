#!/usr/bin/env node
// Assembles the release asset `<code>.json` for a nightlio language pack.
//
// Usage:
//   node scripts/build-lang-pack.mjs <tag> [--source <path>] [--out <path>]
//     [--name <English name>] [--native-name <native name>]
//
// <tag> must match `lang-<code>-v<semver>` (e.g. `lang-es-v1.0.0`) — this is
// the git tag that .github/workflows/i18n-release.yml runs on. The tag is
// the AUTHORITATIVE source for both `language` (the code segment) and
// `version` (the semver segment): whatever the source file says in its own
// `language`/`version`/`schema_version` fields is overwritten to match the
// tag, so a maintainer can never accidentally ship a mismatched pair by
// forgetting to bump the source file in lockstep with the tag.
//
// Source file (default `i18n/packs/<code>.json`, see i18n/README.md for the
// contributor workflow) may be either:
//   (a) a near-complete envelope: { name, native_name, strings, ... } —
//       `schema_version`/`language`/`version` are ignored/overwritten; or
//   (b) a bare strings map with no `strings` key at all — the whole object
//       is treated as the strings map, and --name/--native-name become
//       required flags in that case.
// Either way the strings may be authored NESTED (the source format, like
// src/i18n/en.json) or flat dot-keys; the emitted release asset is always
// canonical NESTED form — the wire format mirrors the source catalog.
//
// Output is the full six-key pack envelope (contract/openapi.yaml
// LanguagePack schema): { schema_version: 1, language, name, native_name,
// version, strings }, pretty-printed with a trailing newline.
//
// This script does not itself run key-coverage validation — pair it with
// scripts/validate-lang-pack.mjs (before AND after, see the release
// workflow) which does.

import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';

// Kept in lockstep with the consumer grammar in
// api/src/i18n.rs::parse_release_tag + valid_code (and the bash regex in
// .github/workflows/i18n-release.yml): code is [a-z]{2,8} with at most one
// -[A-Za-z0-9]+ subtag, version is a strict \d+.\d+.\d+ triple — no
// prerelease/build suffix. A tag the server-side parser would reject must
// fail here too, or the release ships an asset nightlio-api never discovers.
const TAG_PATTERN = /^lang-([a-z]{2,8}(?:-[A-Za-z0-9]+)?)-v(\d+\.\d+\.\d+)$/;

function parseArgs(argv) {
  const args = { tag: null, source: null, out: null, name: null, nativeName: null };
  const rest = [...argv];
  while (rest.length > 0) {
    const arg = rest.shift();
    switch (arg) {
      case '--source':
        args.source = rest.shift() ?? null;
        break;
      case '--out':
        args.out = rest.shift() ?? null;
        break;
      case '--name':
        args.name = rest.shift() ?? null;
        break;
      case '--native-name':
        args.nativeName = rest.shift() ?? null;
        break;
      case '--help':
      case '-h':
        args.help = true;
        break;
      default:
        if (args.tag) {
          throw new Error(`unexpected argument: ${arg}`);
        }
        args.tag = arg;
    }
  }
  return args;
}

function isPlainObject(value) {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Pure transform, exported for unit testing without touching the filesystem. */
export function buildEnvelope({ tag, source, name, nativeName }) {
  const match = TAG_PATTERN.exec(tag);
  if (!match) {
    throw new Error(
      `tag "${tag}" does not match lang-<code>-v<semver> (e.g. lang-es-v1.0.0)`,
    );
  }
  const [, code, version] = match;

  if (!isPlainObject(source)) {
    throw new Error('source pack must be a JSON object');
  }

  const rawStrings = isPlainObject(source.strings) ? source.strings : source;
  if (!isPlainObject(rawStrings)) {
    throw new Error('could not locate a strings map in the source pack');
  }
  // The wire format is NESTED, mirroring src/i18n/en.json. Normalize the
  // source into canonical nested form: flatten first (identity on nested-only
  // input, folds in any flat dot-keys from older sources), rejecting leaves
  // that are neither strings nor nested objects, then rebuild the nest.
  const flat = {};
  const flattenInto = (node, prefix) => {
    for (const [key, value] of Object.entries(node)) {
      const path = prefix ? `${prefix}.${key}` : key;
      if (typeof value === 'string') {
        flat[path] = value;
      } else if (isPlainObject(value)) {
        flattenInto(value, path);
      } else {
        throw new Error(`source strings["${path}"] is not a string: ${JSON.stringify(value)}`);
      }
    }
  };
  flattenInto(rawStrings, '');
  const strings = {};
  for (const [path, value] of Object.entries(flat)) {
    const parts = path.split('.');
    let node = strings;
    for (const part of parts.slice(0, -1)) {
      if (typeof node[part] === 'string') {
        throw new Error(`key conflict: "${path}" nests under a string leaf`);
      }
      node = node[part] ??= {};
    }
    const leaf = parts[parts.length - 1];
    if (isPlainObject(node[leaf])) {
      throw new Error(`key conflict: "${path}" is both a leaf and a parent`);
    }
    node[leaf] = value;
  }

  const resolvedName = name ?? (typeof source.name === 'string' ? source.name : null);
  const resolvedNativeName =
    nativeName ?? (typeof source.native_name === 'string' ? source.native_name : null);

  if (!resolvedName) {
    throw new Error('missing "name" — supply it in the source file or via --name');
  }
  if (!resolvedNativeName) {
    throw new Error('missing "native_name" — supply it in the source file or via --native-name');
  }

  return {
    schema_version: 1,
    language: code,
    name: resolvedName,
    native_name: resolvedNativeName,
    version,
    strings,
  };
}

function main(argv) {
  let args;
  try {
    args = parseArgs(argv);
  } catch (error) {
    console.error(`error: ${error.message}`);
    return 1;
  }

  if (args.help || !args.tag) {
    console.error(
      'Usage: node scripts/build-lang-pack.mjs <tag> [--source <path>] [--out <path>] ' +
        '[--name <name>] [--native-name <native name>]',
    );
    return args.help ? 0 : 1;
  }

  const match = TAG_PATTERN.exec(args.tag);
  if (!match) {
    console.error(
      `error: tag "${args.tag}" does not match lang-<code>-v<semver> (e.g. lang-es-v1.0.0)`,
    );
    return 1;
  }
  const [, code] = match;

  const sourcePath = path.resolve(args.source ?? `i18n/packs/${code}.json`);
  const outPath = path.resolve(args.out ?? `${code}.json`);

  let source;
  try {
    source = JSON.parse(readFileSync(sourcePath, 'utf8'));
  } catch (error) {
    console.error(`error: could not read/parse source pack at ${sourcePath}: ${error.message}`);
    return 1;
  }

  let envelope;
  try {
    envelope = buildEnvelope({ tag: args.tag, source, name: args.name, nativeName: args.nativeName });
  } catch (error) {
    console.error(`error: ${error.message}`);
    return 1;
  }

  mkdirSync(path.dirname(outPath), { recursive: true });
  writeFileSync(outPath, `${JSON.stringify(envelope, null, 2)}\n`, 'utf8');

  const countLeaves = (node) =>
    Object.values(node).reduce(
      (sum, value) => sum + (typeof value === 'string' ? 1 : countLeaves(value)),
      0,
    );
  console.log(
    `OK: wrote ${outPath} (language=${envelope.language} version=${envelope.version} ` +
      `strings=${countLeaves(envelope.strings)})`,
  );
  return 0;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  process.exit(main(process.argv.slice(2)));
}
