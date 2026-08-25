import { useCallback, useEffect, useRef, useState } from 'react';
import type { ChangeEvent } from 'react';
import { useConfig } from '../contexts/ConfigContext';
import type { PublicConfig } from '../contexts/ConfigContext';
import { useTheme, THEMES } from '../contexts/ThemeContext';
import apiService from '../services/api';
import type { ActivityEvent, DataExport, DataImportResult, LanguageInfo } from '../types/api';
import { exportJSONToFile } from '../utils/exportUtils';
import { useI18n } from '../i18n';
import type { TranslateFn } from '../i18n';

// The bundled fallback catalog's own language, always first in the picker.
// "English" here is the language's own autonym (its native_name), not UI
// chrome — like every other language's native_name on the wire, it is
// never itself translated, so it is legitimate data, not a t()-routed
// string.
const BUNDLED_ENGLISH: LanguageInfo = { code: 'en', name: 'English', native_name: 'English', version: 'bundled' };

const ACTIVITY_PAGE_SIZE = 20;

interface FeatureFlag {
  key: keyof PublicConfig;
  label: string;
  description: string;
}

const humanize = (value: unknown): string =>
  String(value || '')
    .replace(/_/g, ' ')
    .replace(/^./, (c) => c.toUpperCase());

const describeActivity = (activity: ActivityEvent, t: TranslateFn): string => {
  const meta = activity.metadata || {};
  switch (activity.event_type) {
    case 'login':
      if (meta.method === 'oidc') return t('settings.activity.signedInSso');
      if (meta.method === 'local') return t('settings.activity.signedInLocal');
      if (meta.method === 'selfhost') return t('settings.activity.signedInSelfhost');
      return t('settings.activity.signedIn');
    case 'entry_created':
      return meta.date
        ? t('settings.activity.entryCreatedFor', { date: String(meta.date) })
        : t('settings.activity.entryCreated');
    case 'entry_edited':
      return t('settings.activity.entryEdited');
    case 'entry_deleted':
      return t('settings.activity.entryDeleted');
    case 'achievement_unlocked':
      return meta.achievement_type
        ? t('settings.activity.achievementUnlockedNamed', { name: humanize(meta.achievement_type) })
        : t('settings.activity.achievementUnlocked');
    case 'goal_completed':
      return meta.title
        ? t('settings.activity.goalCompletedNamed', { title: String(meta.title) })
        : t('settings.activity.goalCompleted');
    default:
      return humanize(activity.event_type) || t('settings.activity.fallback');
  }
};

const formatTimestamp = (value: string): string => {
  if (!value) return '';
  // SQLite stores UTC "YYYY-MM-DD HH:MM:SS"; normalize to ISO so the
  // browser renders it in local time.
  const iso = value.includes('T') ? value : `${value.replace(' ', 'T')}Z`;
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
};

