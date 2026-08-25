import { ChevronUp, Moon } from 'lucide-react';
import { useI18n } from '../../i18n';

// Home's empty "today" state. Deliberately not the AddEntryCard tile: on
// home the mood row is already visible right above, so a card that (like
// AddEntryCard) navigates to the editor's mood prompt would be a redundant
// detour. This state just points at the mood row instead.
const TodayEntryPrompt = () => {
  const { t } = useI18n();
  return (
    <div className="today-entry-prompt" role="note">
      <div className="today-entry-prompt__icon" aria-hidden="true">
        <Moon size={20} strokeWidth={1.8} />
      </div>
      <div className="today-entry-prompt__text">
        <p className="today-entry-prompt__title">{t('history.promptTitle')}</p>
        <p className="today-entry-prompt__subtitle">
          <ChevronUp size={14} className="today-entry-prompt__nudge" aria-hidden="true" />
          {t('history.promptSubtitle')}
        </p>
      </div>
    </div>
  );
};

export default TodayEntryPrompt;
