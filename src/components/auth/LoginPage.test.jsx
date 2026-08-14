import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import LoginPage from './LoginPage';

const authState = {
  loginWithPassword: vi.fn(),
  isAuthenticated: false,
};
const configState = {
  config: { enable_oidc: false, signup_url: null },
  loading: false,
};

vi.mock('../../contexts/AuthContext', () => ({
  useAuth: () => authState,
}));
vi.mock('../../contexts/ConfigContext', () => ({
  useConfig: () => configState,
}));
vi.mock('../../services/api', async () => {
  const { createMockApiService } = await import('../../test/mockApiService');
  return { default: createMockApiService() };
});

const renderPage = () =>
  render(
    <MemoryRouter initialEntries={['/login']}>
      <LoginPage />
    </MemoryRouter>,
  );

beforeEach(() => {
  configState.config = { enable_oidc: false, signup_url: null };
  configState.loading = false;
});

describe('LoginPage', () => {
  it('renders the password form in self-host mode', () => {
    renderPage();
    expect(screen.getByLabelText('Username')).toBeInTheDocument();
    expect(screen.getByLabelText('Password')).toBeInTheDocument();
    expect(screen.getByText('Continue without account')).toBeInTheDocument();
    expect(screen.queryByText('Sign in with SSO')).not.toBeInTheDocument();
  });

  it('renders SSO-only mode when OIDC is enabled', () => {
    configState.config = { enable_oidc: true, signup_url: null };
    renderPage();
    expect(screen.getByText('Sign in with SSO')).toBeInTheDocument();
    expect(screen.queryByLabelText('Username')).not.toBeInTheDocument();
  });

  it('renders the signup link for an https URL', () => {
    configState.config = {
      enable_oidc: true,
      signup_url: 'https://id.example.com/signup',
    };
    renderPage();
    const link = screen.getByText('Create account');
    expect(link).toHaveAttribute('href', 'https://id.example.com/signup');
  });

  it('never renders a signup link with a non-http(s) scheme', () => {
    configState.config = { enable_oidc: true, signup_url: 'file:///etc/passwd' };
    renderPage();
    expect(screen.queryByText('Create account')).not.toBeInTheDocument();
  });

  it('holds a loading state until config resolves', () => {
    configState.loading = true;
    renderPage();
    expect(screen.getByText('Loading sign-in options…')).toBeInTheDocument();
    expect(screen.queryByLabelText('Username')).not.toBeInTheDocument();
  });
});
