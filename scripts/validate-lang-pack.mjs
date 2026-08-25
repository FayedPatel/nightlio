#!/usr/bin/env node
// Validates a nightlio language-pack envelope file.
//
// Usage:
//   node scripts/validate-lang-pack.mjs <pack-file.json> [--en <en.json>]
//
// Checks (contract: contract/openapi.yaml LanguagePack schema, DECISIONS.md
// 2026-08-22):
//   - the file parses as JSON and is a plain object (not an array/null/etc)
//   - the envelope carries exactly: schema_version (must be the integer 1),
//     language / name / native_name / version (non-empty strings; language
//     must additionally match the release-tag code grammar — [a-z]{2,8}
//     with at most one -[A-Za-z0-9]+ subtag, in lockstep with the consumer
//     in api/src/i18n.rs::valid_code), and
//     strings — either the flat dot-key wire map or a nested object like
//     src/i18n/en.json (nested sources are flattened before checking; the
//     wire format itself stays flat)
//   - every string leaf inside `strings` is itself a string
//   - key coverage against src/i18n/en.json, the bundled canonical catalog:
//       * keys present in en.json but missing from the pack  -> WARN only.
//         Partial packs are safe by design: the client lookup order is
//         pack.strings[key] ?? en[key] ?? key, so a partial pack simply
//         falls back to English (or worse, the literal key) for whatever
//         it does not translate yet. This is never a reason to fail CI.
//       * keys present in the pack but NOT in en.json -> WARN only. Almost
//         always a stale/renamed key from an older catalog version; still
//         harmless at runtime (the client just never looks it up), so it
//         warns rather than blocks a release.
//
// Exit codes: 0 when the envelope shape is valid (regardless of warnings),
// 1 when the envelope is malformed OR any `strings` value is not a string.
// Warnings and errors are both printed to stderr so stdout stays quiet for
// scripting; a one-line human summary goes to stdout on success.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(SCRIPT_DIR, '..');
const DEFAULT_EN_PATH = path.join(REPO_ROOT, 'src/i18n/en.json');

function parseArgs(argv) {
  const args = { file: null, en: DEFAULT_EN_PATH };
  const rest = [...argv];
  while (rest.length > 0) {
    const arg = rest.shift();
    if (arg === '--en') {
      const value = rest.shift();
      if (!value) {
        throw new Error('--en requires a path argument');
      }
      args.en = path.resolve(value);
    } else if (arg === '--help' || arg === '-h') {
      args.help = true;
    } else if (!args.file) {
      args.file = arg;
    } else {
      throw new Error(`unexpected argument: ${arg}`);
    }
  }
  return args;
}

function readJson(filePath, label) {
  let raw;
  try {
    raw = readFileSync(filePath, 'utf8');
  } catch (error) {
    throw new Error(`could not read ${label} at ${filePath}: ${error.message}`);
  }
  try {
    return JSON.parse(raw);
  } catch (error) {
    throw new Error(`${label} at ${filePath} is not valid JSON: ${error.message}`);
  }
}

function isPlainObject(value) {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * Flattens a (possibly nested) strings object into dot-keys — the wire
 * format. Identity on an already-flat map. Non-string, non-object leaves are
 * recorded in badValueKeys under their dot-path.
 */
function flattenStrings(node, prefix, out, badValueKeys) {
  for (const [key, value] of Object.entries(node)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === 'string') {
      out[path] = value;
    } else if (isPlainObject(value)) {
      flattenStrings(value, path, out, badValueKeys);
    } else {
      badValueKeys.push(path);
    }
  }
  return out;
}

/**
 * Validates the envelope shape + key coverage. Returns { errors, warnings,
 * info } — never throws for a bad pack, only for I/O problems the caller
 * surfaces separately. `info` is null when the envelope itself is too
 * malformed to describe (e.g. not an object at all).
 */
