import GoalCard from './GoalCard';
import type { GoalDisplay } from './GoalCard';
import AddGoalCard from './AddGoalCard';

interface GoalsListProps {
  goals: GoalDisplay[];
  onDelete: (goalId: number) => void;
  onUpdateProgress: (goalId: number) => void;
  onLogDay?: (goalId: number, date: string) => void;
  onAdd?: () => void;
}

const GoalsList = ({ goals, onDelete, onUpdateProgress, onLogDay, onAdd }: GoalsListProps) => {
  if (goals.length === 0) {
    return (
      <div className="card-grid">
        {onAdd && <AddGoalCard onAdd={onAdd} />}
      </div>
    );
  }

  return (
    <div className="card-grid">
      {onAdd && (
        <AddGoalCard onAdd={onAdd} />
      )}
      {goals.map(goal => (
        <GoalCard
          key={goal.id}
          goal={goal}
          onDelete={onDelete}
          onUpdateProgress={onUpdateProgress}
          onLogDay={onLogDay}
        />
      ))}
    </div>
  );
};

export default GoalsList;
