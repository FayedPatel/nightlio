import HistoryList from '../components/history/HistoryList';
import { useI18n } from '../i18n';
import type { MoodEntryWithSelections } from '../hooks/useMoodData';

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
      />
    </section>
  );
};

export default HistoryPageView;
