import { useCallback, useEffect, useState } from 'react';
import { useConfig } from '../contexts/ConfigContext';
import { useTheme, THEMES } from '../contexts/ThemeContext';
import apiService from '../services/api';

const ACTIVITY_PAGE_SIZE = 20;

const humanize = (value) =>
  String(value || '')
    .replace(/_/g, ' ')
    .replace(/^./, (c) => c.toUpperCase());

const describeActivity = (activity) => {
  const meta = activity.metadata || {};
  switch (activity.event_type) {
    case 'login':
      if (meta.method === 'oidc') return 'Signed in with SSO';
      if (meta.method === 'local') return 'Signed in with username and password';
      if (meta.method === 'selfhost') return 'Signed in (self-host)';
      return 'Signed in';
    case 'entry_created':
      return meta.date ? `Created a journal entry for ${meta.date}` : 'Created a journal entry';
    case 'entry_edited':
      return 'Edited a journal entry';
    case 'entry_deleted':
      return 'Deleted a journal entry';
    case 'achievement_unlocked':
      return meta.achievement_type
        ? `Unlocked achievement: ${humanize(meta.achievement_type)}`
        : 'Unlocked an achievement';
    case 'goal_completed':
      return meta.title ? `Completed goal “${meta.title}”` : 'Completed a goal';
    default:
      return humanize(activity.event_type) || 'Activity';
  }
};

const formatTimestamp = (value) => {
  if (!value) return '';
  // SQLite stores UTC "YYYY-MM-DD HH:MM:SS"; normalize to ISO so the
  // browser renders it in local time.
  const iso = value.includes('T') ? value : `${value.replace(' ', 'T')}Z`;
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
};

