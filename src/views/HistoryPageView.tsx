import { useEffect, useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import HistoryList from '../components/history/HistoryList';
import { useI18n } from '../i18n';
import type { MoodEntryWithSelections } from '../hooks/useMoodData';
import type { HistoryLocationState } from '../types/router';

const HIGHLIGHT_MS = 3000;

interface HistoryPageViewProps {
  entries: MoodEntryWithSelections[];
  loading: boolean;
  error: string | null;
  onDelete: (entryId: number) => void;
  onEdit: (entry: MoodEntryWithSelections) => void;
  searchResults: MoodEntryWithSelections[] | null;
}

// Dedicated History route (added Phase 7c). The full entry list used to be
// embedded straight into the home dashboard below Goals; home now shows
// only today's entry state, so History gets promoted to its own page with a
// nav slot (Sidebar + BottomNav) instead of being reachable only from home.
const HistoryPageView = ({ entries, loading, error, onDelete, onEdit, searchResults }: HistoryPageViewProps) => {
  const { t } = useI18n();
  const location = useLocation();
  const navigate = useNavigate();
  // Settings' activity links land here with an entry to scroll to and pulse.
  const locationState: HistoryLocationState | null = location.state;
  const [highlightId, setHighlightId] = useState(() => locationState?.highlightEntryId ?? null);

  // Consume the id so back/refresh doesn't highlight again.
  useEffect(() => {
    if (locationState?.highlightEntryId != null) {
      navigate('/dashboard/history', { replace: true, state: null });
    }
  }, [locationState, navigate]);

  // An active search that filters the entry out leaves nothing to find.
  useEffect(() => {
    if (highlightId == null || loading) return;
    const row = document.querySelector<HTMLElement>(`[data-entry-id="${highlightId}"]`);
    row?.scrollIntoView?.({ block: 'center' });
    // Focus follows so keyboard and screen-reader users land on the entry too.
    row?.querySelector<HTMLElement>('.entry-card')?.focus({ preventScroll: true });
    const timer = setTimeout(() => setHighlightId(null), HIGHLIGHT_MS);
    return () => clearTimeout(timer);
  }, [highlightId, loading]);

  return (
    <section aria-label={t('history.sectionAria')} id="history-section">
      <h1 style={{ margin: '0 0 var(--space-3) 0', paddingLeft: 'calc(var(--space-1) / 2)', color: 'var(--text)' }}>
        {searchResults !== null ? t('history.searchResults', { count: searchResults.length }) : t('history.title')}
      </h1>
      <HistoryList
        entries={entries}
        loading={loading}
        error={error}
        onDelete={onDelete}
        onEdit={onEdit}
        highlightId={highlightId}
      />
    </section>
  );
};

export default HistoryPageView;
