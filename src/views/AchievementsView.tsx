import { useState, useEffect } from 'react';
import Modal from '../components/ui/Modal';
import ProgressBar from '../components/ui/ProgressBar';
import AchievementNFT from '../components/nft/AchievementNFT';
import apiService from '../services/api';
import type { Achievement, AchievementProgress, NewAchievement } from '../types/api';
import { useI18n } from '../i18n';
import type { TranslateFn } from '../i18n';

// All possible achievements (metadata-only NewAchievement shape: locked
// achievements have no id/earned_at/nft_* columns to show).
const getAllAchievements = (t: TranslateFn): NewAchievement[] => [
  {
    achievement_type: 'first_entry',
    name: t('achievements.firstEntry.name'),
    description: t('achievements.firstEntry.description'),
    icon: 'Zap',
    rarity: 'common'
  },
  {
    achievement_type: 'week_warrior',
    name: t('achievements.weekWarrior.name'),
    description: t('achievements.weekWarrior.description'),
    icon: 'Flame',
    rarity: 'uncommon'
  },
  {
    achievement_type: 'consistency_king',
    name: t('achievements.consistencyKing.name'),
    description: t('achievements.consistencyKing.description'),
    // Icon must match the backend achievement metadata (wire truth) so the
    // icon does not visibly change when the achievement unlocks.
    icon: 'Target',
    rarity: 'rare'
  },
  {
    achievement_type: 'data_lover',
    name: t('achievements.dataLover.name'),
    description: t('achievements.dataLover.description'),
    icon: 'BarChart3',
    rarity: 'uncommon'
  },
  {
    achievement_type: 'mood_master',
    name: t('achievements.moodMaster.name'),
    description: t('achievements.moodMaster.description'),
    icon: 'Crown',
    rarity: 'legendary'
  }
];

const AchievementsView = () => {
  const { t } = useI18n();
  // Web3 removed
  const [achievements, setAchievements] = useState<Achievement[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [active, setActive] = useState<Achievement | NewAchievement | null>(null);
  const [progress, setProgress] = useState<Partial<AchievementProgress>>({});

  useEffect(() => {
    loadAchievements();
  }, []);

  const loadAchievements = async () => {
    try {
      setLoading(true);
      const [data, prog] = await Promise.all([
        apiService.getUserAchievements(),
        apiService.getAchievementsProgress(),
      ]);
      setAchievements(data);
      setProgress(prog || {});
    } catch (err) {
      setError(t('errors.loadAchievements'));
      console.error('Failed to load achievements:', err);
    } finally {
      setLoading(false);
    }
  };

  if (loading) {
    return (
      <div style={{ marginTop: '2rem', textAlign: 'center', color: 'var(--text-muted)' }}>
        {t('achievements.loading')}
      </div>
    );
  }

  if (error) {
    return (
      <div style={{ marginTop: '2rem', textAlign: 'center', color: 'var(--accent-600)' }}>
        {error}
      </div>
    );
  }

  return (
    <div style={{ marginTop: '1.5rem' }}>

  {/* Web3 notice removed */}

      {/* Achievements Grid */}
      <div style={{
        display: 'flex',
        flexWrap: 'wrap',
        gap: '1rem',
        padding: 0,
        margin: 0,
        alignItems: 'stretch',
        alignContent: 'flex-start',
        width: '100%'
      }}>
        {/* All possible achievements */}
        {getAllAchievements(t).map((achievement, index) => {
          const unlockedAchievement = achievements.find(a => a.achievement_type === achievement.achievement_type);
          const isUnlocked = !!unlockedAchievement;
          const p = progress[achievement.achievement_type] || null;
          const progressValue = isUnlocked ? undefined : (p ? p.current : 0);
          const progressMax = p ? p.max : 7;
          return (
            // Card opens the detail modal: expose it as a keyboard-operable
            // button (named by the card's visible text) rather than a bare
            // clickable div.
            <div
              key={index}
              role="button"
              tabIndex={0}
              onClick={() => setActive(unlockedAchievement || achievement)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  setActive(unlockedAchievement || achievement);
                }
              }}
              style={{
                cursor: 'pointer',
                flex: '1 1 300px',
                minWidth: 260,
                maxWidth: '100%',
                display: 'flex'
              }}
            >
              <AchievementNFT
                achievement={unlockedAchievement || achievement}
                isUnlocked={isUnlocked}
                progressValue={progressValue}
                progressMax={progressMax}
              />
            </div>
          );
        })}
      </div>

      <Modal open={!!active} onClose={() => setActive(null)} title={active?.name || t('achievements.modalFallbackTitle')}>
        <p style={{ marginTop: 0 }}>{active?.description}</p>
        {!achievements.find(a => a.achievement_type === active?.achievement_type) && (() => {
          // Same lookup as before conversion: an undefined `active` used to
          // index as progress[undefined] -> undefined -> fallback.
          const p = (active ? progress[active.achievement_type] : undefined) || { current: 0, max: 7 };
          return <ProgressBar value={p.current || 0} max={p.max || 7} label={t('achievements.progressToUnlock')} />;
        })()}
        <div style={{ marginTop: 12, fontSize: 13, color: 'var(--text-muted)' }}>
          {t('achievements.tips')}
        </div>
      </Modal>
    </div>
  );
};

export default AchievementsView;
