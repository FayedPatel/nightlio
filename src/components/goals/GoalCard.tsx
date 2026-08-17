import { useState } from 'react';
import { Target, Trash2, CheckCircle, Calendar, CalendarPlus } from 'lucide-react';
import { useToast } from '../ui/ToastProvider';
import { todayISO, yesterdayISO } from '../../utils/dateUtils';
import GoalStatsCalendar from './GoalStatsCalendar';
import Modal from '../ui/Modal';

/**
 * Client-side view model for a goal card. Built by GoalsView/GoalsSection
 * from the server `Goal` shape (src/types/api.ts): `frequency` is the
 * human-readable "N days a week" string, `total` mirrors frequency_per_week,
 * and `_doneToday` folds server + localStorage "done today" signals.
 * `created_at`, `last_completed_date`, and `_doneToday` are optional because
 * the optimistic-add path in GoalsView omits them.
 */
export interface GoalDisplay {
  id: number;
  title: string;
  description: string | null;
  frequency: string;
  completed: number;
  total: number;
  streak: number;
  created_at?: string;
  last_completed_date?: string | null | undefined;
  _doneToday?: boolean;
}

interface GoalCardProps {
  goal: GoalDisplay;
  onDelete: (goalId: number) => void;
  onUpdateProgress: (goalId: number) => void;
  onLogDay?: ((goalId: number, date: string) => void) | undefined;
}

