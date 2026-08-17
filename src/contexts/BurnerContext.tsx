import { createContext, useContext, useEffect, useMemo, useState } from 'react';
import type { Dispatch, ReactNode, SetStateAction } from 'react';

export interface BurnerContextValue {
  isBurnerMode: boolean;
  setIsBurnerMode: Dispatch<SetStateAction<boolean>>;
  toggleBurnerMode: () => void;
}

const BurnerContext = createContext<BurnerContextValue>({
  isBurnerMode: false,
  setIsBurnerMode: () => {},
  toggleBurnerMode: () => {},
});

export const BurnerProvider = ({ children }: { children: ReactNode }) => {
  const [isBurnerMode, setIsBurnerMode] = useState<boolean>(() => {
    try {
      return localStorage.getItem('nightlio:burner-mode') === 'on';
    } catch {
      return false;
    }
  });

  useEffect(() => {
    try {
      localStorage.setItem('nightlio:burner-mode', isBurnerMode ? 'on' : 'off');
    } catch {
      // Ignore storage write failures.
    }
  }, [isBurnerMode]);

  const value = useMemo<BurnerContextValue>(
    () => ({
      isBurnerMode,
      setIsBurnerMode,
      toggleBurnerMode: () => setIsBurnerMode((current) => !current),
    }),
    [isBurnerMode]
  );

  return <BurnerContext.Provider value={value}>{children}</BurnerContext.Provider>;
};

export const useBurner = (): BurnerContextValue => useContext(BurnerContext);
