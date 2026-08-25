import { createContext, useContext, useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import api from '../services/api';
import type { AppConfig } from '../types/api';
import { useI18n } from '../i18n';

/**
 * The config consumers see: the two flags below are always present (they are
 * the pre-fetch defaults), the rest of AppConfig only after /api/config
 * resolves — so enable_local_login, signup_url, and version can be undefined
 * during the initial load or forever if the fetch fails. Consumers already
 * treat them with `!== false` / truthiness checks accordingly.
 */
export type PublicConfig = Pick<AppConfig, 'enable_oidc' | 'enable_mood_music'> &
  Partial<AppConfig>;

export interface ConfigContextValue {
  config: PublicConfig;
  loading: boolean;
  error: string | null;
}

const ConfigContext = createContext<ConfigContextValue | undefined>(undefined);

export const useConfig = (): ConfigContextValue => {
  const ctx = useContext(ConfigContext);
  if (!ctx) throw new Error('useConfig must be used within ConfigProvider');
  return ctx;
};

export const ConfigProvider = ({ children }: { children: ReactNode }) => {
  const { t } = useI18n();
  const [config, setConfig] = useState<PublicConfig>({
    enable_oidc: false,
    enable_mood_music: false,
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let isMounted = true;
    (async () => {
      try {
        const data = await api.getPublicConfig();
        if (isMounted) {
          setConfig((prev) => ({ ...prev, ...data }));
        }
      } catch (e) {
        if (isMounted) {
          // Fall back to the defaults above (OIDC off → legacy login mode).
          // Warn loudly so a silently unreachable /api/config is diagnosable.
          console.warn('Failed to load /api/config, falling back to defaults (OIDC disabled):', e);
          setError(e instanceof Error && e.message ? e.message : t('errors.loadConfig'));
        }
      } finally {
        if (isMounted) setLoading(false);
      }
    })();
    return () => { isMounted = false; };
  }, []);

  return (
    <ConfigContext.Provider value={{ config, loading, error }}>
      {children}
    </ConfigContext.Provider>
  );
};
