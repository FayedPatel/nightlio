import './LandingPage.css';
import { LandingNav, LandingFooter } from './LandingPage';

const GITHUB_URL = 'https://github.com/FayedPatel/nightlio';
const UPSTREAM_URL = 'https://github.com/shirsakm/nightlio';

const AboutPage = () => {
  return (
    <div className="landing">
      <header className="landing__hero">
        <LandingNav active="about" />
      </header>

      <main className="landing__article">
        <h1>Why I forked Nightlio</h1>
        <p className="landing__subtitle">
          This instance is my own build of Nightlio — maintained, deployed,
          and extended by me for my own daily use.
        </p>

        <h2>Where it came from</h2>
        <p>
          Nightlio was originally created by{' '}
          <a href="https://github.com/shirsakm" target="_blank" rel="noreferrer">
            shirsakm
          </a>{' '}
          as an open source, privacy-first mood tracker — you can find the{' '}
          <a href={UPSTREAM_URL} target="_blank" rel="noreferrer">
            original project on GitHub
          </a>
          . It nailed the core idea: your journal belongs on your own server,
          in one SQLite file, with no analytics and no cloud between you and
          your thoughts. Full credit to the original author for that
          foundation.
        </p>

        <h2>Why the fork</h2>
        <p>
          I wanted Nightlio to be the journal I actually live in every day,
          which meant taking ownership of the code and shaping it around my
          own deployment. Since forking, this build has grown its own
          direction:
        </p>
        <ul>
          <li>Single sign-on through any OIDC provider (I run Pocket ID)</li>
          <li>Backdated entries for the days I forget to journal</li>
          <li>Configurable themes — including the synthwave look — saved per account</li>
          <li>A full test suite: API, unit, and browser tests on desktop and mobile</li>
          <li>Hardened auth, Docker deployment, and CI that runs on every change</li>
        </ul>

        <h2>Philosophy</h2>
        <p>
          The original promise stays untouched: your data is yours. This fork
          just turns the volume up — self-hosted first, no telemetry ever,
          and every feature earns its place by being something I use myself.
        </p>
        <p>
          Found a bug or want to run it yourself? Head to{' '}
          <a href={GITHUB_URL} target="_blank" rel="noreferrer">
            my fork on GitHub
          </a>
          .
        </p>
      </main>

      <LandingFooter />
    </div>
  );
};

export default AboutPage;
