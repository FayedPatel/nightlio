import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import apiService from '../services/api';
import type { ThemeName } from '../types/api';

// The four configurable themes. "default" is the app's signature
// Dracula-purple dark look (what data-theme="dark" used to be); "dark" is
// the neutral near-black variant. Palettes live in src/index.css.
export interface ThemeOption {
  id: ThemeName;
  label: string;
}

export const THEMES: ThemeOption[] = [
  { id: 'default', label: 'Default' },
  { id: 'light', label: 'Light' },
  { id: 'dark', label: 'Dark' },
  { id: 'synthwave', label: 'Synthwave' },
];
const THEME_IDS = THEMES.map((t) => t.id);

// Narrowing guard for the untrusted sources (localStorage, the server's
// free-form Preferences.theme string).
const isThemeId = (value: string | null | undefined): value is ThemeName =>
  THEME_IDS.some((id) => id === value);

// v2 key: under the old single-toggle system 'dark' meant the Dracula look,
// which is now called 'default' — migrate the stored value once.
const STORAGE_KEY = 'nightlio:theme:v2';
const LEGACY_STORAGE_KEY = 'nightlio:theme';

const readStoredTheme = (): ThemeName => {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (isThemeId(stored)) return stored;
    const legacy = localStorage.getItem(LEGACY_STORAGE_KEY);
    if (legacy === 'light') return 'light';
    return 'default';
  } catch {
    return 'default';
  }
};

export interface ThemeContextValue {
  theme: ThemeName;
  setTheme: (next: ThemeName) => void;
  /** Login sync accepts the server's loosely-typed Preferences.theme. */
  syncFromServer: (next: string | null | undefined) => void;
  cycle: () => void;
}

const ThemeContext = createContext<ThemeContextValue>({
  theme: 'default',
  setTheme: () => {},
  syncFromServer: () => {},
  cycle: () => {},
});

export const ThemeProvider = ({ children }: { children: ReactNode }) => {
  const [theme, setThemeState] = useState<ThemeName>(readStoredTheme);
  // Once the user picks a theme in this session, the in-flight login sync
  // must not overwrite it: the GET /api/preferences response can land AFTER
  // a toggle click and silently revert the user's choice.
  const userChangedRef = useRef(false);

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
    try { localStorage.setItem(STORAGE_KEY, theme); } catch { /* ignore */ }
  }, [theme]);

  // User-initiated change: apply locally and persist to the account. The
  // PUT silently no-ops pre-auth (401) — the server copy syncs on the next
  // authenticated change or is overridden by syncFromServer after login.
  const setTheme = useCallback((next: ThemeName) => {
    if (!isThemeId(next)) return;
    userChangedRef.current = true;
    setThemeState(next);
    apiService.updateThemePreference(next).catch(() => {});
  }, []);

  // Server-initiated (login sync): apply without echoing a PUT back, and
  // never over a choice the user already made this session.
  const syncFromServer = useCallback((next: string | null | undefined) => {
    if (userChangedRef.current) return;
    if (isThemeId(next)) setThemeState(next);
  }, []);

  const value = useMemo<ThemeContextValue>(() => ({
    theme,
    setTheme,
    syncFromServer,
    cycle: () => {
      userChangedRef.current = true;
      setThemeState((current) => {
        const next = THEME_IDS[(THEME_IDS.indexOf(current) + 1) % THEME_IDS.length] ?? current;
        apiService.updateThemePreference(next).catch(() => {});
        return next;
      });
    },
  }), [theme, setTheme, syncFromServer]);

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
};

export const useTheme = (): ThemeContextValue => useContext(ThemeContext);
