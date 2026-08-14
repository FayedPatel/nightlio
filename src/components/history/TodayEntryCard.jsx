import HistoryEntry from './HistoryEntry';
import TodayEntryPrompt from './TodayEntryPrompt';
import { entryDateKey, todayISO } from '../../utils/dateUtils';

// Exported so other dashboard sections (RecentEntries) can filter out
// today's own entry using the exact same comparison as this card, instead
// of re-deriving a slightly different one and risking the two disagreeing
// about what "today" matched. Compares normalised ISO day keys so both
// stored date shapes (M/D/YYYY and YYYY-MM-DD) work.
export const isTodayEntry = (entry) => entryDateKey(entry.date) === todayISO();

// Home's "today" widget (Phase 7c, dashboard polish pass): shows EVERY entry
// written today, newest first (multiple same-day entries are a supported
// flow — showing only the latest made the others invisible on the
// dashboard, since RecentEntries excludes the whole day). Each row keeps the
// normal edit/delete access from HistoryEntry, rendered `featured` as
// full-width rows. No entry yet: a prompt pointing at the mood row above.
const TodayEntryCard = ({ pastEntries = [], onDelete, onEdit }) => {
  const todaysEntries = pastEntries
    .filter(isTodayEntry)
    .sort((a, b) => {
      const aTime = new Date(a.created_at || a.time || 0).getTime();
      const bTime = new Date(b.created_at || b.time || 0).getTime();
      if (aTime !== bTime) return bTime - aTime;
      return (b.id || 0) - (a.id || 0);
    });

  if (todaysEntries.length === 0) {
    return <TodayEntryPrompt />;
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      {todaysEntries.map((entry) => (
        <HistoryEntry
          key={entry.id || entry.created_at}
          entry={entry}
          onDelete={onDelete}
          onEdit={onEdit}
          featured
        />
      ))}
    </div>
  );
};

export default TodayEntryCard;
