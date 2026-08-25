import { useNavigate } from 'react-router-dom';
import MoodPicker from '../components/mood/MoodPicker';
import TodayEntryCard from '../components/history/TodayEntryCard';
import RecentEntries from '../components/history/RecentEntries';
import GoalsSection from '../components/goals/GoalsSection';
import { useI18n } from '../i18n';
import type { MoodValue } from '../types/api';
import type { MoodEntryWithSelections } from '../hooks/useMoodData';
import type { GoalsLocationState } from '../types/router';

interface HistoryViewProps {
  pastEntries: MoodEntryWithSelections[];
  onMoodSelect: (moodValue: MoodValue) => void;
  onDelete: (entryId: number) => void;
  onEdit: (entry: MoodEntryWithSelections) => void;
}

// Home dashboard (Phase 7c mood-first, Phase 8c restore, dashboard polish
// pass): mood picker + today's date/time as the hero, then today's entry
// state as its own full-width row below it (existing entry with edit
// access, or a prompt pointing back at the mood row), then compact "Active
// Goals" and "Recent Entries" previews — both link out to their full pages
// (Goals, History already have their own nav slots) rather than duplicating
// the whole list here. Every section below the hero shares the
// `dashboard-section` top-margin so the vertical rhythm stays consistent.
const HistoryView = ({ pastEntries, onMoodSelect, onDelete, onEdit }: HistoryViewProps) => {
  const navigate = useNavigate();
  const { t } = useI18n();
  const currentDate = new Date();
  const dateString = currentDate.toLocaleDateString('en-US', {
    weekday: 'long',
    year: 'numeric',
    month: 'long',
    day: 'numeric'
  });
  const timeString = currentDate.toLocaleTimeString('en-US', {
    hour: '2-digit',
    minute: '2-digit',
    hour12: true
  });

  return (
    <>
      <div className="history-header">
        <MoodPicker onMoodSelect={onMoodSelect} />
        <div className="history-date">
          <h2 className="history-today-title">{t('common.today')}</h2>
          <div className="history-datetime-group">
            <span className="history-date-part">{dateString}</span>
            <span className="history-time-part">{timeString}</span>
          </div>
        </div>
      </div>

      <section className="dashboard-section" aria-label={t('history.todaysEntryAria')}>
        <TodayEntryCard pastEntries={pastEntries} onDelete={onDelete} onEdit={onEdit} />
      </section>

      <section className="dashboard-section" aria-label={t('history.activeGoalsAria')}>
        <GoalsSection onNavigateToGoals={() => navigate('goals', { state: { openForm: true } satisfies GoalsLocationState })} />
      </section>

      <RecentEntries
        entries={pastEntries}
        onDelete={onDelete}
        onEdit={onEdit}
        onViewAll={() => navigate('history')}
      />
    </>
  );
};

export default HistoryView;
