# Contributing a language pack

Nightlio's UI strings ship as **hot-swappable language packs**: a new
language (or a fix to an existing one, including English) is a standalone
GitHub release, fetched by `nightlio-api` at runtime. Adding or updating a
language never requires a server redeploy or a new app version.

The bundled catalog, [`src/i18n/en.json`](../src/i18n/en.json), is the
canonical, permanent fallback. It ships inside the app itself, so the app
always works fully in English even if the server is offline, `/api/i18n` is
unreachable, or no pack has been published yet. Every other language is an
*overlay* on top of it: a pack only needs to translate the strings it wants
to — anything it omits silently falls back to English (never a crash, and
never a blank string).

## How translation lookup works (for context)

The frontend runtime (`src/i18n/index.tsx`) resolves every string with:

```
pack.strings[key] ?? en[key] ?? key
```

in that order. Concretely:

- If the active pack has a translation for `nav.home`, that wins.
- Otherwise the bundled English value for `nav.home` is used.
- If the key doesn't exist in `en.json` at all (a typo, or a key from a
  future app version your pack hasn't caught up to), the raw key string is
  shown. This is deliberately visible/ugly rather than silently blank — it
  is a signal, not a bug, and it can only happen for genuinely unknown keys.

This is why **partial packs are completely safe**: you can translate 20% of
the catalog and ship it. Missing coverage just means those 80% of strings
stay in English for that language until someone fills them in.

## Authoring a pack

1. Copy the keys you want to translate out of `src/i18n/en.json`. You do not
   need to translate every key — start with whatever matters most (nav
   labels, settings, entry flow, ...) and expand later.

