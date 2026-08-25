import './LandingPage.css';
import { useEffect } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { MOODS } from '../utils/moodUtils';
import { useI18n } from '../i18n';
import type { I18nKey } from '../i18n';

const GITHUB_URL = 'https://github.com/FayedPatel/nightlio';
const UPSTREAM_URL = 'https://github.com/shirsakm/nightlio';

const highlights: { id: string; titleKey: I18nKey; descriptionKey: I18nKey }[] = [
  {
    id: 'privacy',
    titleKey: 'landing.highlights.privacy.title',
    descriptionKey: 'landing.highlights.privacy.description',
  },
  {
    id: 'journaling',
    titleKey: 'landing.highlights.journaling.title',
    descriptionKey: 'landing.highlights.journaling.description',
  },
  {
    id: 'yours',
    titleKey: 'landing.highlights.yours.title',
    descriptionKey: 'landing.highlights.yours.description',
  },
];

const featureBlocks: { id: string; titleKey: I18nKey; itemKeys: I18nKey[] }[] = [
  {
    id: 'logging',
    titleKey: 'landing.featureBlocks.logging.title',
    itemKeys: [
      'landing.featureBlocks.logging.item1',
      'landing.featureBlocks.logging.item2',
      'landing.featureBlocks.logging.item3',
    ],
  },
  {
    id: 'consistency',
    titleKey: 'landing.featureBlocks.consistency.title',
    itemKeys: [
      'landing.featureBlocks.consistency.item1',
      'landing.featureBlocks.consistency.item2',
      'landing.featureBlocks.consistency.item3',
    ],
  },
  {
    id: 'analytics',
    titleKey: 'landing.featureBlocks.analytics.title',
    itemKeys: [
      'landing.featureBlocks.analytics.item1',
      'landing.featureBlocks.analytics.item2',
      'landing.featureBlocks.analytics.item3',
    ],
  },
];

interface LandingNavProps {
  /** Highlights the matching nav link; only AboutPage passes 'about' today. */
  active?: 'about';
}

export const LandingNav = ({ active }: LandingNavProps) => {
  const { t } = useI18n();
  return (
    <nav className="landing__nav" aria-label={t('landing.navAria')}>
      <div className="landing__nav-inner">
      <Link className="landing__brand" to="/">
        <img src="/logo.png" alt={t('common.logoAlt')} className="landing__brand-mark" />
        <span className="landing__brand-name">{t('common.appName')}</span>
      </Link>
      <div className="landing__nav-links">
        <Link to="/#features">{t('landing.features')}</Link>
        <Link to="/about" className={active === 'about' ? 'is-active' : undefined}>
          {t('landing.about')}
        </Link>
      </div>
      <div className="landing__nav-actions">
        <a
          className="landing__button landing__button--icon"
          href={GITHUB_URL}
          target="_blank"
          rel="noreferrer"
          aria-label={t('landing.github')}
        >
          <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" xmlns="http://www.w3.org/2000/svg">
            <path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405 1.02 0 2.04.135 3 .405 2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.285 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z" />
          </svg>
        </a>
        <Link className="landing__button landing__button--primary" to="/login">
          {t('landing.signIn')}
        </Link>
      </div>
      </div>
    </nav>
  );
};

export const LandingFooter = () => {
  const { t } = useI18n();
  return (
    <footer className="landing__footer">
      <p className="landing__footer-note">
        {t('landing.footer.notePrefix')}{' '}
        <a href={UPSTREAM_URL} target="_blank" rel="noreferrer">
          {t('landing.footer.originalLink')}
        </a>{' '}
        {t('landing.footer.noteSuffix')}
      </p>
      <div className="landing__footer-links">
        <Link to="/about">{t('landing.about')}</Link>
        <a href={GITHUB_URL} target="_blank" rel="noreferrer">{t('landing.github')}</a>
        <a href={`${GITHUB_URL}/issues`} target="_blank" rel="noreferrer">{t('landing.contact')}</a>
      </div>
    </footer>
  );
};

