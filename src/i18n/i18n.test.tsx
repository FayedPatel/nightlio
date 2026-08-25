import { describe, expect, it, beforeEach } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { I18nProvider, useI18n, translate, getDateLocale, sanitizePack } from './index';
import type { I18nKey } from './index';
import en from './en.json';

const PACK_STORAGE_KEY = 'nightlio:i18n:pack:v1';
const LANG_STORAGE_KEY = 'nightlio:lang:v1';

const wrapper = ({ children }: { children: ReactNode }) => (
  <I18nProvider>{children}</I18nProvider>
);

const renderI18n = () => renderHook(() => useI18n(), { wrapper });

const validPack = (strings: Record<string, unknown>, language = 'en') => ({
  schema_version: 1,
  language,
  strings,
});

beforeEach(() => {
  localStorage.clear();
});

describe('t() basics', () => {
  it('returns the bundled English string for a known key', () => {
    const { result } = renderI18n();
    expect(result.current.t('nav.home')).toBe('Home');
    expect(result.current.lang).toBe('en');
    expect(result.current.availableLanguages).toEqual(['en']);
  });

  it('returns the key itself on a key miss', () => {
    const { result } = renderI18n();
    expect(result.current.t('does.not.exist' as I18nKey)).toBe('does.not.exist');
  });

  it('interpolates {name}-style params', () => {
    const { result } = renderI18n();
    expect(
      result.current.t('settings.activity.entryCreatedFor', { date: '2026-08-01' }),
    ).toBe('Created a journal entry for 2026-08-01');
    expect(result.current.t('common.dateAtTime', { date: 'Aug 1', time: '9:30 PM' })).toBe(
      'Aug 1 at 9:30 PM',
    );
  });

  it('leaves unknown placeholders verbatim instead of eating text', () => {
    const { result } = renderI18n();
    expect(result.current.t('settings.activity.entryCreatedFor', { nope: 'x' })).toBe(
      'Created a journal entry for {date}',
    );
  });
});

describe('plural selection', () => {
  it('selects .one and .other via Intl.PluralRules', () => {
    const { result } = renderI18n();
    expect(result.current.t('goals.frequencyPerWeek', { count: 1 })).toBe('1 day a week');
    expect(result.current.t('goals.frequencyPerWeek', { count: 3 })).toBe('3 days a week');
    expect(result.current.t('goals.frequencyPerWeek', { count: 0 })).toBe('0 days a week');
  });

  it('falls back to the plain key when no plural variants exist', () => {
    // "stats.entriesTooltip" is a non-plural key that takes a count param.
    const { result } = renderI18n();
    expect(result.current.t('stats.entriesTooltip', { count: 1 })).toBe('1 entries');
  });
});

describe('pack overlay', () => {
  it('overlays pack strings and falls back to bundled en for missing keys', () => {
    const { result } = renderI18n();
    let accepted = false;
    act(() => {
      accepted = result.current.setPack(validPack({ 'nav.home': 'Casa' }));
    });
    expect(accepted).toBe(true);
    expect(result.current.t('nav.home')).toBe('Casa');
    // Partial pack: everything it does not cover stays bundled English.
    expect(result.current.t('nav.settings')).toBe('Settings');
  });

  it('only overlays the language the pack belongs to', () => {
    const { result } = renderI18n();
    act(() => {
      result.current.setPack(validPack({ 'nav.home': 'Maison' }, 'fr'));
    });
    // Active language is still 'en' — the fr pack must not leak in.
    expect(result.current.t('nav.home')).toBe('Home');
    expect(result.current.availableLanguages).toEqual(['en', 'fr']);
    act(() => {
      result.current.setLang('fr');
    });
    expect(result.current.lang).toBe('fr');
    expect(result.current.t('nav.home')).toBe('Maison');
  });

  it('rejects packs with the wrong shape', () => {
    const { result } = renderI18n();
    const badPacks: unknown[] = [
      null,
      undefined,
      'strings',
      42,
      [],
      {},
      { schema_version: 2, language: 'en', strings: {} },
      { schema_version: 1, language: '', strings: {} },
      { schema_version: 1, language: 'en', strings: 'nope' },
      { schema_version: 1, language: 'en', strings: null },
      { schema_version: 1, strings: {} },
    ];
    for (const bad of badPacks) {
      let accepted = true;
      act(() => {
        accepted = result.current.setPack(bad);
      });
      expect(accepted).toBe(false);
    }
    // Nothing was installed or persisted.
    expect(result.current.t('nav.home')).toBe('Home');
    expect(localStorage.getItem(PACK_STORAGE_KEY)).toBeNull();
  });

  it('drops non-string values from an otherwise valid pack', () => {
    const { result } = renderI18n();
    let accepted = false;
    act(() => {
      accepted = result.current.setPack(
        validPack({ 'nav.home': 42, 'nav.goals': 'Metas', 'nav.history': null }),
      );
    });
    expect(accepted).toBe(true);
    expect(result.current.t('nav.goals')).toBe('Metas');
    // Dropped entries fall back to bundled English.
    expect(result.current.t('nav.home')).toBe('Home');
    expect(result.current.t('nav.history')).toBe('History');
  });

  it('a bad pack never displaces the last-good pack', () => {
    const { result } = renderI18n();
    act(() => {
      result.current.setPack(validPack({ 'nav.home': 'Casa' }));
    });
    act(() => {
      result.current.setPack({ schema_version: 99 });
    });
    expect(result.current.t('nav.home')).toBe('Casa');
  });
});

