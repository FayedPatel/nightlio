import { Home, History, BarChart3, Trophy, Settings, Target } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { NavLink } from 'react-router-dom';
import { useI18n } from '../../i18n';
import type { I18nKey } from '../../i18n';

interface NavItem {
  key: string;
  labelKey: I18nKey;
  icon: LucideIcon;
  end?: boolean;
}

// 6 items at 640px width: measured against `.bottom-nav__item` min-width
// 44px + the row's own 12px side padding (App.css) — even at a 320px
// viewport that's ~49px/item average, still above the 44px touch-target
// floor. History is a primary destination (Phase 7c pulled it off home),
// not a Stats sub-view, so it gets its own slot rather than folding under
// Stats.
const BottomNav = () => {
  const { t } = useI18n();
  const items: NavItem[] = [
    { key: '/dashboard', labelKey: 'nav.home', icon: Home, end: true },
    { key: '/dashboard/history', labelKey: 'nav.history', icon: History },
    { key: '/dashboard/goals', labelKey: 'nav.goals', icon: Target },
    { key: '/dashboard/stats', labelKey: 'nav.statsShort', icon: BarChart3 },
    { key: '/dashboard/achievements', labelKey: 'nav.achievementsShort', icon: Trophy },
    { key: '/dashboard/settings', labelKey: 'nav.settings', icon: Settings },
  ];

  return (
    <nav className="bottom-nav">
      {items.map(({ key, labelKey, icon: Icon, end }) => (
        <NavLink
          key={key}
          to={key}
          end={end ?? false}

          className={({ isActive }) => `bottom-nav__item ${isActive ? 'is-active' : ''}`}
          aria-label={t(labelKey)}
        >
          <Icon size={20} />
          <span className="bottom-nav__label">{t(labelKey)}</span>
        </NavLink>
      ))}
    </nav>
  );
};

export default BottomNav;
