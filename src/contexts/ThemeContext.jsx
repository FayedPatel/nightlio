import { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react';
import apiService from '../services/api';

// The four configurable themes. "default" is the app's signature
// Dracula-purple dark look (what data-theme="dark" used to be); "dark" is
// the neutral near-black variant. Palettes live in src/index.css.
export const THEMES = [
  { id: 'default', label: 'Default' },
  { id: 'light', label: 'Light' },
  { id: 'dark', label: 'Dark' },
  { id: 'synthwave', label: 'Synthwave' },
];
const THEME_IDS = THEMES.map((t) => t.id);

// v2 key: under the old single-toggle system 'dark' meant the Dracula look,
// which is now called 'default' — migrate the stored value once.
const STORAGE_KEY = 'nightlio:theme:v2';
const LEGACY_STORAGE_KEY = 'nightlio:theme';

const readStoredTheme = () => {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (THEME_IDS.includes(stored)) return stored;
    const legacy = localStorage.getItem(LEGACY_STORAGE_KEY);
    if (legacy === 'light') return 'light';
    return 'default';
  } catch {
    return 'default';
  }
};

const ThemeContext = createContext({
  theme: 'default',
  setTheme: () => {},
  syncFromServer: () => {},
  cycle: () => {},
});

export const ThemeProvider = ({ children }) => {
  const [theme, setThemeState] = useState(readStoredTheme);

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
    try { localStorage.setItem(STORAGE_KEY, theme); } catch { /* ignore */ }
  }, [theme]);

  // User-initiated change: apply locally and persist to the account. The
  // PUT silently no-ops pre-auth (401) — the server copy syncs on the next
  // authenticated change or is overridden by syncFromServer after login.
  const setTheme = useCallback((next) => {
    if (!THEME_IDS.includes(next)) return;
    setThemeState(next);
    apiService.updateThemePreference(next).catch(() => {});
  }, []);

  // Server-initiated (login sync): apply without echoing a PUT back.
  const syncFromServer = useCallback((next) => {
    if (THEME_IDS.includes(next)) setThemeState(next);
  }, []);

  const value = useMemo(() => ({
    theme,
    setTheme,
    syncFromServer,
    cycle: () => {
      setThemeState((current) => {
        const next = THEME_IDS[(THEME_IDS.indexOf(current) + 1) % THEME_IDS.length];
        apiService.updateThemePreference(next).catch(() => {});
        return next;
      });
    },
  }), [theme, setTheme, syncFromServer]);

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
};

export const useTheme = () => useContext(ThemeContext);
