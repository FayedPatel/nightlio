import { Search } from 'lucide-react';
import { useI18n } from '../../i18n';

const SearchPlaceholder = () => {
  const { t } = useI18n();
  return (
    <div
      className="search-placeholder"
      role="search"
      aria-label={t('search.aria')}
      title={t('search.titleHint')}
      id="global-search"
    >
      <Search size={16} strokeWidth={2} aria-hidden="true" />
      <input
        className="search-placeholder__input"
        type="text"
        readOnly
        placeholder={t('search.placeholderReadOnly')}
        aria-readonly="true"
        id="global-search-input"
      />
      <kbd className="search-placeholder__hint">/</kbd>
    </div>
  );
};

export default SearchPlaceholder;
