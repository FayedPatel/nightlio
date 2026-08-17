import './LandingPage.css';
import { LandingNav, LandingFooter } from './LandingPage';

const GITHUB_URL = 'https://github.com/FayedPatel/nightlio';
const UPSTREAM_URL = 'https://github.com/shirsakm/nightlio';

const AboutPage = () => {
  return (
    <div className="landing">
      <LandingNav active="about" />

      <main className="landing__article">
        <h1>Why I forked Nightlio</h1>
        <p className="landing__subtitle">
          This instance is my own build of Nightlio — forked, maintained, and
          by now rewritten top to bottom, because it is the journal I open
          every single day.
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
          your thoughts. Everything here still stands on that foundation, and
          the credit for it belongs to the original author. This is a fork,
          and it says so proudly.
        </p>

        <h2>Why the fork</h2>
        <p>
          I wanted Nightlio to be the journal I actually live in, which meant
          owning the code — fixing what bothered me, shaping it around my own
          server, and not waiting on anyone else&apos;s roadmap. Since forking,
          this build has grown its own direction:
        </p>
        <ul>
          <li>Single sign-on through any OIDC provider (I run Pocket ID)</li>
          <li>Backdated entries and goal completions for the days I forget</li>
          <li>Configurable themes — including the synthwave look — saved per account</li>
          <li>Achievements that play fair, like the per-day Data Lover streak</li>
          <li>A full test pyramid: backend unit and integration tests, frontend unit tests, and browser tests on desktop and mobile</li>
          <li>Docker images for amd64 and arm64, built and checked on every change</li>
        </ul>

        <h2>The v0.4.0 rewrite</h2>
        <p>
          For v0.4.0 I rebuilt the whole stack. The Python backend became a
          Rust API built on Axum and rusqlite, and the frontend moved to
          strict TypeScript. Before porting a single route, I pinned the wire
          contract with golden fixtures and an OpenAPI document, so the new
          backend had to reproduce the old one byte for byte — a process that
          flushed out latent bugs the original code had been quietly living
          with. The payoff: the server image shrank from 624&nbsp;MB to
          158&nbsp;MB, memory use dropped from around 290&nbsp;MiB to about
          7.5&nbsp;MiB, and every request is now checked against a typed
          contract instead of hope.
        </p>

        <h2>Philosophy</h2>
        <p>
          The original promise stays untouched: your data is yours. One SQLite
          file on your own machine, no telemetry ever, and every feature earns
          its place by being something I use myself.
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