const GoalCard = ({ goal, onDelete, onUpdateProgress, onLogDay }: GoalCardProps) => {
  const [isHovered, setIsHovered] = useState(false);
  const [isDeleting, setIsDeleting] = useState(false);
  const [showStats, setShowStats] = useState(false);
  const [showBackdate, setShowBackdate] = useState(false);
  const [backdate, setBackdate] = useState(() => yesterdayISO());
  const { show } = useToast();

  const handleDelete = async () => {
    if (!window.confirm('Are you sure you want to delete this goal?')) return;
    setIsDeleting(true);
    
    // Simulate API call delay
    setTimeout(() => {
      onDelete(goal.id);
      show('Goal deleted successfully', 'success');
      setIsDeleting(false);
    }, 500);
  };

  const handleMarkComplete = () => {
    const today = todayISO();
    try {
      const localVal = typeof localStorage !== 'undefined' ? localStorage.getItem(`goal_done_${goal.id}`) : null;
      if (localVal === today) {
        show('Already completed for today', 'info');
        return;
      }
    } catch {
      // localStorage access failed
    }
    if (goal.last_completed_date === today) {
      show('Already completed for today', 'info');
      return;
    }
    if (goal.completed >= goal.total) {
      show('Goal already completed for this period!', 'info');
      return;
    }
    onUpdateProgress(goal.id);
    show('Progress updated!', 'success');
  };

  const handleLogDay = () => {
    if (!backdate || backdate > todayISO()) return;
    onLogDay?.(goal.id, backdate);
    setShowBackdate(false);
  };

  const progressPercentage = (goal.completed / goal.total) * 100;
  const isCompleted = goal.completed >= goal.total;
  const isDoneToday = (() => {
    const today = todayISO();
    try {
      const localVal = typeof localStorage !== 'undefined' ? localStorage.getItem(`goal_done_${goal.id}`) : null;
      return (localVal === today) || goal.last_completed_date === today || goal._doneToday === true;
    } catch {
      return goal.last_completed_date === today || goal._doneToday === true;
    }
  })();

  return (
    <div
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
      onClick={() => setShowStats(true)}
      className="entry-card"
      style={{
        border: isHovered ? '1px solid color-mix(in oklab, var(--accent-600), transparent 55%)' : '1px solid var(--border)',
        boxShadow: isHovered ? 'var(--shadow-md)' : 'var(--shadow-sm)',
        position: 'relative',
        opacity: isDeleting ? 0.5 : 1,
        pointerEvents: isDeleting ? 'none' : 'auto',
        cursor: 'pointer',
        // Flex column + marginTop: auto on the progress block pins the
        // progress/actions group to the card bottom, so buttons line up
        // across cards whose descriptions wrap differently.
        display: 'flex',
        flexDirection: 'column'
      }}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); setShowStats(true); } }}
    >
      {/* Header: icon + delete button */}
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: '12px', position: 'relative' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: '10px', flex: 1, marginRight: goal.streak > 0 ? '60px' : '40px' }}>
          <span style={{ 
            color: 'var(--accent-600)', 
            display: 'flex', 
            alignItems: 'center', 
            justifyContent: 'center', 
            width: 28, 
            height: 28, 
            borderRadius: '50%', 
            background: 'var(--accent-bg-softer)', 
            border: '1px solid var(--border)' 
          }}>
            <Target size={16} strokeWidth={2} />
          </span>
          <div style={{ display: 'flex', alignItems: 'center', fontSize: '0.85rem', color: 'var(--text)', opacity: 0.8 }}>
            <Calendar size={14} style={{ marginRight: '4px' }} />
            <span>{goal.frequency}</span>
          </div>
        </div>
        
        <div style={{ display: 'flex', alignItems: 'center', gap: '8px', position: 'absolute', right: 0, top: '50%', transform: 'translateY(-50%)' }}>
          {goal.streak > 0 && (
            <div style={{
              display: 'flex',
              alignItems: 'center',
              background: 'var(--accent-600)',
              color: 'white',
              fontSize: '0.75rem',
              padding: '2px 6px',
              borderRadius: '12px',
              fontWeight: '500',
              lineHeight: '1'
            }}>
              🔥 {goal.streak}
            </div>
          )}
          <button
            onClick={(e) => { e.stopPropagation(); handleDelete(); }}
            style={{
              background: 'none',
              border: 'none',
              color: 'var(--text)',
              opacity: isHovered ? 0.7 : 0.4,
              cursor: 'pointer',
              padding: '4px',
              borderRadius: '4px',
              transition: 'opacity 0.2s',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center'
            }}
            aria-label="Delete goal"
          >
            <Trash2 size={14} />
          </button>
        </div>
      </div>

      {/* Title */}
      <div className="entry-card__title" style={{ marginBottom: '8px' }}>
        {goal.title}
      </div>

      {/* Description */}
      {goal.description && (
        <div className="entry-card__excerpt" style={{ marginBottom: '16px' }}>
          {goal.description}
        </div>
      )}

      {/* Progress Bar */}
      <div style={{ marginBottom: '12px', marginTop: 'auto' }}>
        <div style={{ 
          display: 'flex', 
          justifyContent: 'space-between', 
          alignItems: 'center', 
          marginBottom: '6px',
          fontSize: '0.85rem',
          color: 'var(--text)',
          opacity: 0.9
        }}>
          <span>Progress</span>
          <span>{goal.completed}/{goal.total}</span>
        </div>
        <div style={{
          width: '100%',
          height: '8px',
          background: 'var(--accent-bg-softer)',
          borderRadius: '4px',
          overflow: 'hidden'
        }}>
          <div style={{
            width: `${progressPercentage}%`,
            height: '100%',
            background: isCompleted ? 'var(--success)' : 'var(--accent-600)',
            transition: 'width 0.3s ease'
          }} />
        </div>
      </div>

      {/* Action Button */}
      <button
        onClick={(e) => { e.stopPropagation(); handleMarkComplete(); }}
        disabled={isDoneToday}
        style={{
          width: '100%',
          padding: '8px 12px',
          borderRadius: '8px',
          border: 'none',
          background: isDoneToday ? 'var(--success)' : 'var(--accent-bg)',
          color: '#fff',
          fontSize: '0.85rem',
          fontWeight: '500',
          cursor: isDoneToday ? 'default' : 'pointer',
          // Keep text crisp white even when disabled
          opacity: 1,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          gap: '6px',
          transition: 'background-color 0.2s'
        }}
      >
        <CheckCircle size={14} />
        {isDoneToday ? 'Completed' : 'Mark as done'}
      </button>

      {/* Backdate: log a completion for a day the user forgot to record.
          stopPropagation on the wrapper keeps clicks and date-input typing
          from opening the stats modal via the card's own handlers. */}
      <div
        role="presentation"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.stopPropagation()}
        style={{ marginTop: '8px' }}
      >
        {!showBackdate ? (
          <button
            type="button"
            onClick={() => setShowBackdate(true)}
            aria-label={`Log past day for ${goal.title}`}
            style={{
              width: '100%',
              padding: '8px 12px',
              borderRadius: '8px',
              border: '1px solid var(--border)',
              background: 'transparent',
              color: 'var(--text)',
              opacity: 0.85,
              fontSize: '0.85rem',
              fontWeight: '500',
              cursor: 'pointer',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              gap: '6px',
            }}
          >
            <CalendarPlus size={14} />
            Log past day
          </button>
        ) : (
          <div className="entry-date-section" style={{ marginBottom: 0 }}>
            <label
              className="entry-date-section__label"
              htmlFor={`goal-backdate-${goal.id}`}
            >
              Day you did it
            </label>
            <div className="entry-date-section__controls">
              <input
                id={`goal-backdate-${goal.id}`}
                type="date"
                className="entry-date-section__input"
                max={todayISO()}
                value={backdate}
                onChange={(e) => {
                  const next = e.target.value;
                  if (next && next <= todayISO()) setBackdate(next);
                }}
              />
              <button
                type="button"
                className={`entry-date-chip${backdate === yesterdayISO() ? ' is-active' : ''}`}
                onClick={() => setBackdate(yesterdayISO())}
              >
                Yesterday
              </button>
              <button
                type="button"
                className="entry-date-chip"
                style={{
                  background: 'var(--accent-bg)',
                  borderColor: 'transparent',
                  color: '#fff',
                }}
                onClick={handleLogDay}
              >
                Log it
              </button>
              <button
                type="button"
                className="entry-date-chip"
                onClick={() => setShowBackdate(false)}
                aria-label="Cancel logging a past day"
              >
                Cancel
              </button>
            </div>
          </div>
        )}
      </div>
      <Modal open={showStats} title="Goal Statistics" onClose={() => setShowStats(false)}>
        <GoalStatsCalendar goalId={goal.id} />
      </Modal>
    </div>
  );
};

export default GoalCard;