const LandingPage = () => {
  const location = useLocation();
  const { t } = useI18n();

  // Hash links (e.g. Features from the About page navigating to /#features)
  // don't auto-scroll under client-side routing — do it explicitly.
  useEffect(() => {
    if (!location.hash) return;
    const target = document.querySelector(location.hash);
    if (target) target.scrollIntoView({ behavior: 'smooth' });
  }, [location.hash]);

  return (
    <div className="landing">
      {/* Sticky: outside the hero so it follows the whole page and keeps
          Sign in reachable without a scroll-to-top control. */}
      <LandingNav />

      <header className="landing__hero">
        <div className="landing__horizon" aria-hidden="true">
          <div className="landing__sun" />
          <div className="landing__grid-floor" />
        </div>

        <div className="landing__hero-body">
          <div className="landing__hero-copy">
            <span className="landing__tag">{t('landing.tag')}</span>
            <h1>
              {t('landing.hero.line1')}
              <br />
              <span className="landing__neon">{t('landing.hero.line2')}</span>
              <br />
              {t('landing.hero.line3')}
            </h1>
            <p>
              {t('landing.hero.description')}
            </p>
            <div className="landing__cta-group">
              <Link className="landing__button landing__button--primary landing__button--lg" to="/login">
                {t('landing.signIn')}
              </Link>
              <a
                className="landing__button landing__button--ghost landing__button--lg"
                href={GITHUB_URL}
                target="_blank"
                rel="noreferrer"
              >
                {t('landing.viewOnGitHub')}
              </a>
            </div>
          </div>

          <div className="landing__hero-visual">
            {/* Product motif: the app's real 5-point mood scale (same
                icons as MoodPicker), remapped to the synthwave palette. */}
            <div className="landing__card">
              <div className="landing__card-header">
                <span className="landing__card-label">{t('landing.card.label')}</span>
                <span className="landing__card-dots" aria-hidden="true">
                  <i /><i /><i />
                </span>
              </div>
              <div className="landing__mood-scale">
                {MOODS.map(({ icon: Icon, value, label }) => (
                  <div key={value} className="landing__mood-scale-item">
                    <span className={`landing__mood-scale-icon landing__mood-scale-icon--${value}`}>
                      <Icon size={22} strokeWidth={1.8} aria-hidden="true" />
                    </span>
                    <span className="landing__mood-scale-label">{label}</span>
                  </div>
                ))}
              </div>
              <p className="landing__note">
                {t('landing.card.note')}
              </p>
            </div>
          </div>
        </div>
      </header>

      <section id="why-nightlio" className="landing__section landing__section--alt">
        <h2>{t('landing.why.title')}</h2>
        <div className="landing__tile-grid">
          {highlights.map((item) => (
            <article key={item.id} className="landing__tile">
              <h3>{t(item.titleKey)}</h3>
              <p>{t(item.descriptionKey)}</p>
            </article>
          ))}
        </div>
      </section>

      <section id="features" className="landing__section">
        <h2>{t('landing.featuresSection.title')}</h2>
        <div className="landing__feature-columns">
          {featureBlocks.map((block) => (
            <div key={block.id} className="landing__feature">
              <h3>{t(block.titleKey)}</h3>
              <ul>
                {block.itemKeys.map((itemKey) => (
                  <li key={itemKey}>{t(itemKey)}</li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      </section>

      <section id="self-host" className="landing__section landing__section--alt">
        <h2>{t('landing.selfHost.title')}</h2>
        <div className="landing__quickstart">
          <div className="landing__terminal">
            <div className="landing__terminal-bar" aria-hidden="true">
              <i /><i /><i />
              <span>{t('landing.terminal.title')}</span>
            </div>
            <pre className="landing__code"><code>{`git clone ${GITHUB_URL}.git
cd nightlio
cp .env.docker .env   # set SECRET_KEY + JWT_SECRET
docker-compose up -d`}</code></pre>
          </div>
          <p className="landing__quickstart-note">
            {t('landing.quickstart.notePrefix')}{' '}
            <a href={`${GITHUB_URL}/blob/main/docs/SETUP.md`} target="_blank" rel="noreferrer">
              {t('landing.quickstart.setupGuide')}
            </a>.
          </p>
        </div>
      </section>

      <section id="cta" className="landing__section landing__section--cta">
        <div className="landing__cta">
          <div>
            <h2>{t('landing.cta.title')}</h2>
            <p>
              {t('landing.cta.description')}
            </p>
          </div>
          <div className="landing__cta-buttons">
            {/* Single closing action: the footer right below already links
                GitHub, so repeating it here was pure duplication. */}
            <Link className="landing__button landing__button--primary landing__button--lg" to="/login">
              {t('landing.signIn')}
            </Link>
          </div>
        </div>
      </section>

      <LandingFooter />
    </div>
  );
};

export default LandingPage;
