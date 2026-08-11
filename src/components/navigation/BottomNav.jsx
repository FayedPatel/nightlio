import { Home, History, BarChart3, Trophy, Settings, Target } from 'lucide-react';
import { useEffect } from 'react';
import { NavLink, useLocation } from 'react-router-dom';

// 6 items at 640px width: measured against `.bottom-nav__item` min-width
// 44px + the row's own 12px side padding (App.css) — even at a 320px
// viewport that's ~49px/item average, still above the 44px touch-target
// floor. History is a primary destination (Phase 7c pulled it off home),
// not a Stats sub-view, so it gets its own slot rather than folding under
// Stats.
const BottomNav = ({ onLoadStatistics }) => {
  const items = [
    { key: '/dashboard', label: 'Home', icon: Home, end: true },
    { key: '/dashboard/history', label: 'History', icon: History },
    { key: '/dashboard/goals', label: 'Goals', icon: Target },
    { key: '/dashboard/stats', label: 'Stats', icon: BarChart3 },
    { key: '/dashboard/achievements', label: 'Awards', icon: Trophy },
    { key: '/dashboard/settings', label: 'Settings', icon: Settings },
  ];

  const location = useLocation();

  useEffect(() => {
    if (location.pathname.includes('stats') && typeof onLoadStatistics === 'function') {
      onLoadStatistics();
    }
  }, [location.pathname, onLoadStatistics]);

  return (
    <nav className="bottom-nav">
      {items.map(({ key, label, icon: Icon, end }) => (
        <NavLink
          key={key}
          to={key}
          end={end}

          className={({ isActive }) => `bottom-nav__item ${isActive ? 'is-active' : ''}`}
          aria-label={label}
        >
          <Icon size={20} />
          <span className="bottom-nav__label">{label}</span>
        </NavLink>
      ))}
    </nav>
  );
};

export default BottomNav;
