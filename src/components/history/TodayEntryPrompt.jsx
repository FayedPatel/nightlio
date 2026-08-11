import { ChevronUp, Moon } from 'lucide-react';

// Home's empty "today" state. Used to be the shared AddEntryCard tile, but
// on the dashboard route its click handler is a no-op: it dispatches
// `nightlio:new-entry`, which App.jsx's listener only turns into a
// `window.scrollTo({ top: 0 })` when already on `/dashboard` — it never
// opens the editor. Mood selection is the real entry point (MoodPicker's
// onMoodSelect navigates straight into EntryView), so this state points at
// the mood row above instead of offering a second, dead "Add Entry" button.
const TodayEntryPrompt = () => (
  <div className="today-entry-prompt" role="note">
    <div className="today-entry-prompt__icon" aria-hidden="true">
      <Moon size={20} strokeWidth={1.8} />
    </div>
    <div className="today-entry-prompt__text">
      <p className="today-entry-prompt__title">How are you feeling tonight?</p>
      <p className="today-entry-prompt__subtitle">
        <ChevronUp size={14} className="today-entry-prompt__nudge" aria-hidden="true" />
        Tap a mood above to start today&rsquo;s entry.
      </p>
    </div>
  </div>
);

export default TodayEntryPrompt;
