import HistoryList from '../components/history/HistoryList';

// Dedicated History route (added Phase 7c). The full entry list used to be
// embedded straight into the home dashboard below Goals; home now shows
// only today's entry state, so History gets promoted to its own page with a
// nav slot (Sidebar + BottomNav) instead of being reachable only from home.
const HistoryPageView = ({ entries, loading, error, onDelete, onEdit, searchResults }) => (
  <section aria-label="History entries" id="history-section">
    <h1 style={{ margin: '0 0 var(--space-3) 0', paddingLeft: 'calc(var(--space-1) / 2)', color: 'var(--text)' }}>
      {searchResults !== null ? `Search Results (${searchResults.length})` : 'History'}
    </h1>
    <HistoryList
      entries={entries}
      loading={loading}
      error={error}
      onDelete={onDelete}
      onEdit={onEdit}
    />
  </section>
);

export default HistoryPageView;
