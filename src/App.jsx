import { useCallback, useEffect, useState } from "react";
import { Routes, Route, Navigate, useNavigate, useLocation } from "react-router-dom";
import LoginPage from "./components/auth/LoginPage";
import NotFound from "./views/NotFound";
import { AuthProvider } from "./contexts/AuthContext";
import { ConfigProvider, useConfig } from "./contexts/ConfigContext";
import { ThemeProvider, useTheme } from "./contexts/ThemeContext";
import { BurnerProvider } from "./contexts/BurnerContext";
import ProtectedRoute from "./components/auth/ProtectedRoute";
import Header from "./components/Header";
import Sidebar from "./components/navigation/Sidebar";
import BottomNav from "./components/navigation/BottomNav";
import HistoryView from "./views/HistoryView";
import HistoryPageView from "./views/HistoryPageView";
import EntryView from "./views/EntryView";
import StatisticsView from "./components/stats/StatisticsView";
import SettingsView from "./views/SettingsView";
import GoalsView from "./views/GoalsView";
import { ToastProvider } from "./components/ui/ToastProvider";
import AchievementsView from "./views/AchievementsView";
import LandingPage from "./views/LandingPage";
import AboutPage from "./views/AboutPage";
import apiService from "./services/api";
import { useMoodData } from "./hooks/useMoodData";
import { useGroups } from "./hooks/useGroups";
import { useStatistics } from "./hooks/useStatistics";
import MusicDock from './components/mood/MusicDock'
import "./App.css";

const MusicDockGate = () => {
  const { config } = useConfig();

  if (!config.enable_mood_music) return null;
  return <MusicDock />;
};

