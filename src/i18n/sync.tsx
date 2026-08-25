import { useEffect } from 'react';
import apiService from '../services/api';
import { useI18n } from './index';

/**
 * Background language-pack refresh (v0.6.0 hot-swappable language packs).
 *
 * `I18nProvider` (src/i18n/index.tsx, not owned by this module) already
 * applies the last-good pack from `localStorage` synchronously at boot —
 * there is no English flash and no network round-trip on the critical
 * render path. This component's only job is to keep that cached pack fresh
 * by re-fetching the active language's pack from the server:
 *   - once on mount (picks up a newer pack version since the last visit)
 *   - again whenever the active language changes (the user picked a
 *     different language in Settings, or a stored `nightlio:lang:v1`
 *     choice loaded on this boot for the first time)
 *
 * English never fetches a pack here: the bundled `src/i18n/en.json` is
 * already the complete, permanent English catalog (the one thing the app
 * must work on even fully offline), so there is nothing to overlay.
 *
 * Every failure path is silent (console.warn only), leaving whatever was
 * already active — the last-good stored pack, or bundled English — serving:
 *   - the fetch rejects (offline, DNS, CORS, ...)
 *   - the server responds with a non-2xx status (the /api/i18n routes are
 *     shipped, but a code with no cached pack 404s — nothing published yet
 *     for it, a cold cache, or I18N_OFFLINE; an expected path, not an
 *     exceptional one)
 *   - the response parses but fails envelope validation (`setPack` returns
 *     false for a malformed or empty-shaped candidate)
 *
 * Renders nothing. Mount once, near the top of the tree, inside
 * `I18nProvider` (see src/App.tsx) so `useI18n()` resolves to the real
 * context instead of the no-op default.
 */
const I18nSync = () => {
  const { lang, setPack } = useI18n();

  useEffect(() => {
    if (lang === 'en') return;
    let cancelled = false;

    apiService
      .getLanguagePack(lang)
      .then((pack) => {
        if (cancelled) return;
        if (!setPack(pack)) {
          console.warn(`i18n: server pack for "${lang}" failed validation; keeping the last-good pack`);
        }
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        console.warn(`i18n: could not refresh the language pack for "${lang}"`, error);
      });

    return () => {
      cancelled = true;
    };
  }, [lang, setPack]);

  return null;
};

export default I18nSync;
