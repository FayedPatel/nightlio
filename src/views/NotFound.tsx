import { Link } from 'react-router-dom';
import { useI18n } from '../i18n';

const NotFound = () => {
  const { t } = useI18n();
  return (
    <div style={{
      display: 'flex',
      flexDirection: 'column',
      alignItems: 'center',
      justifyContent: 'center',
      height: '100vh',
      textAlign: 'center',
      padding: '2rem',
      color: 'var(--text)'
    }}>
      <h1 style={{ fontSize: '4rem', marginBottom: '1rem' }}>{t('errors.notFoundCode')}</h1>
      <p style={{ fontSize: '1.5rem', marginBottom: '2rem' }}>{t('errors.notFound')}</p>
      <Link to="/" style={{
        padding: '0.75rem 1.5rem',
        backgroundColor: 'var(--primary)',
        color: 'white',
        textDecoration: 'none',
        borderRadius: 'var(--radius)',
        fontWeight: '500'
      }}>
        {t('errors.goHome')}
      </Link>
    </div>
  );
};

export default NotFound;
