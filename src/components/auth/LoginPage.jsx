import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Lock } from 'lucide-react';
import { useAuth } from '../../contexts/AuthContext';
import { useConfig } from '../../contexts/ConfigContext';
import apiService from '../../services/api';
import './LoginPage.css';

const LoadingSpinner = () => (
  <svg className="login-page__spinner" viewBox="0 0 24 24" aria-hidden="true">
    <circle className="login-page__spinner-circle" cx="12" cy="12" r="10" />
  </svg>
);

const SSO_ERROR_MESSAGES = {
  callback_failed: 'Single sign-on failed. Please try again.',
  auth_failed: 'Single sign-on could not complete. Please try again.',
};

const LoginPage = () => {
  const navigate = useNavigate();
  const { loginWithPassword, isAuthenticated } = useAuth();
  const { config, loading: configLoading } = useConfig();
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [message, setMessage] = useState('');

  const enableOidc = Boolean(config.enable_oidc);
  // Server-supplied value that lands in an <a href> — only ever render
  // absolute http(s) URLs, even if a misconfigured backend passes something
  // else (file:, javascript:, ...) through.
  const isSafeHttpUrl = (value) => {
    try {
      const { protocol } = new URL(value);
      return protocol === 'http:' || protocol === 'https:';
    } catch {
      return false;
    }
  };
  const signupUrl =
    enableOidc && typeof config.signup_url === 'string' && isSafeHttpUrl(config.signup_url)
      ? config.signup_url
      : null;

  useEffect(() => {
    if (typeof document === 'undefined') return undefined;

    const rootElement = document.getElementById('root');
    if (!rootElement) return undefined;

    const previousStyles = {
      background: rootElement.style.background,
      padding: rootElement.style.padding,
      margin: rootElement.style.margin,
      boxShadow: rootElement.style.boxShadow,
      border: rootElement.style.border,
      borderRadius: rootElement.style.borderRadius,
    };

    Object.assign(rootElement.style, {
      background: 'transparent',
      padding: '0',
      margin: '0',
      boxShadow: 'none',
      border: 'none',
      borderRadius: '0',
    });

    return () => {
      Object.assign(rootElement.style, previousStyles);
    };
  }, []);

  useEffect(() => {
    if (isAuthenticated) {
      navigate('/dashboard', { replace: true });
    }
  }, [isAuthenticated, navigate]);

  // A failed OIDC round-trip lands back here with an error flag in the URL
  // fragment (the success case is consumed by AuthContext before render).
  useEffect(() => {
    if (typeof window === 'undefined') return;
    const match = (window.location.hash || '').match(/[#&]sso_error=([^&]+)/);
    if (!match) return;
    setMessage(SSO_ERROR_MESSAGES[match[1]] || SSO_ERROR_MESSAGES.callback_failed);
    window.history.replaceState(null, '', window.location.pathname + window.location.search);
  }, []);

  const handleSubmit = useCallback(
    async (event) => {
      event.preventDefault();
      if (!username.trim() || !password) {
        setMessage('Enter your username and password.');
        return;
      }

      setIsLoading(true);
      setMessage('');
      const result = await loginWithPassword(username.trim(), password);
      if (result.success) {
        navigate('/dashboard', { replace: true });
      } else {
        setMessage(result.error || 'Login failed. Please try again.');
        setIsLoading(false);
      }
    },
    [username, password, loginWithPassword, navigate],
  );

  const handleSsoLogin = useCallback(() => {
    if (typeof window === 'undefined') return;
    window.location.assign(apiService.getOidcLoginUrl());
  }, []);

  const handleSelfHostContinue = useCallback(() => {
    if (typeof window === 'undefined') return;
    window.location.reload();
  }, []);

  // Never render a login mode before /api/config resolves: the ConfigContext
  // default (enable_oidc: false) would flash — or, if the user acts fast, fully
  // expose — the legacy password form on OIDC deployments. Hold a minimal
  // loading state until the config fetch settles; if it failed, ConfigContext
  // keeps the defaults and we fall back to legacy mode (warned in the console).
  if (configLoading) {
    return (
      <div className="login-page">
        <div className="login-page__card login-page__card--auth" aria-busy="true">
          <LoadingSpinner />
          <span className="login-page__sr-only">Loading sign-in options…</span>
        </div>
      </div>
    );
  }

  return (
    <div className="login-page">
      <div className="login-page__card login-page__card--auth">
        <div className="login-page__header">
          <h1 className="login-page__brand-title">
            <img src="/logo.png" alt="Nightlio logo" className="login-page__brand-logo" />
            Nightlio
          </h1>
          <p className="login-page__brand-subtitle">Your daily mood companion.</p>
        </div>

        <div className="login-page__body">
          {enableOidc ? (
            /* SSO-first mode: the identity provider owns credentials and
               registration, so no local form and no credential-free entry. */
            <>
              {message && <p className="login-page__message">{message}</p>}

              <button
                type="button"
                className="login-page__button"
                onClick={handleSsoLogin}
              >
                Sign in with SSO
              </button>

              {signupUrl && (
                <a className="login-page__signup-link" href={signupUrl}>
                  Create account
                </a>
              )}
            </>
          ) : (
            <>
              <p className="login-page__description">
                Sign in to continue tracking your mood journey.
              </p>

              {message && <p className="login-page__message">{message}</p>}

              <form onSubmit={handleSubmit} className="login-page__form">
                <input
                  type="text"
                  name="username"
                  autoComplete="username"
                  placeholder="Username"
                  aria-label="Username"
                  value={username}
                  onChange={(event) => setUsername(event.target.value)}
                  disabled={isLoading}
                  className="login-page__input"
                />
                <input
                  type="password"
                  name="password"
                  autoComplete="current-password"
                  placeholder="Password"
                  aria-label="Password"
                  value={password}
                  onChange={(event) => setPassword(event.target.value)}
                  disabled={isLoading}
                  className="login-page__input"
                />
                <button
                  type="submit"
                  className="login-page__button"
                  disabled={isLoading}
                >
                  {isLoading ? (
                    <>
                      <LoadingSpinner />
                      <span>Signing in…</span>
                    </>
                  ) : (
                    'Sign in'
                  )}
                </button>
              </form>

              <button
                type="button"
                className="login-page__button login-page__button--secondary"
                onClick={handleSelfHostContinue}
                disabled={isLoading}
              >
                Continue without account
              </button>
            </>
          )}

          <div className="login-page__footer">
            <Lock size={12} aria-hidden="true" />
            <span>
              {enableOidc
                ? 'Sign in with your identity provider.'
                : 'Self-hosted: your data never leaves your server.'}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
};

export default LoginPage;
