import { ArrowRight } from 'lucide-react';
import HistoryEntry from './HistoryEntry';
import { isTodayEntry } from './TodayEntryCard';
import { useI18n } from '../../i18n';
import type { MoodEntryWithSelections } from '../../hooks/useMoodData';

const RECENT_LIMIT = 5;

interface RecentEntriesProps {
  entries?: MoodEntryWithSelections[];
  onDelete: (entryId: number) => void;
  onEdit?: (entry: MoodEntryWithSelections) => void;
  onViewAll: () => void;
}

// Dashboard "Recent Entries" (Phase 8c restore): a short, capped preview of
// the entry list — full history has its own page/nav slot. Today's entry is
// excluded here since the dashboard already shows it via TodayEntryCard
// right above this section; showing it twice would just be noise.
const RecentEntries = ({ entries = [], onDelete, onEdit, onViewAll }: RecentEntriesProps) => {
  const { t } = useI18n();
  const recent = entries.filter((entry) => !isTodayEntry(entry)).slice(0, RECENT_LIMIT);

  // Render only when there is something to show — an empty-history one-liner
  // would just duplicate the "add entry" CTA TodayEntryCard already renders.
  // The whole <section> (not just its contents) is skipped so an empty
  // section doesn't still eat the section+section spacing gap.
  if (recent.length === 0) return null;

  return (
    <section className="dashboard-section" aria-label={t('history.recentEntriesAria')}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 12 }}>
        <h2
          style={{
            margin: 0,
            paddingLeft: 'calc(var(--space-1) / 2)',
            color: 'var(--text)',
            fontWeight: 600,
            fontSize: '1.1rem',
          }}
        >
          {t('history.recentEntries')}
        </h2>
        <button
          type="button"
          onClick={onViewAll}
          className="dashboard-section__view-all"
        >
          {t('common.viewAll')}
          <ArrowRight size={14} aria-hidden="true" />
        </button>
      </div>

      <div className="card-grid">
        {recent.map((entry) => (
          <HistoryEntry
            key={entry.id || entry.date}
            entry={entry}
            onDelete={onDelete}
            onEdit={onEdit}
          />
        ))}
      </div>
    </section>
  );
};

export default RecentEntries;