2. Create `i18n/packs/<code>.json` — next to the reference English pack,
   [`en.json`](packs/en.json), which is a complete worked example (generated
   from `src/i18n/en.json` by `scripts/build-lang-pack.mjs`) — shaped as the
   full pack envelope:

   ```json
   {
     "schema_version": 1,
     "language": "es",
     "name": "Spanish",
     "native_name": "Español",
     "version": "1.0.0",
     "strings": {
       "common": {
         "appName": "Nightlio",
         "cancel": "Cancelar"
       },
       "nav": {
         "home": "Inicio",
         "history": "Historial"
       }
     }
   }
   ```

   Field notes:
   - `language` — the code you're translating into (lowercase, e.g. `es`,
     `fr`, `pt-br`). This becomes the `GET /api/i18n/{code}` path segment.
   - `name` — the English display name of the language (e.g. `"Spanish"`).
   - `native_name` — the language's own name for itself, i.e. its
     **autonym** (e.g. `"Español"`, not `"Spanish"`). This is what the
     Settings language picker shows in the UI, in the language's own
     script/spelling. Never translate this into other languages.
   - `version` — a semver you control (start at `1.0.0`). Bump it whenever
     you update the pack; see [Releasing](#releasing-tagging--the-pipeline)
     below for why the *tag* is what actually matters at release time.
   - `strings` — a **nested** object mirroring `src/i18n/en.json` exactly:
     the easiest way to start is to copy `en.json`, paste it under
     `strings`, and translate the values in place. Each nesting level is one
     segment of the key's dot-path (`nav.home` lives at `strings.nav.home`).
     Do not invent new keys — a key your pack adds that doesn't exist in
     `en.json` is harmless (the client never looks it up) but is very likely
     a typo, and `validate-lang-pack.mjs` will warn about it. (Nested is
     also the wire format the API serves; the build step just normalizes.
     A flat `{"nav.home": "Inicio"}` source is accepted as input too and
     gets re-nested.)

3. **Interpolation placeholders.** Some English values contain `{name}`
   -style placeholders, e.g. `"common.dateAtTime": "{date} at {time}"`. Keep
   the placeholder tokens **verbatim** (same names, same `{curly braces}`)
   in your translation — you may reorder them for grammar, but do not
   rename or drop them:

   ```json
   "common": { "dateAtTime": "{date} a las {time}" }
   ```

   An unrecognized placeholder is left in the output untouched, so a typo'd
   placeholder name just means that literal `{typo}` text leaks into the
   UI — always double-check against the English source key.

4. **Plural keys.** Keys that vary by count are suffixed `.one` / `.other`
   in `en.json`, e.g.:

   ```json
   "goals.frequencyPerWeek.one": "{count} day a week",
   "goals.frequencyPerWeek.other": "{count} days a week"
   ```

   The app selects a suffix using the browser's [`Intl.PluralRules`] for the
   active language, falling back to `.other` and then to the bare base key
   if a specific category is missing. English only ever needs `.one`/
   `.other`, but some languages have more plural categories (`zero`, `two`,
   `few`, `many` — see [CLDR plural rules]). You **may** add any of those
   suffixes to a plural key in your pack (e.g. `stats.volatility.few`) and
   the runtime will pick them up automatically for languages whose
   `Intl.PluralRules` reports that category; always include `.other` as the
   universal fallback for that key.

   [`Intl.PluralRules`]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/PluralRules
   [CLDR plural rules]: https://cldr.unicode.org/index/cldr-spec/plural-rules

5. Validate locally before opening a PR:

   ```sh
   node scripts/validate-lang-pack.mjs i18n/packs/<code>.json
   ```

   This checks the envelope shape (all six keys, correct types,
   `schema_version` pinned to `1`) and reports key coverage against
   `src/i18n/en.json`. **Missing keys only warn** — partial packs are
   fine. **Unknown keys only warn** too (almost always a stale/renamed key;
   harmless at runtime, but worth a look). A **non-string value** inside
   `strings`, or a malformed envelope (wrong types, missing required
   fields, `schema_version` != `1`), is a hard **error** and fails
   validation (exit code 1).

6. Open a PR with your `i18n/packs/<code>.json` file.

## Releasing (tagging + the pipeline)

Releases are cut by a maintainer, after a pack PR merges:

```sh
git tag lang-<code>-v<version>   # e.g. lang-es-v1.0.0 — match your pack's "version" field
git push origin lang-<code>-v<version>
```

Pushing a tag matching `lang-*-v*` triggers
[`.github/workflows/i18n-release.yml`](../.github/workflows/i18n-release.yml), which:

1. Validates `i18n/packs/<code>.json` (`scripts/validate-lang-pack.mjs`).
2. Builds the release asset `<code>.json` (`scripts/build-lang-pack.mjs`),
   **re-stamping `language` and `version` from the tag itself** — the tag
   is authoritative, so even if the source file's own `version` field lags
   behind what you actually tagged, the published asset is correct.
3. Validates the built asset again, as a final safety net.
4. Publishes a GitHub **prerelease** (`gh release create --prerelease`)
   carrying the `<code>.json` asset.

`--prerelease` is load-bearing, not cosmetic: GitHub's `releases/latest` —
which is what `nightlio-api` / self-hoster upgrade tooling resolves to know
"what's the current app version" — always skips prereleases. Marking every
language-pack release as a prerelease guarantees `releases/latest` can never
accidentally resolve to a language pack instead of an actual application
release, no matter how many language packs get tagged.

`nightlio-api` discovers these releases (not `releases/latest` — its own
GitHub-releases listing, scoped to `lang-*-v*` tags) and serves the
translations from `GET /api/i18n/languages` and `GET /api/i18n/{code}`.

### English itself is hot-swappable

There is nothing special about English in this pipeline. The very first
release after this pipeline merges is **`lang-en-v1.0.0`** — a full pack
built from `src/i18n/en.json` itself. If a typo or wording issue is found
later, the fix is: update `i18n/packs/en.json`, bump its version, PR it, tag
`lang-en-v1.0.1` (or whatever's next) — no application redeploy needed.
(The *bundled* `src/i18n/en.json` inside the app keeps working as the
offline/first-boot fallback regardless; the server-fetched `en` pack is an
overlay on top of it exactly like every other language, per the lookup
order above.)

## Self-hosting offline / air-gapped

`nightlio-api`'s i18n discovery normally talks to the GitHub releases API to
find and download pack assets. Self-hosters who cannot or do not want that
(air-gapped networks, strict egress policies, or just not wanting to depend
on GitHub at runtime) have two environment variables:

- **`I18N_LOCAL_DIR`** — point this at a local directory containing pack
  files named `<code>.json` (the same nested envelope shape described
  above — produced by `scripts/build-lang-pack.mjs`, copied straight from a
  published release asset, or written by hand). When set, this takes precedence over the
  GitHub-releases discovery entirely — the server reads packs from disk
  instead of the network. This is the recommended offline setup: build or
  download the packs you want once, drop them in a directory, point
  `I18N_LOCAL_DIR` at it, done.

- **`I18N_OFFLINE`** — set this (e.g. `I18N_OFFLINE=1`) to disable GitHub
  discovery outright without necessarily supplying local packs. Combined
  with an empty/absent `I18N_LOCAL_DIR`, the server simply serves no
  language packs (`GET /api/i18n/languages` returns `{"languages": []}`,
  every `GET /api/i18n/{code}` 404s) — every client runs on bundled
  English, which always works. Combine with `I18N_LOCAL_DIR` to serve a
  fixed, curated set of packs with zero network dependency.

In every case — GitHub unreachable, `I18N_OFFLINE` set, a cold cache on a
fresh instance, or simply no pack published yet for a given code — the
client-side contract is the same: **English always works.** The bundled
`src/i18n/en.json` never depends on the network, the server, or any of
these settings.
