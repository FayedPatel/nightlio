import { useState, useEffect } from 'react';
import { Search, X } from 'lucide-react';
import { useI18n } from '../../i18n';
import type { MoodEntry } from '../../types/api';
import './SearchBar.css';

// Stable default (a default-parameter array literal would be a fresh
// identity every render and re-trigger the search effect).
const DEFAULT_SEARCH_FIELDS: readonly (keyof MoodEntry)[] = ['content', 'date'];
const EMPTY_ENTRIES: readonly MoodEntry[] = [];

interface SearchBarProps {
  entries?: readonly MoodEntry[];
  onSearch?: (results: MoodEntry[] | null) => void;
  placeholder?: string;
  /** Which fields to search. */
  searchFields?: readonly (keyof MoodEntry)[];
}

export default function SearchBar({
  entries = EMPTY_ENTRIES,
  onSearch,
  placeholder,
  searchFields = DEFAULT_SEARCH_FIELDS
}: SearchBarProps) {
  const { t } = useI18n();
  const [query, setQuery] = useState('');
  const effectivePlaceholder = placeholder ?? t('search.entriesPlaceholder');

  // Real-time search as user types; results are delivered to the parent via
  // onSearch rather than rendered here (the inline dropdown was removed).
  useEffect(() => {
    if (!query.trim()) {
      if (onSearch) onSearch(null); // Return null when search is cleared
      return;
    }

    const queryLower = query.toLowerCase();

    // Filter entries by search query
    const filtered = entries.filter(entry => {
      // Search in specified fields
      return searchFields.some(field => {
        const value = entry[field];
        if (!value) return false;
        return String(value).toLowerCase().includes(queryLower);
      });
    });

    // Callback to parent component
    if (onSearch) {
      onSearch(filtered);
    }
  }, [query, entries, searchFields, onSearch]);

  return (
    <div className="search-bar-container">
      <div className="search-bar-input-wrapper">
        <Search size={18} className="search-bar-icon" />
        <input
          type="text"
          placeholder={effectivePlaceholder}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="search-bar-input"
        />
        {query && (
          <button
            className="search-bar-clear"
            onClick={() => setQuery('')}
          >
            <X size={18} />
          </button>
        )}
      </div>
    </div>
  );
}