const SettingsView = () => {
  const { config, loading } = useConfig();
  const { theme, setTheme } = useTheme();
  const [activities, setActivities] = useState([]);
  const [nextCursor, setNextCursor] = useState(null);
  const [activityLoading, setActivityLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [activityError, setActivityError] = useState(null);

  const loadActivity = useCallback(async (before) => {
    try {
      if (before != null) setLoadingMore(true);
      const data = await apiService.getActivity(before, ACTIVITY_PAGE_SIZE);
      setActivities((prev) =>
        before != null ? [...prev, ...(data.activities || [])] : data.activities || [],
      );
      setNextCursor(data.next_cursor ?? null);
      setActivityError(null);
    } catch {
      setActivityError('Failed to load activity.');
    } finally {
      setActivityLoading(false);
      setLoadingMore(false);
    }
  }, []);

  useEffect(() => {
    loadActivity();
  }, [loadActivity]);

  const featureFlags = [
    {
      key: 'enable_oidc',
      label: 'Single Sign-On (OIDC)',
      description: 'Enable login through a configured OpenID Connect provider.',
    },
    {
      key: 'enable_mood_music',
      label: 'Mood Music',
      description: 'Play mood-based music suggestions from the mood picker.',
    },
  ];

  return (
    <div style={{ textAlign: 'left' }}>
      <h2 style={{ marginTop: 0, color: 'var(--text)' }}>Settings</h2>

      <section
        style={{
          marginTop: '1rem',
          border: '1px solid var(--border)',
          borderRadius: '12px',
          padding: '1rem',
          background: 'var(--surface)',
        }}
        aria-label="Appearance"
      >
        <h3 style={{ marginTop: 0, marginBottom: '0.5rem', color: 'var(--text)' }}>Appearance</h3>
        <p style={{ marginTop: 0, marginBottom: '0.75rem', color: 'var(--text-muted)', fontSize: '0.9rem' }}>
          Pick a theme. Your choice is saved to your account and follows you
          across devices.
        </p>
        <div
          role="radiogroup"
          aria-label="Theme"
          style={{ display: 'flex', flexWrap: 'wrap', gap: '0.5rem' }}
        >
          {THEMES.map((option) => (
            <button
              key={option.id}
              type="button"
              role="radio"
              aria-checked={theme === option.id}
              onClick={() => setTheme(option.id)}
              className={`theme-option${theme === option.id ? ' is-active' : ''}`}
            >
              <span className={`theme-option__swatch theme-option__swatch--${option.id}`} aria-hidden="true" />
              {option.label}
            </button>
          ))}
        </div>
      </section>

      <section
        style={{
          marginTop: '1rem',
          border: '1px solid var(--border)',
          borderRadius: '12px',
          padding: '1rem',
          background: 'var(--surface)',
        }}
        aria-label="Feature flags"
      >
        <h3 style={{ marginTop: 0, marginBottom: '0.5rem', color: 'var(--text)' }}>Feature flags</h3>
        <p style={{ marginTop: 0, marginBottom: '0.75rem', color: 'var(--text-muted)', fontSize: '0.9rem' }}>
          These are currently server-managed. Editable toggles can be added here later.
        </p>

        {featureFlags.map((flag) => {
          const isEnabled = Boolean(config[flag.key]);
          return (
            <label
              key={flag.key}
              style={{
                display: 'flex',
                alignItems: 'flex-start',
                gap: '0.75rem',
                padding: '0.5rem 0',
                borderTop: '1px solid var(--border)',
              }}
            >
              <input
                type="checkbox"
                checked={isEnabled}
                readOnly
                disabled
                aria-label={flag.label}
                style={{ marginTop: '0.15rem' }}
              />
              <span>
                <strong style={{ color: 'var(--text)' }}>{flag.label}</strong>
                <span style={{ display: 'block', color: 'var(--text-muted)', fontSize: '0.86rem' }}>
                  {flag.description}
                  {loading ? ' (loading...)' : isEnabled ? ' (enabled)' : ' (disabled)'}
                </span>
              </span>
            </label>
          );
        })}
      </section>

      <section
        style={{
          marginTop: '1rem',
          border: '1px solid var(--border)',
          borderRadius: '12px',
          padding: '1rem',
          background: 'var(--surface)',
        }}
        aria-label="Recent activity"
      >
        <h3 style={{ marginTop: 0, marginBottom: '0.5rem', color: 'var(--text)' }}>Recent activity</h3>
        <p style={{ marginTop: 0, marginBottom: '0.75rem', color: 'var(--text-muted)', fontSize: '0.9rem' }}>
          A log of recent logins, entries, goals, and achievements on this account.
        </p>

        {activityLoading ? (
          <p style={{ margin: 0, color: 'var(--text-muted)', fontSize: '0.9rem' }}>Loading activity...</p>
        ) : activityError ? (
          <p style={{ margin: 0, color: 'var(--danger)', fontSize: '0.9rem' }}>{activityError}</p>
        ) : activities.length === 0 ? (
          <p style={{ margin: 0, color: 'var(--text-muted)', fontSize: '0.9rem' }}>
            No activity yet. Logins, journal entries, goals, and achievements will show up here.
          </p>
        ) : (
          <>
            <ul style={{ listStyle: 'none', margin: 0, padding: 0 }}>
              {activities.map((activity) => (
                <li
                  key={activity.id}
                  style={{
                    display: 'flex',
                    flexWrap: 'wrap',
                    justifyContent: 'space-between',
                    gap: '0.25rem 0.75rem',
                    padding: '0.5rem 0',
                    borderTop: '1px solid var(--border)',
                  }}
                >
                  <span style={{ color: 'var(--text)', fontSize: '0.9rem' }}>
                    {describeActivity(activity)}
                  </span>
                  <span style={{ color: 'var(--text-muted)', fontSize: '0.82rem', whiteSpace: 'nowrap' }}>
                    {formatTimestamp(activity.created_at)}
                  </span>
                </li>
              ))}
            </ul>
            {nextCursor != null && (
              <button
                type="button"
                onClick={() => loadActivity(nextCursor)}
                disabled={loadingMore}
                style={{
                  marginTop: '0.75rem',
                  padding: '0.5rem 1rem',
                  borderRadius: '8px',
                  border: '1px solid var(--border)',
                  background: 'var(--surface)',
                  color: 'var(--text)',
                  fontSize: '0.9rem',
                  cursor: loadingMore ? 'not-allowed' : 'pointer',
                }}
              >
                {loadingMore ? 'Loading...' : 'Load more'}
              </button>
            )}
          </>
        )}
      </section>
    </div>
  );
};

export default SettingsView;
