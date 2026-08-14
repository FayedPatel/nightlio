import GoalCard from './GoalCard';
import AddGoalCard from './AddGoalCard';

const GoalsList = ({ goals, onDelete, onUpdateProgress, onLogDay, onAdd }) => {
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
