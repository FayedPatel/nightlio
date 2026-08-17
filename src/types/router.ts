import type { MoodValue } from './api';
import type { MoodEntryWithSelections } from '../hooks/useMoodData';

/**
 * location.state contract for the /dashboard/entry route. Producers: App's
 * handleMoodSelect / handleStartEdit / handleEditMoodSelect (and
 * AddEntryCard, which navigates here with no state at all). Consumer: the
 * useLocation() destructure in AppContent. Both keys are optional and the
 * whole state may be null/undefined.
 */
export interface EntryLocationState {
  mood?: MoodValue;
  entry?: MoodEntryWithSelections;
}

/**
 * location.state contract for the /dashboard/goals route. Producer:
 * HistoryView's "Add Goal" shortcut (openForm lands straight on the creation
 * form). Consumer: GoalsView, which reads the flag once and clears it with a
 * replace navigation (state: null).
 */
export interface GoalsLocationState {
  openForm?: boolean;
}
