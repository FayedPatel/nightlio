import { createContext, useContext, useEffect, useState } from 'react';
import api from '../services/api';

const ConfigContext = createContext();

export const useConfig = () => {
  const ctx = useContext(ConfigContext);
  if (!ctx) throw new Error('useConfig must be used within ConfigProvider');
  return ctx;
};

export const ConfigProvider = ({ children }) => {
  const [config, setConfig] = useState({
    enable_oidc: false,
    enable_mood_music: false,
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

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
          setError(e.message || 'Failed to load config');
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