export function validatePack(candidate, enCatalog) {
  const errors = [];
  const warnings = [];

  if (!isPlainObject(candidate)) {
    errors.push('pack must be a JSON object (envelope), not an array/primitive/null');
    return { errors, warnings, info: null };
  }

  if (candidate.schema_version !== 1) {
    errors.push(
      `schema_version must be the integer 1, got ${JSON.stringify(candidate.schema_version)}`,
    );
  }

  for (const field of ['language', 'name', 'native_name', 'version']) {
    const value = candidate[field];
    if (typeof value !== 'string' || value.length === 0) {
      errors.push(`"${field}" must be a non-empty string, got ${JSON.stringify(value)}`);
    }
  }

  // Language-code grammar, kept in lockstep with the consumer in
  // api/src/i18n.rs::valid_code (and the release-tag patterns in
  // scripts/build-lang-pack.mjs / .github/workflows/i18n-release.yml):
  // [a-z]{2,8} optionally followed by one -[A-Za-z0-9]+ subtag. A code the
  // server-side parser rejects can never be discovered or served, so a
  // mismatch here is a hard error, not a warning.
  if (
    typeof candidate.language === 'string' &&
    candidate.language.length > 0 &&
    !/^[a-z]{2,8}(?:-[A-Za-z0-9]+)?$/.test(candidate.language)
  ) {
    errors.push(
      `"language" must match [a-z]{2,8} with at most one -[A-Za-z0-9]+ subtag ` +
        `(api/src/i18n.rs valid_code), got ${JSON.stringify(candidate.language)}`,
    );
  }

  let strings = null;
  if (!isPlainObject(candidate.strings)) {
    errors.push('"strings" must be a JSON object mapping keys to string values');
  } else {
    // Source packs may be authored NESTED (like src/i18n/en.json); release
    // assets on the wire are flat. Flattening is identity on a flat map, so
    // one path handles both. Non-string leaves are collected as errors.
    const badValueKeys = [];
    strings = flattenStrings(candidate.strings, '', {}, badValueKeys);
    if (badValueKeys.length > 0) {
      errors.push(
        `"strings" has ${badValueKeys.length} non-string value(s): ${badValueKeys.slice(0, 10).join(', ')}` +
          (badValueKeys.length > 10 ? ', ...' : ''),
      );
    }
  }

  let missingKeys = [];
  let unknownKeys = [];
  if (strings !== null && isPlainObject(enCatalog)) {
    // en.json is authored nested; flatten to dot-keys before comparing.
    const enKeys = new Set(Object.keys(flattenStrings(enCatalog, '', {}, [])));
    const packKeys = new Set(Object.keys(strings));
    missingKeys = [...enKeys].filter((key) => !packKeys.has(key));
    unknownKeys = [...packKeys].filter((key) => !enKeys.has(key));

    if (missingKeys.length > 0) {
      warnings.push(
        `${missingKeys.length} key(s) from en.json are missing from this pack (partial pack — ` +
          `safe, falls back to English/key): ${missingKeys.slice(0, 10).join(', ')}` +
          (missingKeys.length > 10 ? ', ...' : ''),
      );
    }
    if (unknownKeys.length > 0) {
      warnings.push(
        `${unknownKeys.length} key(s) in this pack are not in en.json (stale/unknown key, ` +
          `harmless at runtime): ${unknownKeys.slice(0, 10).join(', ')}` +
          (unknownKeys.length > 10 ? ', ...' : ''),
      );
    }
  }

  const info =
    errors.length === 0 || strings !== null
      ? {
          language: candidate.language,
          name: candidate.name,
          native_name: candidate.native_name,
          version: candidate.version,
          stringCount: strings ? Object.keys(strings).length : 0,
          missingCount: missingKeys.length,
          unknownCount: unknownKeys.length,
        }
      : null;

  return { errors, warnings, info };
}

function main(argv) {
  let args;
  try {
    args = parseArgs(argv);
  } catch (error) {
    console.error(`error: ${error.message}`);
    return 1;
  }

  if (args.help || !args.file) {
    console.error('Usage: node scripts/validate-lang-pack.mjs <pack-file.json> [--en <en.json>]');
    return args.help ? 0 : 1;
  }

  const packPath = path.resolve(args.file);

  let candidate;
  let enCatalog;
  try {
    candidate = readJson(packPath, 'pack file');
    enCatalog = readJson(args.en, 'reference catalog');
  } catch (error) {
    console.error(`error: ${error.message}`);
    return 1;
  }

  const { errors, warnings, info } = validatePack(candidate, enCatalog);

  for (const warning of warnings) {
    console.error(`warn: ${warning}`);
  }
  for (const error of errors) {
    console.error(`error: ${error}`);
  }

  if (info) {
    console.log(
      `${packPath}: language=${info.language} name=${info.name} native_name=${info.native_name} ` +
        `version=${info.version} strings=${info.stringCount} missing=${info.missingCount} ` +
        `unknown=${info.unknownCount} errors=${errors.length} warnings=${warnings.length}`,
    );
  }

  if (errors.length > 0) {
    console.error(`FAIL: ${packPath} is not a valid language pack (${errors.length} error(s)).`);
    return 1;
  }

  console.log(`OK: ${packPath} is a valid language pack (${warnings.length} warning(s)).`);
  return 0;
}

// Only run as a CLI when invoked directly (not when imported, e.g. by tests
// or build-lang-pack.mjs's own self-check).
if (import.meta.url === `file://${process.argv[1]}`) {
  process.exit(main(process.argv.slice(2)));
}