const AppContent = () => {
  const navigate = useNavigate();
  const location = useLocation();
  const { syncFromServer } = useTheme();

  // Pull the account's saved theme once per session; the server copy wins
  // over whatever this browser had locally.
  useEffect(() => {
    let cancelled = false;
    apiService
      .getPreferences()
      .then((prefs) => {
        if (!cancelled && prefs?.theme) syncFromServer(prefs.theme);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [syncFromServer]);
  
  // Custom hooks
  const { pastEntries, setPastEntries, loading: historyLoading, error: historyError, refreshHistory } = useMoodData();
  const [searchResults, setSearchResults] = useState(null);
  const { groups, createGroup, createGroupOption } = useGroups();
  const { statistics, currentStreak, loading: statsLoading, error: statsError, loadStatistics } = useStatistics();

  const handleMoodSelect = (moodValue) => {
    // Absolute path: relative 'entry' resolves against the current nested
    // location (e.g. /dashboard/history -> /dashboard/history/entry), which
    // matches no route and rendered a blank screen.
    navigate('/dashboard/entry', { state: { mood: moodValue } });
  };

  const handleBackToHistory = () => {
    navigate('/dashboard');
  };

  const upsertEntry = (entry) => {
    if (!entry?.id) return;

    setPastEntries((prev) => {
      const existingIndex = prev.findIndex((item) => item.id === entry.id);
      if (existingIndex === -1) {
        return [entry, ...prev];
      }

      return prev.map((item) => (
        item.id === entry.id ? { ...item, ...entry } : item
      ));
    });
  };

  const handleEntryDeleted = (deletedEntryId) => {
    // Remove the deleted entry from the local state
    setPastEntries(prev => prev.filter(entry => entry.id !== deletedEntryId));
  };

  const handleStartEdit = (entry) => {
    navigate('/dashboard/entry', { state: { entry: entry, mood: entry.mood } });
  };

  const handleEditMoodSelect = (moodValue) => {
    navigate('/dashboard/entry', { state: { ...location.state, mood: moodValue }, replace: true });
  };

  const handleEntryUpdated = (updatedEntry, options = {}) => {
    const {
      navigateAfterSave = true,
      refreshAfterSave = true,
    } = options;

    upsertEntry(updatedEntry);

    if (navigateAfterSave) {
      navigate('/dashboard');
    }

    if (refreshAfterSave) {
      refreshHistory();
    }
  };

  // Helper to get state from location
  const locationState = location.state || {};
  const { mood: selectedMood, entry: editingEntry } = locationState;
  
  // Get currently displayed entries (apply search filter if active)
  const displayEntries = searchResults !== null ? searchResults : pastEntries;

  // Determine if we are in entry view for layout purposes (no sidebar)
  const isEntryView = location.pathname === '/dashboard/entry';

  const handleGlobalSearch = useCallback((results) => {
    setSearchResults(results);
    if (results !== null) {
      // Search results live on the History page (Phase 7c moved the full
      // entry list off home), so route there instead of home.
      if (location.pathname !== '/dashboard/history') {
        navigate('/dashboard/history');
      }
      setTimeout(() => {
        const historySection = document.getElementById('history-section');
        if (historySection) {
          // Calculate an offset to prevent header overlap
          const headerOffset = 80;
          const elementPosition = historySection.getBoundingClientRect().top;
          const offsetPosition = elementPosition + window.scrollY - headerOffset;
          
          window.scrollTo({
            top: offsetPosition,
            behavior: 'smooth'
          });
        }
      }, 50); // slight delay to allow rendering if navigating
    }
  }, [location.pathname, navigate]);

  useEffect(() => {
    const handler = () => {
      // The mood picker that actually starts an entry only lives on home
      // (index route). "Add Entry" tiles can now be tapped from other pages
      // too (e.g. the History page's empty state), so route to home itself
      // — not just anywhere under /dashboard — before scrolling up to it.
      const isHome = location.pathname === '/dashboard' || location.pathname === '/dashboard/';
      if (!isHome) {
        navigate('/dashboard');
        return;
      }

      window.scrollTo({ top: 0, behavior: 'smooth' });
    };
    window.addEventListener('nightlio:new-entry', handler);
    return () => window.removeEventListener('nightlio:new-entry', handler);
  }, [location.pathname, navigate]);

  return (
    <>
      <div className={`app-page ${isEntryView ? 'no-sidebar' : ''}`}>
        <Sidebar
          onLoadStatistics={loadStatistics}
        />
        
        <div className="app-shell">
          <Header
            currentStreak={currentStreak}
            pastEntries={pastEntries}
            onSearch={handleGlobalSearch}
            showSearch={!isEntryView}
          />

          <div className="app-layout">

            <main className="app-main">
              <Routes>
                <Route index element={
                  <HistoryView
                    pastEntries={pastEntries}
                    onMoodSelect={handleMoodSelect}
                    onDelete={handleEntryDeleted}
                    onEdit={handleStartEdit}
                  />
                } />
                <Route path="history" element={
                  <HistoryPageView
                    entries={displayEntries}
                    loading={historyLoading}
                    error={historyError}
                    onDelete={handleEntryDeleted}
                    onEdit={handleStartEdit}
                    searchResults={searchResults}
                  />
                } />
                <Route path="entry" element={
                  <EntryView
                    selectedMood={selectedMood}
                    groups={groups}
                    onBack={handleBackToHistory}
                    onEntryDeleted={handleEntryDeleted}
                    onCreateGroup={createGroup}
                    onCreateOption={createGroupOption}
                    editingEntry={editingEntry}
                    onEntryUpdated={handleEntryUpdated}
                    onEditMoodSelect={handleEditMoodSelect}
                    onSelectMood={handleMoodSelect}
                  />
                } />
                <Route path="stats" element={
                  <StatisticsView
                    statistics={statistics}
                    pastEntries={pastEntries}
                    loading={statsLoading}
                    error={statsError}
                  />
                } />
                <Route path="achievements" element={<AchievementsView />} />
                <Route path="goals" element={<GoalsView />} />
                <Route path="settings" element={<SettingsView />} />
                {/* Unmatched /dashboard/... paths bounce home instead of
                    rendering an empty main area. */}
                <Route path="*" element={<Navigate to="/dashboard" replace />} />
              </Routes>
            </main>
          </div>
        </div>
      </div>

      <BottomNav
        onLoadStatistics={loadStatistics}
      />

      <MusicDockGate />
    </>
  );
};

function App() {
  return (
    <ConfigProvider>
      <ThemeProvider>
        <ToastProvider>
          <BurnerProvider>
            <AuthProvider>
              <Routes>
                <Route path="/" element={<LandingPage />} />
                <Route path="/about" element={<AboutPage />} />
                <Route path="/login" element={<LoginPage />} />
                <Route
                  path="/dashboard/*"
                  element={
                    <ProtectedRoute>
                      <AppContent />
                    </ProtectedRoute>
                  }
                />
                <Route path="*" element={<NotFound />} />
              </Routes>
            </AuthProvider>
          </BurnerProvider>
        </ToastProvider>
      </ThemeProvider>
    </ConfigProvider>
  );
}

export default App;