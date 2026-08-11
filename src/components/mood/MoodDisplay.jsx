import { MOODS } from '../../utils/moodUtils';
import './MoodDisplay.css';

const MoodDisplay = ({ moodValue, size = 32, showLabel = true, children = null }) => {
  const mood = MOODS.find(m => m.value === moodValue);
  const isIconOnly = !showLabel;

  if (!mood) return null;

  const IconComponent = mood.icon;

  return (
    <div className={`mood-display${isIconOnly ? ' mood-display--icon-only' : ''}${!children ? ' mood-display--centered' : ''}`}>
      {children && (
        <div className="mood-display__content">
          {children}
        </div>
      )}
      <div className={`mood-display__icon${!showLabel ? ' mood-display__icon--compact' : ''}`}>
        <IconComponent
          size={isIconOnly ? Math.max(24, size - 2) : size}
          strokeWidth={1.5}
          style={{ color: mood.color }}
        />
        {showLabel && (
          <span className="mood-display__label" style={{ color: mood.color }}>
            Feeling {mood.label}
          </span>
        )}
      </div>
    </div>
  );
};

export default MoodDisplay;
