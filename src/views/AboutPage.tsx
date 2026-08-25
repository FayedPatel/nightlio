import './LandingPage.css';
import { LandingNav, LandingFooter } from './LandingPage';
import { useI18n } from '../i18n';

const GITHUB_URL = 'https://github.com/FayedPatel/nightlio';
const UPSTREAM_URL = 'https://github.com/shirsakm/nightlio';

const AboutPage = () => {
  const { t } = useI18n();
  return (
    <div className="landing">
      <LandingNav active="about" />

      <main className="landing__article">
        <h1>{t('about.title')}</h1>
        <p className="landing__subtitle">
          {t('about.subtitle')}
        </p>

        <h2>{t('about.origin.title')}</h2>
        <p>
          {t('about.origin.beforeAuthor')}{' '}
          <a href="https://github.com/shirsakm" target="_blank" rel="noreferrer">
            {t('about.origin.author')}
          </a>{' '}
          {t('about.origin.afterAuthor')}{' '}
          <a href={UPSTREAM_URL} target="_blank" rel="noreferrer">
            {t('about.origin.projectLink')}
          </a>
          {t('about.origin.afterLink')}
        </p>

        <h2>{t('about.fork.title')}</h2>
        <p>
          {t('about.fork.intro')}
        </p>
        <ul>
          <li>{t('about.fork.item1')}</li>
          <li>{t('about.fork.item2')}</li>
          <li>{t('about.fork.item3')}</li>
          <li>{t('about.fork.item4')}</li>
          <li>{t('about.fork.item5')}</li>
          <li>{t('about.fork.item6')}</li>
        </ul>

        <h2>{t('about.rewrite.title')}</h2>
        <p>
          {t('about.rewrite.body')}
        </p>

        <h2>{t('about.philosophy.title')}</h2>
        <p>
          {t('about.philosophy.p1')}
        </p>
        <p>
          {t('about.philosophy.beforeLink')}{' '}
          <a href={GITHUB_URL} target="_blank" rel="noreferrer">
            {t('about.philosophy.forkLink')}
          </a>
          .
        </p>
      </main>

      <LandingFooter />
    </div>
  );
};

export default AboutPage;
