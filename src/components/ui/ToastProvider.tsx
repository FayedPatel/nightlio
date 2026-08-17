import { createContext, useCallback, useContext, useMemo, useState } from 'react';
import type { ReactNode } from 'react';

export type ToastType = 'info' | 'success' | 'error';

interface Toast {
  id: string;
  message: string;
  type: ToastType;
}

interface ToastContextValue {
  /** Interface declared from the implementation below; the default value is
      a no-op stub (it also predates the `duration` parameter). */
  show: (message: string, type?: ToastType, duration?: number) => void;
}

const ToastContext = createContext<ToastContextValue>({
  show: () => {},
});

export const ToastProvider = ({ children }: { children?: ReactNode }) => {
  const [toasts, setToasts] = useState<Toast[]>([]);

  const remove = useCallback((id: string) => {
    setToasts((t) => t.filter((x) => x.id !== id));
  }, []);

  const show = useCallback((message: string, type: ToastType = 'info', duration = 3000) => {
    const id = Math.random().toString(36).slice(2);
    setToasts((t) => [...t, { id, message, type }]);
    if (duration > 0) setTimeout(() => remove(id), duration);
  }, [remove]);

  const value = useMemo(() => ({ show }), [show]);

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toast-container" aria-live="polite" aria-atomic="true">
        {toasts.map((t) => (
          // A real <button>: click-to-dismiss now works from the keyboard and
          // is announced as interactive. CSS below resets UA button styling.
          <button key={t.id} type="button" className={`toast toast--${t.type}`} onClick={() => remove(t.id)}>
            {t.message}
          </button>
        ))}
      </div>
      <style>
        {`
        .toast-container {
          position: fixed;
          left: 50%;
          transform: translateX(-50%);
          bottom: 90px;
          display: flex;
          flex-direction: column;
          gap: 8px;
          /* Above the modal/sheet layer (var(--z-modal): 1000) on purpose —
             an error toast must stay legible even while a modal is open. */
          z-index: var(--z-toast);
        }
        .toast {
          padding: 10px 14px;
          border-radius: 10px;
          background: var(--bg-card);
          box-shadow: var(--shadow-2);
          border: 1px solid var(--border);
          color: var(--fg-strong);
          cursor: pointer;
          min-width: 220px;
          text-align: center;
          /* Neutralize UA <button> styling so the toast renders exactly as
             the old <div> did. */
          font: inherit;
          appearance: none;
        }
  .toast--success { border-color: color-mix(in oklab, var(--success), transparent 60%); }
  .toast--error { border-color: color-mix(in oklab, var(--danger), transparent 60%); }
  .toast--info { border-color: color-mix(in oklab, var(--accent-600), transparent 60%); }
        `}
      </style>
    </ToastContext.Provider>
  );
};

export const useToast = () => useContext(ToastContext);