const SettingsView = () => {
  const { t, lang, setLang } = useI18n();
  const { config, loading } = useConfig();
  const { theme, setTheme } = useTheme();
  const [activities, setActivities] = useState<ActivityEvent[]>([]);
  const [nextCursor, setNextCursor] = useState<number | null>(null);
  const [activityLoading, setActivityLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [activityError, setActivityError] = useState<string | null>(null);
  const [serverLanguages, setServerLanguages] = useState<LanguageInfo[]>([]);
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [dataError, setDataError] = useState<string | null>(null);
  const [importResult, setImportResult] = useState<DataImportResult | null>(null);
  const importInputRef = useRef<HTMLInputElement>(null);

  // Server list is best-effort and additive only: a failure (offline, the
  // API down) or an empty list (the shipped /api/i18n routes' no-packs
  // degrade path -- nothing published or cached yet, or I18N_OFFLINE) just
  // leaves the picker at bundled English, exactly like the runtime's own
  // pack fetch degrades (src/i18n/sync.tsx).
  useEffect(() => {
    let cancelled = false;
    apiService
      .getLanguages()
      .then((data) => {
        if (!cancelled) setServerLanguages(data.languages || []);
      })
      .catch(() => {
        if (!cancelled) setServerLanguages([]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Bundled English union the server list, deduped by code (the contract
  // guarantees the server never lists "en", but a stale/misbehaving pack
  // cache is untrusted input from this component's point of view too).
  const languageOptions: LanguageInfo[] = [
    BUNDLED_ENGLISH,
    ...serverLanguages.filter((entry) => entry.code !== BUNDLED_ENGLISH.code),
  ];

  const loadActivity = useCallback(async (before?: number | null) => {
    try {
      if (before != null) setLoadingMore(true);
      const data = await apiService.getActivity(before, ACTIVITY_PAGE_SIZE);
      setActivities((prev) =>
        before != null ? [...prev, ...(data.activities || [])] : data.activities || [],
      );
      setNextCursor(data.next_cursor ?? null);
      setActivityError(null);
    } catch {
      setActivityError(t('errors.loadActivity'));
    } finally {
      setActivityLoading(false);
      setLoadingMore(false);
    }
  }, [t]);

  useEffect(() => {
    loadActivity();
  }, [loadActivity]);

  const handleExport = async () => {
    setDataError(null);
    setImportResult(null);
    setExporting(true);
    try {
      const data = await apiService.exportData();
      exportJSONToFile(data, `nightlio-export-${new Date().toISOString().slice(0, 10)}.json`);
    } catch {
      setDataError(t('settings.data.exportError'));
    } finally {
      setExporting(false);
    }
  };

  const handleImportFile = async (event: ChangeEvent<HTMLInputElement>) => {
    const input = event.target;
    const file = input.files?.[0];
    if (!file) return;
    setDataError(null);
    setImportResult(null);
    setImporting(true);
    try {
      const text = await file.text();
      let parsed: unknown;
      try {
        parsed = JSON.parse(text);
      } catch {
        setDataError(t('settings.data.importInvalidFile'));
        return;
      }
      // Client pre-validation: anything that is not an object carrying a
      // numeric schema_version cannot be a Nightlio export — reject locally
      // without an API round-trip. Everything deeper (version support, row
      // validation) is the server's call.
      if (
        typeof parsed !== 'object' ||
        parsed === null ||
        Array.isArray(parsed) ||
        typeof (parsed as { schema_version?: unknown }).schema_version !== 'number'
      ) {
        setDataError(t('settings.data.importInvalidFile'));
        return;
      }
      const result = await apiService.importData(parsed as DataExport);
      setImportResult(result);
    } catch {
      setDataError(t('settings.data.importError'));
    } finally {
      setImporting(false);
      // Clear the input so re-selecting the same file fires change again
      // (e.g. retry after a failed import of the identical file).
      input.value = '';
    }
  };

  const featureFlags: FeatureFlag[] = [
    {
      key: 'enable_oidc',
      label: t('settings.flags.oidc.label'),
      description: t('settings.flags.oidc.description'),
    },
    {
      key: 'enable_mood_music',
      label: t('settings.flags.moodMusic.label'),
      description: t('settings.flags.moodMusic.description'),
    },
  ];

  return (
    <div style={{ textAlign: 'left' }}>
      <h2 style={{ marginTop: 0, color: 'var(--text)' }}>{t('settings.title')}</h2>

      <section
        style={{
          marginTop: '1rem',
          border: '1px solid var(--border)',
          borderRadius: '12px',
          padding: '1rem',
          background: 'var(--surface)',
        }}
        aria-label={t('settings.appearance.title')}
      >
        <h3 style={{ marginTop: 0, marginBottom: '0.5rem', color: 'var(--text)' }}>{t('settings.appearance.title')}</h3>
        <p style={{ marginTop: 0, marginBottom: '0.75rem', color: 'var(--text-muted)', fontSize: '0.9rem' }}>
          {t('settings.appearance.description')}
        </p>
        <div
          role="radiogroup"
          aria-label={t('settings.appearance.themeAria')}
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
        aria-label={t('settings.language.title')}
      >
        <h3 style={{ marginTop: 0, marginBottom: '0.5rem', color: 'var(--text)' }}>{t('settings.language.title')}</h3>
        <p style={{ marginTop: 0, marginBottom: '0.75rem', color: 'var(--text-muted)', fontSize: '0.9rem' }}>
          {t('settings.language.description')}
        </p>
        <div
          role="radiogroup"
          aria-label={t('settings.language.title')}
          style={{ display: 'flex', flexWrap: 'wrap', gap: '0.5rem' }}
        >
          {languageOptions.map((option) => (
            <button
              key={option.code}
              type="button"
              role="radio"
              aria-checked={lang === option.code}
              onClick={() => setLang(option.code)}
              className={`theme-option${lang === option.code ? ' is-active' : ''}`}
            >
              {option.native_name}
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
        aria-label={t('settings.flags.title')}
      >
        <h3 style={{ marginTop: 0, marginBottom: '0.5rem', color: 'var(--text)' }}>{t('settings.flags.title')}</h3>
        <p style={{ marginTop: 0, marginBottom: '0.75rem', color: 'var(--text-muted)', fontSize: '0.9rem' }}>
          {t('settings.flags.description')}
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
                  {loading
                    ? t('settings.flags.stateLoading')
                    : isEnabled
                      ? t('settings.flags.stateEnabled')
                      : t('settings.flags.stateDisabled')}
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
        aria-label={t('settings.data.title')}
      >
        <h3 style={{ marginTop: 0, marginBottom: '0.5rem', color: 'var(--text)' }}>{t('settings.data.title')}</h3>
        <p style={{ marginTop: 0, marginBottom: '0.75rem', color: 'var(--text-muted)', fontSize: '0.9rem' }}>
          {t('settings.data.description')}
        </p>
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: '0.5rem' }}>
          <button
            type="button"
            onClick={handleExport}
            disabled={exporting || importing}
            style={{
              padding: '0.5rem 1rem',
              borderRadius: '8px',
              border: '1px solid var(--border)',
              background: 'var(--surface)',
              color: 'var(--text)',
              fontSize: '0.9rem',
              cursor: exporting || importing ? 'not-allowed' : 'pointer',
            }}
          >
            {exporting ? t('settings.data.exporting') : t('settings.data.exportButton')}
          </button>
          <button
            type="button"
            onClick={() => importInputRef.current?.click()}
            disabled={exporting || importing}
            style={{
              padding: '0.5rem 1rem',
              borderRadius: '8px',
              border: '1px solid var(--border)',
              background: 'var(--surface)',
              color: 'var(--text)',
              fontSize: '0.9rem',
              cursor: exporting || importing ? 'not-allowed' : 'pointer',
            }}
          >
            {importing ? t('settings.data.importing') : t('settings.data.importButton')}
          </button>
          <input
            ref={importInputRef}
            type="file"
            accept=".json,application/json"
            onChange={handleImportFile}
            style={{ display: 'none' }}
          />
        </div>
        {dataError && (
          <p style={{ marginTop: '0.75rem', marginBottom: 0, color: 'var(--danger)', fontSize: '0.9rem' }}>
            {dataError}
          </p>
        )}
        {importResult && (
          <p style={{ marginTop: '0.75rem', marginBottom: 0, color: 'var(--text)', fontSize: '0.9rem' }}>
            {t('settings.data.importSummary', {
              entries: importResult.entries.imported,
              entriesSkipped: importResult.entries.skipped,
              goals: importResult.goals.imported,
              goalsSkipped: importResult.goals.skipped,
            })}
          </p>
        )}
      </section>

      <section
        style={{
          marginTop: '1rem',
          border: '1px solid var(--border)',
          borderRadius: '12px',
          padding: '1rem',
          background: 'var(--surface)',
        }}
        aria-label={t('settings.activity.title')}
      >
        <h3 style={{ marginTop: 0, marginBottom: '0.5rem', color: 'var(--text)' }}>{t('settings.activity.title')}</h3>
        <p style={{ marginTop: 0, marginBottom: '0.75rem', color: 'var(--text-muted)', fontSize: '0.9rem' }}>
          {t('settings.activity.description')}
        </p>

        {activityLoading ? (
          <p style={{ margin: 0, color: 'var(--text-muted)', fontSize: '0.9rem' }}>{t('settings.activity.loading')}</p>
        ) : activityError ? (
          <p style={{ margin: 0, color: 'var(--danger)', fontSize: '0.9rem' }}>{activityError}</p>
        ) : activities.length === 0 ? (
          <p style={{ margin: 0, color: 'var(--text-muted)', fontSize: '0.9rem' }}>
            {t('settings.activity.empty')}
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
                    {describeActivity(activity, t)}
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
                {loadingMore ? t('common.loading') : t('settings.activity.loadMore')}
              </button>
            )}
          </>
        )}
      </section>

      <footer style={{ marginTop: '1.5rem', color: 'var(--text-muted)', fontSize: '0.82rem' }}>
        Nightlio v{__APP_VERSION__}
        {config.version && config.version !== __APP_VERSION__ && ` · API v${config.version}`}
      </footer>
    </div>
  );
};

export default SettingsView;
