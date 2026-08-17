import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter, Routes, Route } from 'react-router-dom';
import ProtectedRoute from './ProtectedRoute';

const authState: { loading: boolean; isAuthenticated: boolean; user: { id: number } | null } = {
  loading: false,
  isAuthenticated: false,
  user: null,
};

vi.mock('../../contexts/AuthContext', () => ({
  useAuth: () => authState,
}));

const renderAt = (path: string) =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/login" element={<div>login screen</div>} />
        <Route
          path="/dashboard"
          element={
            <ProtectedRoute>
              <div>dashboard content</div>
            </ProtectedRoute>
          }
        />
      </Routes>
    </MemoryRouter>,
  );

describe('ProtectedRoute', () => {
  it('redirects unauthenticated visitors to /login', () => {
    Object.assign(authState, { loading: false, isAuthenticated: false });
    renderAt('/dashboard');
    expect(screen.getByText('login screen')).toBeInTheDocument();
    expect(screen.queryByText('dashboard content')).not.toBeInTheDocument();
  });

  it('renders children when authenticated', () => {
    Object.assign(authState, { loading: false, isAuthenticated: true, user: { id: 1 } });
    renderAt('/dashboard');
    expect(screen.getByText('dashboard content')).toBeInTheDocument();
  });
});