describe('localStorage persistence', () => {
  it('round-trips the last-good pack across a remount', () => {
    const first = renderI18n();
    act(() => {
      first.result.current.setPack(validPack({ 'nav.home': 'Casa' }));
    });
    first.unmount();

    const stored = localStorage.getItem(PACK_STORAGE_KEY);
    expect(stored).not.toBeNull();
    expect(JSON.parse(stored as string)).toEqual(validPack({ 'nav.home': 'Casa' }));

    // Fresh provider: the stored pack overlays synchronously at boot.
    const second = renderI18n();
    expect(second.result.current.t('nav.home')).toBe('Casa');
  });

  it('round-trips the language choice across a remount', () => {
    const first = renderI18n();
    act(() => {
      first.result.current.setLang('fr');
    });
    first.unmount();
    expect(localStorage.getItem(LANG_STORAGE_KEY)).toBe('fr');

    const second = renderI18n();
    expect(second.result.current.lang).toBe('fr');
    // No fr pack installed: bundled English keeps serving.
    expect(second.result.current.t('nav.home')).toBe('Home');
  });

  it('recovers from corrupted stored data', () => {
    localStorage.setItem(PACK_STORAGE_KEY, '{not json');
    localStorage.setItem(LANG_STORAGE_KEY, '');
    const { result } = renderI18n();
    expect(result.current.lang).toBe('en');
    expect(result.current.t('nav.home')).toBe('Home');
    expect(result.current.availableLanguages).toEqual(['en']);
  });

  it('recovers from stored data that parses but fails validation', () => {
    localStorage.setItem(
      PACK_STORAGE_KEY,
      JSON.stringify({ schema_version: 2, language: 'en', strings: { 'nav.home': 'Casa' } }),
    );
    const { result } = renderI18n();
    expect(result.current.t('nav.home')).toBe('Home');
  });
});

describe('module-level accessors', () => {
  it('translate() and getDateLocale() track the mounted provider', () => {
    const { result } = renderI18n();
    expect(translate('nav.home')).toBe('Home');
    expect(getDateLocale()).toBe('en');
    act(() => {
      result.current.setPack(validPack({ 'nav.home': 'Casa' }));
    });
    expect(translate('nav.home')).toBe('Casa');
    act(() => {
      result.current.setLang('de');
    });
    expect(getDateLocale()).toBe('de');
  });

  it('translate() interpolates and pluralizes like the hook t()', () => {
    renderI18n();
    expect(translate('goals.frequencyPerWeek', { count: 1 })).toBe('1 day a week');
    expect(translate('goals.frequencyPerWeek', { count: 5 })).toBe('5 days a week');
  });
});

describe('sanitizePack', () => {
  it('returns a sanitized copy for a valid envelope (flat input accepted)', () => {
    const sanitized = sanitizePack(validPack({ 'a.b': 'c', 'd.e': 7 }));
    expect(sanitized).toEqual({ schema_version: 1, language: 'en', strings: { 'a.b': 'c' } });
  });

  it('flattens the nested wire format to dot-keys, dropping invalid leaves', () => {
    const sanitized = sanitizePack(
      validPack({ nav: { home: 'Inicio', history: 'Historial' }, bad: { leaf: 7, arr: [] }, top: 'Hola' }),
    );
    expect(sanitized).toEqual({
      schema_version: 1,
      language: 'en',
      strings: { 'nav.home': 'Inicio', 'nav.history': 'Historial', top: 'Hola' },
    });
  });

  it('returns null for invalid envelopes', () => {
    expect(sanitizePack(null)).toBeNull();
    expect(sanitizePack([])).toBeNull();
    expect(sanitizePack({ schema_version: '1', language: 'en', strings: {} })).toBeNull();
  });
});

describe('catalog integrity', () => {
  // The source catalog is nested; lookups use dot-paths. Flatten the same way
  // the runtime does before asserting.
  const flatten = (node: unknown, prefix = '', out: Record<string, string> = {}) => {
    if (typeof node !== 'object' || node === null) return out;
    for (const [key, value] of Object.entries(node)) {
      const path = prefix ? `${prefix}.${key}` : key;
      if (typeof value === 'string') out[path] = value;
      else flatten(value, path, out);
    }
    return out;
  };
  const flat = flatten(en);

  it('every leaf is a non-empty string and every branch is a plain object', () => {
    const walk = (node: unknown, path: string) => {
      expect(typeof node === 'string' || (typeof node === 'object' && node !== null && !Array.isArray(node)), `node at ${path}`).toBe(true);
      if (typeof node === 'string') {
        expect(node.length, `length of ${path}`).toBeGreaterThan(0);
        return;
      }
      for (const [key, value] of Object.entries(node as Record<string, unknown>)) {
        expect(key.includes('.'), `key "${key}" at ${path} must not contain dots (nesting expresses the path)`).toBe(false);
        walk(value, path ? `${path}.${key}` : key);
      }
    };
    walk(en, '');
    expect(Object.keys(flat).length).toBeGreaterThan(400);
  });

  it('every plural .one key has a matching .other twin', () => {
    const keys = new Set(Object.keys(flat));
    for (const key of keys) {
      if (key.endsWith('.one')) {
        expect(keys.has(`${key.slice(0, -4)}.other`), `${key} needs .other`).toBe(true);
      }
      if (key.endsWith('.other')) {
        expect(keys.has(`${key.slice(0, -6)}.one`), `${key} needs .one`).toBe(true);
      }
    }
  });
});
