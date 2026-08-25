import { createContext, useCallback, useContext, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import en from './en.json';

/**
 * i18n runtime (v0.6.0 hot-swappable language packs).
 *
 * The bundled `en.json` is the permanent fallback catalog: the app must work
 * fully standalone on it (API down / offline => English always works). A
 * server-fetched language pack, when present, overlays it — lookup order is
 * pack.strings[key] ?? en[key] ?? key, so partial packs are always safe.
 *
 * Server fetch wiring lives in src/i18n/sync.tsx and the Settings language
 * picker in src/views/SettingsView.tsx, both backed by the API's shipped
 * /api/i18n routes; this module provides the provider/hook, the pack
 * overlay + storage, and module-level accessors for non-component code.
 */

/**
 * The bundled catalog is authored as a NESTED object (translator-friendly
 * source format); the runtime flattens it to dot-keys at module load. The
 * WIRE format is nested too, mirroring this source format per the contract;
 * sanitizePack flattens fetched packs into the same flat dot-key lookup map.
 */
type LeafPaths<T> = {
  [K in keyof T & string]: T[K] extends string ? K : `${K}.${LeafPaths<T[K]>}`;
}[keyof T & string];

/** Every dot-path key in the bundled catalog (typed — typos fail `yarn typecheck`). */
export type I18nKey = LeafPaths<typeof en>;

/**
 * Base keys of plural pairs: for "goals.frequencyPerWeek.one"/".other" the
 * base "goals.frequencyPerWeek" is what callers pass together with a numeric
 * `count` param; t() appends the Intl.PluralRules category.
 */
type PluralBaseKey = { [K in I18nKey]: K extends `${infer B}.other` ? B : never }[I18nKey];

/** What t() accepts: a full catalog key, or a plural base key (+ `count`). */
export type TranslateKey = I18nKey | PluralBaseKey;

export type TranslateParams = Record<string, string | number>;
export type TranslateFn = (key: TranslateKey, params?: TranslateParams) => string;

/**
 * Validated language-pack envelope. The wire `strings` object is nested
 * (mirroring en.json); sanitizePack flattens it, so this internal shape is
 * always the flat dot-key map lookups use.
 */
export interface LanguagePack {
  schema_version: 1;
  language: string;
  strings: Record<string, string>;
}

// Storage keys mirror the ThemeContext pattern (versioned, try/catch reads).
const PACK_STORAGE_KEY = 'nightlio:i18n:pack:v1';
const LANG_STORAGE_KEY = 'nightlio:lang:v1';

// Flatten the nested source catalog into the flat dot-key map every lookup
// (and the wire format) uses; with noUncheckedIndexedAccess every index read
// is `string | undefined`.
const flattenCatalog = (
  node: unknown,
  prefix = '',
  out: Record<string, string> = {},
): Record<string, string> => {
  if (typeof node !== 'object' || node === null) return out;
  for (const [key, value] of Object.entries(node)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === 'string') out[path] = value;
    else flattenCatalog(value, path, out);
  }
  return out;
};

const EN_STRINGS: Record<string, string> = flattenCatalog(en);
const EMPTY_STRINGS: Record<string, string> = {};

/**
 * Validate an untrusted candidate pack. The wire format nests `strings`
 * (mirroring en.json; a flat dot-key map is the degenerate nesting and still
 * accepted). Returns a sanitized copy with `strings` FLATTENED to the
 * dot-key map every lookup uses (invalid leaves dropped), or null when the
 * envelope shape is wrong — wrong schema_version, missing language, or a
 * non-object strings map.
 */
export const sanitizePack = (candidate: unknown): LanguagePack | null => {
  if (typeof candidate !== 'object' || candidate === null || Array.isArray(candidate)) return null;
  const record = candidate as Record<string, unknown>;
  if (record['schema_version'] !== 1) return null;
  const language = record['language'];
  if (typeof language !== 'string' || language.length === 0) return null;
  const rawStrings = record['strings'];
  if (typeof rawStrings !== 'object' || rawStrings === null || Array.isArray(rawStrings)) return null;
  const strings: Record<string, string> = {};
  const collect = (node: object, prefix: string) => {
    for (const [key, value] of Object.entries(node)) {
      const path = prefix ? `${prefix}.${key}` : key;
      if (typeof value === 'string') strings[path] = value;
      else if (typeof value === 'object' && value !== null && !Array.isArray(value)) collect(value, path);
      // Anything else (numbers, arrays, null) is dropped — untrusted input.
    }
  };
  collect(rawStrings, '');
  return { schema_version: 1, language, strings };
};

// Last-good pack, overlaid synchronously at boot. Corrupted or invalid
// stored data degrades to "no pack" (bundled English keeps working).
const readStoredPack = (): LanguagePack | null => {
  try {
    const raw = localStorage.getItem(PACK_STORAGE_KEY);
    if (!raw) return null;
    return sanitizePack(JSON.parse(raw));
  } catch {
    return null;
  }
};

