import { MOODS } from '../../utils/moodUtils';
import type { Mood } from '../../utils/moodUtils';
import apiService from '../../services/api';
import { useConfig } from '../../contexts/ConfigContext';
import type { MoodValue } from '../../types/api';
import type { PlayMoodMusicDetail } from './MusicDock';
import './MoodPicker.css';

interface MoodPickerProps {
  onMoodSelect: (moodValue: MoodValue) => void;
}

const MoodPicker = ({ onMoodSelect }: MoodPickerProps) => {
  const { config } = useConfig();

  //Handler to manage both saving the mood and playing music
  const handleMoodClick = async (mood: Mood) => {
    onMoodSelect(mood.value);

    if (!config.enable_mood_music) {
      return;
    }

    // Fetch music only when the feature is enabled.
    try {
      const data = await apiService.getMoodMusic(mood.tag);

      if (data.audio_url) {
        // Broadcast the music data to the permanent MusicDock
        const detail: PlayMoodMusicDetail = { ...data, color: mood.color };
        window.dispatchEvent(new CustomEvent('playMoodMusic', { detail }));
      }
    } catch (err) {
      console.error("Music fetch failed:", err);
    }
  };

  return (
    <div className="mood-grid">
      {MOODS.map(mood => {
        const IconComponent = mood.icon;
        return (
          <button
            key={mood.value}
            onClick={() => handleMoodClick(mood)}
            className="mood-button"
            style={{ color: mood.color }}
            title={mood.label}
          >
            <IconComponent size={40} strokeWidth={1.5} />
          </button>
        );
      })}
    </div>
  );
};

export default MoodPicker;
