import './LandingPage.css';
import { useEffect } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { MOODS } from '../utils/moodUtils';

const GITHUB_URL = 'https://github.com/FayedPatel/nightlio';
const UPSTREAM_URL = 'https://github.com/shirsakm/nightlio';

const highlights = [
  {
    title: 'Privacy First',
    description:
      'Your data lives in one SQLite file on your own server. No third-party trackers, no analytics, no cloud.',
  },
  {
    title: 'Rich Journaling',
    description:
      'Markdown notes for every entry — formatting, lists, and links. Forgot a day? Backdate it.',
  },
  {
    title: 'Make It Yours',
    description:
      'Four built-in themes — including the synthwave look you are staring at — saved to your account.',
  },
];

const featureBlocks = [
  {
    title: 'Effortless Logging',
    items: [
      'Log your mood in seconds on a 5-point scale',
      'Tag entries with categories like "Sleep" or "Work"',
      'Autosave keeps every word without a save button',
    ],
  },
  {
    title: 'Gamified Consistency',
    items: [
      'Earn achievements as you build the habit',
      'Track your streak day to day',
      'Set weekly goals and mark daily progress',
    ],
  },
  {
    title: 'Insightful Analytics',
    items: [
      'Calendar heatmap of your mood history',
      'Rolling averages and trend lines',
      'Spot the patterns behind your state of mind',
    ],
  },
];

interface LandingNavProps {
  /** Highlights the matching nav link; only AboutPage passes 'about' today. */
  active?: 'about';
}

export const LandingNav = ({ active }: LandingNavProps) => (
  <nav className="landing__nav" aria-label="Main">
    <div className="landing__nav-inner">
    <Link className="landing__brand" to="/">
      <img src="/logo.png" alt="Nightlio logo" className="landing__brand-mark" />
      <span className="landing__brand-name">Nightlio</span>
    </Link>
    <div className="landing__nav-links">
      <Link to="/#features">Features</Link>
      <Link to="/about" className={active === 'about' ? 'is-active' : undefined}>
        About
      </Link>
    </div>
    <div className="landing__nav-actions">
      <a
        className="landing__button landing__button--icon"
        href={GITHUB_URL}
        target="_blank"
        rel="noreferrer"
        aria-label="GitHub"
      >
        <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" xmlns="http://www.w3.org/2000/svg">
          <path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405 1.02 0 2.04.135 3 .405 2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.285 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z" />
        </svg>
      </a>
      <Link className="landing__button landing__button--primary" to="/login">
        Sign in
      </Link>
    </div>
    </div>
  </nav>
);

export const LandingFooter = () => (
  <footer className="landing__footer">
    <p className="landing__footer-note">
      © 2026 Nightlio · maintained by Fayed Patel · forked from the{' '}
      <a href={UPSTREAM_URL} target="_blank" rel="noreferrer">
        original Nightlio
      </a>{' '}
      by shirsakm
    </p>
    <div className="landing__footer-links">
      <Link to="/about">About</Link>
      <a href={GITHUB_URL} target="_blank" rel="noreferrer">GitHub</a>
      <a href={`${GITHUB_URL}/issues`} target="_blank" rel="noreferrer">Contact</a>
    </div>
  </footer>
);

const LandingPage = () => {
  const location = useLocation();

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
            <span className="landing__tag">Self-hosted · Open source</span>
            <h1>
              Your moods.
              <br />
              <span className="landing__neon">Your server.</span>
              <br />
              Your rules.
            </h1>
            <p>
              Nightlio is a privacy-first mood tracker and daily journal that
              runs entirely on your own hardware. No ads, no subscriptions,
              and absolutely no data mining — just you and the night.
            </p>
            <div className="landing__cta-group">
              <Link className="landing__button landing__button--primary landing__button--lg" to="/login">
                Sign in
              </Link>
              <a
                className="landing__button landing__button--ghost landing__button--lg"
                href={GITHUB_URL}
                target="_blank"
                rel="noreferrer"
              >
                View on GitHub
              </a>
            </div>
          </div>

          <div className="landing__hero-visual">
            {/* Product motif: the app's real 5-point mood scale (same
                icons as MoodPicker), remapped to the synthwave palette. */}
            <div className="landing__card">
              <div className="landing__card-header">
                <span className="landing__card-label">The mood scale</span>
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
                Every entry starts with one tap on this scale, then as much or
                as little writing as you want.
              </p>
            </div>
          </div>
        </div>
      </header>

      <section id="why-nightlio" className="landing__section landing__section--alt">
        <h2>Built for mindful nights and focused mornings</h2>
        <div className="landing__tile-grid">
          {highlights.map((item) => (
            <article key={item.title} className="landing__tile">
              <h3>{item.title}</h3>
              <p>{item.description}</p>
            </article>
          ))}
        </div>
      </section>

      <section id="features" className="landing__section">
        <h2>Everything you need to capture your story</h2>
        <div className="landing__feature-columns">
          {featureBlocks.map((block) => (
            <div key={block.title} className="landing__feature">
              <h3>{block.title}</h3>
              <ul>
                {block.items.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      </section>

      <section id="self-host" className="landing__section landing__section--alt">
        <h2>Self-host it in minutes</h2>
        <div className="landing__quickstart">
          <div className="landing__terminal">
            <div className="landing__terminal-bar" aria-hidden="true">
              <i /><i /><i />
              <span>nightlio — zsh</span>
            </div>
            <pre className="landing__code"><code>{`git clone ${GITHUB_URL}.git
cd nightlio
cp .env.docker .env   # set SECRET_KEY + JWT_SECRET
docker-compose up -d`}</code></pre>
          </div>
          <p className="landing__quickstart-note">
            Two containers, one SQLite file, no external services. Full
            walkthrough in the{' '}
            <a href={`${GITHUB_URL}/blob/main/docs/SETUP.md`} target="_blank" rel="noreferrer">
              setup guide
            </a>.
          </p>
        </div>
      </section>

      <section id="cta" className="landing__section landing__section--cta">
        <div className="landing__cta">
          <div>
            <h2>Take the weight off your mind.</h2>
            <p>
              Spin it up with a single Docker command and start journaling
              tonight.
            </p>
          </div>
          <div className="landing__cta-buttons">
            {/* Single closing action: the footer right below already links
                GitHub, so repeating it here was pure duplication. */}
            <Link className="landing__button landing__button--primary landing__button--lg" to="/login">
              Sign in
            </Link>
          </div>
        </div>
      </section>

      <LandingFooter />
    </div>
  );
};

export default LandingPage;
