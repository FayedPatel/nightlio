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

const Sidebar = () => {
  const { t } = useI18n();
  const items: NavItem[] = [
    { key: '/dashboard', labelKey: 'nav.home', icon: Home, end: true },
    { key: '/dashboard/history', labelKey: 'nav.history', icon: History },
    { key: '/dashboard/goals', labelKey: 'nav.goals', icon: Target },
    { key: '/dashboard/stats', labelKey: 'nav.stats', icon: BarChart3 },
    { key: '/dashboard/achievements', labelKey: 'nav.achievements', icon: Trophy },
  ];

  return (
    <aside className={`sidebar`}>
      <div className="sidebar__inner">
        <div className="sidebar__brand" style={{ alignItems: 'flex-start', flexDirection: 'column', gap: '0.75rem' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: '0.75rem' }}>
            <div style={{ width: 36, height: 36, borderRadius: 10, background: 'transparent', display: 'grid', placeItems: 'center', color: 'var(--text)', overflow: 'hidden' }}>
              <img
                src={'/logo.png'}
                alt={t('common.appName')}
                style={{ width: '100%', height: '100%', objectFit: 'contain', display: 'block', background: 'transparent', outline: 'none' }}
              />
            </div>
            <strong style={{ color: 'var(--text)', letterSpacing: '-0.01em', fontSize: '1.5rem', fontWeight: '700' }}>{t('common.appName')}</strong>
          </div>
          <span style={{ color: 'var(--text)' , opacity: 0.85, fontSize: '0.875rem', paddingLeft: '0.25rem' }}>{t('common.tagline')}</span>
        </div>

        <div className="sidebar__sections">
          {items.map(({ key, labelKey, icon: Icon, end }) => (
            <NavLink
              key={key}
              to={key}
              end={end ?? false}

              className={({ isActive }) => `sidebar__item ${isActive ? 'is-active' : ''}`}
              title={t(labelKey)}
            >
              <Icon size={18} style={{ flexShrink: 0 }} />
              <span>{t(labelKey)}</span>
            </NavLink>
          ))}
        </div>

        <div className="sidebar__footer">
          <NavLink
            to="/dashboard/settings"
            className={({ isActive }) => `sidebar__item ${isActive ? 'is-active' : ''}`}
            title={t('nav.settings')}
          >
            <Settings size={18} style={{ flexShrink: 0 }} />
            <span>{t('nav.settings')}</span>
          </NavLink>
        </div>
      </div>
    </aside>
  );
};

export default Sidebar;
