import { useState, useEffect, useRef } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { Target } from 'lucide-react';
import GoalsList from '../components/goals/GoalsList';
import GoalForm from '../components/goals/GoalForm';
import type { GoalFormData, GoalFormHandle } from '../components/goals/GoalForm';
import type { GoalDisplay } from '../components/goals/GoalCard';
import Skeleton from '../components/ui/Skeleton';
import { useToast } from '../components/ui/ToastProvider';
import { todayISO, formatEntryDate } from '../utils/dateUtils';
import apiService from '../services/api';
import type { GoalsLocationState } from '../types/router';
import { useI18n } from '../i18n';

/**
 * GoalForm's typed submit payload no longer carries frequencyNumber (it only
 * ever sends title/description/frequency), but this handler still consults
 * it first — kept for exact behavior; see the dead-branch note in the
 * migration report.
 */
type NewGoalInput = GoalFormData & { frequencyNumber?: number | string };

const GoalsView = () => {
  const location = useLocation();
  const navigate = useNavigate();
  const { show } = useToast();
  const { t } = useI18n();
  const [goals, setGoals] = useState<GoalDisplay[]>([]);
  const [loading, setLoading] = useState(true);
  // Home's "Add Goal" card navigates here with openForm so one click lands
  // on the creation form instead of requiring a second click on this page.
  const locationState: GoalsLocationState | null = location.state;
  const [showForm, setShowForm] = useState(() => Boolean(locationState?.openForm));
  const formRef = useRef<GoalFormHandle | null>(null);

  // Consume the flag so back/refresh shows the normal Goals page rather
  // than re-opening the form.
  useEffect(() => {
    if (locationState?.openForm) {
      navigate('/dashboard/goals', { replace: true, state: null });
    }
  }, [locationState, navigate]);
  const suggestions = [
    { t: t('goals.suggestions.meditation.title'), d: t('goals.suggestions.meditation.description') },
    { t: t('goals.suggestions.walk.title'), d: t('goals.suggestions.walk.description') },
    { t: t('goals.suggestions.read.title'), d: t('goals.suggestions.read.description') },
    { t: t('goals.suggestions.water.title'), d: t('goals.suggestions.water.description') },
    { t: t('goals.suggestions.stretch.title'), d: t('goals.suggestions.stretch.description') },
    { t: t('goals.suggestions.language.title'), d: t('goals.suggestions.language.description') },
    { t: t('goals.suggestions.journal.title'), d: t('goals.suggestions.journal.description') },
  ];

  const handlePrefill = (title: string, description: string) => {
    if (formRef.current && typeof formRef.current.prefill === 'function') {
      formRef.current.prefill(title, description);
    }
  };

  useEffect(() => {
    let mounted = true;
    (async () => {
      try {
        const data = await apiService.getGoals();
        if (!mounted) return;
        const d = new Date();
        const today = `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,'0')}-${String(d.getDate()).padStart(2,'0')}`;
        const mapped: GoalDisplay[] = (data || []).map(g => ({
          id: g.id,
          title: g.title,
          description: g.description,
          frequency: t('goals.frequencyDaysAWeek', { count: g.frequency_per_week }),
          completed: g.completed ?? 0,
          total: g.frequency_per_week ?? 0,
          streak: g.streak ?? 0,
          created_at: g.created_at,
          last_completed_date: g.last_completed_date || null,
          _doneToday: (() => {
            try {
              const localKey = `goal_done_${g.id}`;
              const localVal = typeof localStorage !== 'undefined' ? localStorage.getItem(localKey) : null;
              return (localVal === today) || g.already_completed_today === true || (g.last_completed_date === today);
            } catch {
              return g.already_completed_today === true || (g.last_completed_date === today);
            }
          })(),
        }));
        setGoals(mapped);
  } catch {
        // fallback: keep goals empty; UI can still add
      } finally {
        if (mounted) setLoading(false);
      }
    })();
    return () => { mounted = false; };
  }, []);

  const handleAddGoal = (newGoal: NewGoalInput) => {
    (async () => {
      try {
        // parseInt stringifies its argument anyway; String() only makes the
        // coercion explicit for the number|string frequencyNumber type.
        const freqNum = newGoal.frequencyNumber ? parseInt(String(newGoal.frequencyNumber)) : parseInt((newGoal.frequency || '0').split(' ')[0] ?? '0');
        const resp = await apiService.createGoal({
          title: newGoal.title,
          description: newGoal.description,
          frequency: Number.isFinite(freqNum) && freqNum > 0 ? freqNum : 3,
        });
        const id = resp?.id ?? Date.now();
        const goal: GoalDisplay = {
          id,
          title: newGoal.title,
          description: newGoal.description,
          frequency: t('goals.frequencyDaysAWeek', { count: Number.isFinite(freqNum) && freqNum > 0 ? freqNum : 3 }),
          completed: 0,
          total: Number.isFinite(freqNum) && freqNum > 0 ? freqNum : 3,
          streak: 0,
          created_at: new Date().toISOString()
        };
        setGoals(prev => [goal, ...prev]);
  } catch {
        // optimistic add on failure
        const freqNum = newGoal.frequencyNumber ? parseInt(String(newGoal.frequencyNumber)) : parseInt((newGoal.frequency || '0').split(' ')[0] ?? '0');
        const goal: GoalDisplay = {
          id: Date.now(),
          title: newGoal.title,
          description: newGoal.description,
          frequency: t('goals.frequencyDaysAWeek', { count: Number.isFinite(freqNum) && freqNum > 0 ? freqNum : 3 }),
          completed: 0,
          total: Number.isFinite(freqNum) && freqNum > 0 ? freqNum : 3,
          streak: 0,
          created_at: new Date().toISOString()
        };
        setGoals(prev => [goal, ...prev]);
      } finally {
        setShowForm(false);
      }
    })();
  };

  const handleDeleteGoal = (goalId: number) => {
    try {
      if (typeof localStorage !== 'undefined') {
        localStorage.removeItem(`goal_done_${goalId}`);
      }
    } catch {
      // localStorage access failed
    }
    setGoals(prev => prev.filter(goal => goal.id !== goalId));
    apiService.deleteGoal(goalId).catch(() => {});
  };

  const handleUpdateProgress = (goalId: number) => {
  const d = new Date();
  const today = `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,'0')}-${String(d.getDate()).padStart(2,'0')}`;
    // Guard: if done today already, ignore and do not call API
    const target = goals.find(g => g.id === goalId);
    if (!target) return;
  if (target.last_completed_date === today || target._doneToday) return;
    // Optimistically lock the button for today without incrementing counts
  try {
    if (typeof localStorage !== 'undefined') {
      localStorage.setItem(`goal_done_${goalId}`, today);
    }
  } catch {
    // localStorage access failed
  }
  setGoals(prev => prev.map(g => g.id === goalId ? { ...g, last_completed_date: today, _doneToday: true } : g));
    apiService.incrementGoalProgress(goalId).then(updated => {
      if (!updated) return;
      setGoals(prev => prev.map(g => g.id === goalId ? {
        ...g,
        completed: updated.completed ?? g.completed,
        total: updated.frequency_per_week ?? g.total,
        streak: updated.streak ?? g.streak,
        last_completed_date: updated.last_completed_date || today,
        _doneToday: (() => {
          const serverDone = updated.already_completed_today === true || (updated.last_completed_date === today);
          if (serverDone) return true;
          try {
            const localVal = typeof localStorage !== 'undefined' ? localStorage.getItem(`goal_done_${goalId}`) : null;
            return localVal === today;
          } catch {
            return false;
          }
        })(),
        frequency: t('goals.frequencyDaysAWeek', { count: updated.frequency_per_week ?? g.total })
      } : g));
    }).catch(() => {
      // Revert lock on failure
      try {
        if (typeof localStorage !== 'undefined') {
          // Remove the optimistic local lock on failure
          const existing = localStorage.getItem(`goal_done_${goalId}`);
          if (existing === today) localStorage.removeItem(`goal_done_${goalId}`);
        }
      } catch {
        // localStorage access failed
      }
      setGoals(prev => prev.map(g => g.id === goalId ? { ...g, last_completed_date: target.last_completed_date || null, _doneToday: target.last_completed_date === today } : g));
    });
  };

  // Backdated completion: log a day the user forgot to record. The server
  // inserts the goal_completions row idempotently and only bumps the weekly
  // counter for dates inside the current week.
  const handleLogDay = (goalId: number, date: string) => {
    const today = todayISO();
    apiService.incrementGoalProgress(goalId, date).then(updated => {
      if (!updated) return;
      if (date === today && !updated.already_logged) {
        try {
          if (typeof localStorage !== 'undefined') {
            localStorage.setItem(`goal_done_${goalId}`, today);
          }
        } catch {
          // localStorage access failed
        }
      }
      setGoals(prev => prev.map(g => g.id === goalId ? {
        ...g,
        completed: updated.completed ?? g.completed,
        total: updated.frequency_per_week ?? g.total,
        streak: updated.streak ?? g.streak,
        last_completed_date: updated.last_completed_date || g.last_completed_date,
        _doneToday: updated.already_completed_today === true || g._doneToday === true,
        frequency: t('goals.frequencyDaysAWeek', { count: updated.frequency_per_week ?? g.total }),
      } : g));
      if (updated.already_logged) {
        show(t('toast.alreadyLoggedFor', { date: formatEntryDate(date) }), 'info');
      } else {
        show(t('toast.loggedFor', { date: formatEntryDate(date) }), 'success');
      }
    }).catch(() => {
      show(t('toast.logDayFailed'), 'error');
    });
  };

  if (showForm) {
    return (
      <div>
        <div style={{ display: 'flex', alignItems: 'center', gap: '1rem', marginBottom: '2rem' }}>
          <div style={{
            width: 40,
            height: 40,
            borderRadius: 12,
            background: 'var(--accent-bg)',
            display: 'grid',
            placeItems: 'center',
            color: '#fff'
          }}>
            <Target size={20} />
          </div>
          <div>
            <h1 style={{ margin: 0, color: 'var(--text)', fontSize: '1.75rem', fontWeight: '700' }}>{t('goals.addNew.title')}</h1>
            <p style={{ margin: 0, color: 'var(--text)', opacity: 0.8, fontSize: '0.95rem' }}>
              {t('goals.addNew.subtitle')}
            </p>
          </div>
        </div>

        <div style={{ display: 'flex', gap: 24, alignItems: 'flex-start', flexWrap: 'wrap' }}>
          <div style={{ flex: '1 1 480px', minWidth: 280 }}>
            <GoalForm
              ref={formRef}
              showInlineSuggestions={false}
              onSubmit={handleAddGoal}
              onCancel={() => setShowForm(false)}
            />
          </div>
          <aside style={{ flex: '0 0 320px', minWidth: 260 }}>
            <div style={{
              position: 'sticky', top: 12,
              border: '1px solid var(--border)',
              background: 'var(--surface)',
              borderRadius: 16,
              padding: 16
            }}>
              <div style={{ color: 'var(--text-muted)', fontSize: '0.85rem', marginBottom: 8 }}>{t('goals.quickSuggestions')}</div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                {suggestions.map(s => (
                  <button
                    key={s.t}
                    type="button"
                    onClick={() => handlePrefill(s.t, s.d)}
                    style={{
                      textAlign: 'left',
                      padding: '10px 12px',
                      borderRadius: 10,
                      border: '1px solid var(--border)',
                      background: 'var(--surface-2, var(--surface))',
                      color: 'var(--text)',
                      cursor: 'pointer',
                      transition: 'background 0.2s, border-color 0.2s'
                    }}
                  >
                    <div style={{ fontWeight: 600, fontSize: '0.95rem', marginBottom: 4 }}>{s.t}</div>
                    <div style={{ color: 'var(--text-muted)', fontSize: '0.85rem' }}>{s.d}</div>
                  </button>
                ))}
              </div>
            </div>
          </aside>
        </div>
      </div>
    );
  }

  return (
    <div>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: '2rem' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: '1rem' }}>
          <div style={{
            width: 40,
            height: 40,
            borderRadius: 12,
            background: 'var(--accent-bg)',
            display: 'grid',
            placeItems: 'center',
            color: '#fff'
          }}>
            <Target size={20} />
          </div>
          <div>
            <h1 style={{ margin: 0, color: 'var(--text)', fontSize: '1.75rem', fontWeight: '700' }}>{t('goals.title')}</h1>
            <p style={{ margin: 0, color: 'var(--text)', opacity: 0.8, fontSize: '0.95rem' }}>
              {t('goals.subtitle')}
            </p>
          </div>
        </div>
      </div>

      {loading ? (
        <div>
          <div className="card-grid">
            {[1,2,3,4].map((i) => (
              <div key={i}>
                <Skeleton height={180} radius={16} />
              </div>
            ))}
          </div>
        </div>
      ) : (
        <GoalsList
          goals={goals}
          onDelete={handleDeleteGoal}
          onUpdateProgress={handleUpdateProgress}
          onLogDay={handleLogDay}
          onAdd={() => setShowForm(true)}
        />
      )}
    </div>
  );
};

export default GoalsView;
