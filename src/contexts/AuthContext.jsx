import { createContext, useContext, useState, useEffect, useCallback, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import apiService from '../services/api';
import { useConfig } from './ConfigContext';

const AuthContext = createContext();

export const useAuth = () => {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error('useAuth must be used within an AuthProvider');
  }
  return context;
};

// After the OIDC redirect flow, the backend hands the app JWT back in the
// URL fragment (e.g. /login#sso_token=...). Fragments never reach servers,
// so the token stays out of access logs; we store it and strip it from the
// address bar before anything else renders.
const consumeSsoToken = () => {
  if (typeof window === 'undefined') return null;
  const match = (window.location.hash || '').match(/[#&]sso_token=([^&]+)/);
  if (!match) return null;
  const token = decodeURIComponent(match[1]);
  localStorage.setItem('nightlio_token', token);
  window.history.replaceState(null, '', window.location.pathname + window.location.search);
  return token;
};

export const AuthProvider = ({ children }) => {
  const { config, loading: configLoading } = useConfig();
  const [user, setUser] = useState(null);
  const [loading, setLoading] = useState(true);
  const [token, setToken] = useState(
    () => consumeSsoToken() || localStorage.getItem('nightlio_token'),
  );
  const navigate = useNavigate();
  const loggingOutRef = useRef(false);

  // Logout must be awaited and must navigate. Clearing state before the
  // server call resolves used to race the cookie-restore effect below:
  // token flips to null, the effect runs restoreFromCookie while the
  // httpOnly cookie is still valid (POST /api/auth/logout in flight), and
  // the session silently comes back — logout appeared to do nothing. Order
  // now: flag the logout so the restore effect stands down, await the
  // server clearing the cookie, then clear local state, then replace-navigate
  // to /login so the back button cannot resurrect the dashboard shell.
  const logout = useCallback(async () => {
    loggingOutRef.current = true;
    try {
      await apiService.logout();
    } catch {
      // Failure just means the cookie outlives until its JWT expiry
      // (bounded); proceed with the local logout regardless.
    }
    localStorage.removeItem('nightlio_token');
    setToken(null);
    setUser(null);
    apiService.setAuthToken(null);
    navigate('/login', { replace: true });
    setTimeout(() => {
      loggingOutRef.current = false;
    }, 0);
  }, [navigate]);

  const applyLogin = useCallback((jwtToken, userData) => {
    localStorage.setItem('nightlio_token', jwtToken);
    setToken(jwtToken);
    setUser(userData);
    apiService.setAuthToken(jwtToken);
  }, []);

  // Credential-free single-user self-host login. The backend refuses this
  // path when OIDC is configured, so callers gate it on !config.enable_oidc.
  const localLogin = useCallback(async () => {
    try {
      setLoading(true);
      const response = await apiService.localLogin();
      const { token: jwtToken, user: userData } = response;
      if (jwtToken) {
        applyLogin(jwtToken, userData);
      }
      return { success: true };
    } catch {
      return { success: false, error: 'Local login failed' };
    } finally {
      setLoading(false);
    }
  }, [applyLogin]);

  const loginWithPassword = useCallback(async (username, password) => {
    try {
      setLoading(true);
      const response = await apiService.localLogin(username, password);
      const { token: jwtToken, user: userData } = response;
      if (!jwtToken) {
        return { success: false, error: 'Login failed. Please try again.' };
      }
      applyLogin(jwtToken, userData);
      return { success: true };
    } catch (error) {
      return { success: false, error: error.message || 'Login failed. Please try again.' };
    } finally {
      setLoading(false);
    }
  }, [applyLogin]);

  const verifyToken = useCallback(async () => {
    try {
      const userData = await apiService.verifyToken(token);
      setUser(userData.user);
      apiService.setAuthToken(token);
    } catch {
      // If verify fails, clear token; in single-user self-host mode
      // (no OIDC) immediately local-login again.
      logout();
      // Credential-free re-entry only exists when the server allows local
      // login at all (DISABLE_LOCAL_LOGIN exposes enable_local_login: false).
      if (!config.enable_oidc && config.enable_local_login !== false) {
        await localLogin();
        return;
      }
    } finally {
      setLoading(false);
    }
  }, [token, config.enable_oidc, config.enable_local_login, logout, localLogin]);

  // Cookie-only session check: no token in memory or localStorage, but a
  // previous login may still have left the httpOnly session cookie behind
  // (e.g. localStorage was cleared, or a login this session relied on the
  // cookie rather than localStorage). No Authorization header is sent --
  // the browser attaches the cookie automatically via credentials:
  // 'include'. Returns whether a session was restored.
  const restoreFromCookie = useCallback(async () => {
    try {
      const userData = await apiService.verifyToken();
      setUser(userData.user);
      return true;
    } catch {
      return false;
    }
  }, []);

  useEffect(() => {
    if (configLoading) return;
    if (token) {
      verifyToken();
      return undefined;
    }
    // Stand down while a logout is in progress — the token just flipped to
    // null because the user logged out, not because a cookie session needs
    // restoring (see logout above for the race this prevents).
    if (loggingOutRef.current) {
      setLoading(false);
      return undefined;
    }
    let cancelled = false;
    (async () => {
      const restored = await restoreFromCookie();
      if (cancelled) return;
      if (restored) {
        setLoading(false);
      } else if (!config.enable_oidc && config.enable_local_login !== false) {
        // In single-user self-host mode, auto-login to the local account on
        // first visit — the credential-free "enter" flow.
        await localLogin();
      } else {
        setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [token, configLoading, config.enable_oidc, config.enable_local_login, verifyToken, localLogin, restoreFromCookie]);

  const value = {
    user,
    loading,
    loginWithPassword,
    localLogin,
    logout,
    isAuthenticated: !!user,
  };

  return (
    <AuthContext.Provider value={value}>
      {children}
    </AuthContext.Provider>
  );
};
