import { ChevronUp, Moon } from 'lucide-react';

// Home's empty "today" state. Deliberately not the AddEntryCard tile: on
// home the mood row is already visible right above, so a card that (like
// AddEntryCard) navigates to the editor's mood prompt would be a redundant
// detour. This state just points at the mood row instead.
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