const readStoredLang = (): string => {
  try {
    const stored = localStorage.getItem(LANG_STORAGE_KEY);
    return stored && stored.length > 0 ? stored : 'en';
  } catch {
    return 'en';
  }
};

// {name}-style interpolation via one regex replace. Unknown placeholders are
// left verbatim so a stale pack can never eat surrounding text.
const interpolate = (template: string, params?: TranslateParams): string => {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) => {
    const value = params[name];
    return value === undefined ? match : String(value);
  });
};

const pluralRulesFor = (lang: string): Intl.PluralRules => {
  try {
    return new Intl.PluralRules(lang);
  } catch {
    // Unknown/garbage language codes (packs are untrusted input).
    return new Intl.PluralRules('en');
  }
};

const buildTranslate = (lang: string, packStrings: Record<string, string>): TranslateFn => {
  const rules = pluralRulesFor(lang);
  const resolve = (key: string): string | undefined => packStrings[key] ?? EN_STRINGS[key];
  return (key, params) => {
    let template: string | undefined;
    if (params && typeof params['count'] === 'number') {
      // Plural selection: prefer the language's category, fall back to
      // '.other', then to the key itself (for non-plural keys that happen to
      // take a count param, e.g. "{count} entries").
      const category = rules.select(params['count']);
      template = resolve(`${key}.${category}`) ?? resolve(`${key}.other`) ?? resolve(key);
    } else {
      template = resolve(key);
    }
    // Key miss returns the key — visible in the UI, never a crash.
    return interpolate(template ?? key, params);
  };
};

// English-only translator: default context value, initial module accessor,
// and the permanent fallback when no provider is mounted (tests, storybooks).
const fallbackTranslate = buildTranslate('en', EMPTY_STRINGS);

// ---------------------------------------------------------------------------
// Module-level accessors for non-component code (moodUtils labels, utils).
// The provider registers its current translator/lang during render so code
// invoked from child renders sees the overlaid pack from the first paint.
// ---------------------------------------------------------------------------

let activeTranslate: TranslateFn = fallbackTranslate;
let activeLang = 'en';

/** Module-level t() for non-component code. */
export const translate: TranslateFn = (key, params) => activeTranslate(key, params);

/** Active language code, for date/number formatting outside components. */
export const getDateLocale = (): string => activeLang;

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

export interface I18nContextValue {
  t: TranslateFn;
  lang: string;
  setLang: (next: string) => void;
  /** Bundled English union whatever language setPack introduced. */
  availableLanguages: string[];
  /**
   * Install a server-fetched pack (later fetch-wiring phase calls this).
   * Invalid packs are rejected (returns false) and the last-good pack keeps
   * serving; valid packs persist to localStorage as the new last-good pack.
   */
  setPack: (pack: unknown) => boolean;
}

const I18nContext = createContext<I18nContextValue>({
  t: fallbackTranslate,
  lang: 'en',
  setLang: () => {},
  availableLanguages: ['en'],
  setPack: () => false,
});

export const I18nProvider = ({ children }: { children: ReactNode }) => {
  // Both initializers read localStorage synchronously so the last-good pack
  // and language choice apply on the very first render (no English flash).
  const [pack, setPackState] = useState<LanguagePack | null>(readStoredPack);
  const [lang, setLangState] = useState<string>(readStoredLang);

  const setLang = useCallback((next: string) => {
    if (typeof next !== 'string' || next.length === 0) return;
    setLangState(next);
    try { localStorage.setItem(LANG_STORAGE_KEY, next); } catch { /* ignore */ }
  }, []);

  const setPack = useCallback((candidate: unknown): boolean => {
    const sanitized = sanitizePack(candidate);
    if (!sanitized) return false;
    setPackState(sanitized);
    try { localStorage.setItem(PACK_STORAGE_KEY, JSON.stringify(sanitized)); } catch { /* ignore */ }
    return true;
  }, []);

  // A pack only overlays the language it belongs to; any other active
  // language runs on the bundled catalog (English-only in 0.6.0).
  const packStrings = useMemo(
    () => (pack !== null && pack.language === lang ? pack.strings : EMPTY_STRINGS),
    [pack, lang],
  );

  const t = useMemo(() => buildTranslate(lang, packStrings), [lang, packStrings]);

  // Register for module-level translate()/getDateLocale(). Assigned during
  // render on purpose (idempotent): children render after this line, so
  // non-component code they call sees the current translator — an effect
  // would lag one paint behind and boot-render stale English over a stored
  // pack.
  activeTranslate = t;
  activeLang = lang;

  const availableLanguages = useMemo(() => {
    const languages = ['en'];
    if (pack !== null && !languages.includes(pack.language)) languages.push(pack.language);
    return languages;
  }, [pack]);

  const value = useMemo<I18nContextValue>(
    () => ({ t, lang, setLang, availableLanguages, setPack }),
    [t, lang, setLang, availableLanguages, setPack],
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
};

export const useI18n = (): I18nContextValue => useContext(I18nContext);
