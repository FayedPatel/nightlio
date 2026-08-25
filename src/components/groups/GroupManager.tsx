import { useState } from 'react';
import { Settings, X } from 'lucide-react';
import { useI18n } from '../../i18n';
import type { Group } from '../../types/api';

interface GroupManagerProps {
  groups: Group[];
  onCreateGroup: (name: string) => Promise<boolean>;
  onCreateOption: (groupId: number, name: string) => Promise<boolean>;
}

const GroupManager = ({ groups, onCreateGroup, onCreateOption }: GroupManagerProps) => {
  const { t } = useI18n();
  const [showManager, setShowManager] = useState(false);
  const [newGroupName, setNewGroupName] = useState('');
  const [newOptionName, setNewOptionName] = useState('');
  const [selectedGroupForOption, setSelectedGroupForOption] = useState('');
  const [isCreatingGroup, setIsCreatingGroup] = useState(false);
  const [isCreatingOption, setIsCreatingOption] = useState(false);

  const handleCreateGroup = async () => {
    if (!newGroupName.trim()) return;

    setIsCreatingGroup(true);
    try {
      const success = await onCreateGroup(newGroupName.trim());
      if (success) {
        setNewGroupName('');
      }
    } finally {
      setIsCreatingGroup(false);
    }
  };

  const handleCreateOption = async () => {
    if (!newOptionName.trim() || !selectedGroupForOption) return;

    setIsCreatingOption(true);
    try {
      // The <select> value is a string; the onCreateOption contract
      // (useGroups.createGroupOption) takes a numeric group id, which is only
      // ever interpolated into the request URL, so the conversion is inert.
      const success = await onCreateOption(Number(selectedGroupForOption), newOptionName.trim());
      if (success) {
        setNewOptionName('');
        setSelectedGroupForOption('');
      }
    } finally {
      setIsCreatingOption(false);
    }
  };

  if (!showManager) {
    return (
      <div style={{ textAlign: 'center', marginTop: '1rem' }}>
  <button
          onClick={() => setShowManager(true)}
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: '0.5rem',
            padding: '0.75rem 1.5rem',
            background: 'linear-gradient(135deg, var(--accent-bg), var(--accent-bg-2))',
            color: 'white',
            border: 'none',
            borderRadius: '25px',
            cursor: 'pointer',
            fontSize: '0.9rem',
            fontWeight: '500',
            margin: '0 auto',
            transition: 'all 0.3s ease',
            boxShadow: 'var(--shadow-md)',
          }}
        >
          <Settings size={16} />
          {t('groups.manageCategories')}
        </button>
      </div>
    );
  }

  return (
  <div
      style={{
    background: 'var(--bg-card)',
        borderRadius: '16px',
        padding: '1.5rem',
    boxShadow: 'var(--shadow-lg)',
        marginTop: '1rem',
      }}
    >
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          marginBottom: '1.5rem',
        }}
      >
        <h3
          style={{
            margin: '0',
            color: 'var(--text)',
            fontSize: '1.2rem',
            fontWeight: '600',
          }}
        >
          {t('groups.manageCategories')}
        </h3>
        <button
          onClick={() => setShowManager(false)}
          style={{
            background: 'none',
            border: 'none',
            cursor: 'pointer',
            color: 'var(--text-muted)',
            padding: '0.25rem',
          }}
        >
          <X size={20} />
        </button>
      </div>

      {/* Create New Group */}
      <div style={{ marginBottom: '2rem' }}>
            <h4 style={{ margin: '0 0 1rem 0', color: 'var(--text)', opacity: 0.9, fontSize: '1rem' }}>
          {t('groups.createNew')}
        </h4>
        <div style={{ display: 'flex', gap: '0.5rem', alignItems: 'center' }}>
          <input
            type="text"
            placeholder={t('groups.categoryNamePlaceholder')}
            value={newGroupName}
            onChange={(e) => setNewGroupName(e.target.value)}
            onKeyPress={(e) => e.key === 'Enter' && handleCreateGroup()}
            style={{
              flex: 1,
              padding: '0.75rem',
              border: '1px solid var(--border)',
              borderRadius: '8px',
              fontSize: '0.9rem',
            }}
          />
      <button
            onClick={handleCreateGroup}
            disabled={!newGroupName.trim() || isCreatingGroup}
            style={{
              padding: '0.75rem 1rem',
  background: 'linear-gradient(135deg, var(--accent-bg), var(--accent-bg-2))',
              color: 'white',
              border: 'none',
              borderRadius: '8px',
              cursor: !newGroupName.trim() || isCreatingGroup ? 'not-allowed' : 'pointer',
              fontSize: '0.9rem',
              fontWeight: '500',
              opacity: !newGroupName.trim() || isCreatingGroup ? 0.6 : 1,
            }}
          >
            {isCreatingGroup ? t('groups.creating') : t('groups.create')}
          </button>
        </div>
      </div>

      {/* Create New Option */}
      {groups.length > 0 && (
        <div style={{ marginBottom: '2rem' }}>
          <h4 style={{ margin: '0 0 1rem 0', color: 'var(--text)', opacity: 0.9, fontSize: '1rem' }}>
            {t('groups.addOption')}
          </h4>
          <div
            style={{
              display: 'flex',
              gap: '0.5rem',
              alignItems: 'center',
              flexWrap: 'wrap',
            }}
          >
      <select
              value={selectedGroupForOption}
              onChange={(e) => setSelectedGroupForOption(e.target.value)}
              style={{
                padding: '0.75rem',
        border: '1px solid var(--border)',
                borderRadius: '8px',
                fontSize: '0.9rem',
                minWidth: '150px',
              }}
            >
              <option value="">{t('groups.selectCategory')}</option>
              {groups.map(group => (
                <option key={group.id} value={group.id}>
                  {group.name}
                </option>
              ))}
            </select>
      <input
              type="text"
              placeholder={t('groups.optionNamePlaceholder')}
              value={newOptionName}
              onChange={(e) => setNewOptionName(e.target.value)}
              onKeyPress={(e) => e.key === 'Enter' && handleCreateOption()}
              style={{
                flex: 1,
                minWidth: '200px',
                padding: '0.75rem',
        border: '1px solid var(--border)',
                borderRadius: '8px',
                fontSize: '0.9rem',
              }}
            />
      <button
              onClick={handleCreateOption}
              disabled={!newOptionName.trim() || !selectedGroupForOption || isCreatingOption}
              style={{
                padding: '0.75rem 1rem',
  background: 'linear-gradient(135deg, var(--accent-bg), var(--accent-bg-2))',
                color: 'white',
                border: 'none',
                borderRadius: '8px',
                cursor: (!newOptionName.trim() || !selectedGroupForOption || isCreatingOption) ? 'not-allowed' : 'pointer',
                fontSize: '0.9rem',
                fontWeight: '500',
                opacity: (!newOptionName.trim() || !selectedGroupForOption || isCreatingOption) ? 0.6 : 1,
              }}
            >
              {isCreatingOption ? t('groups.adding') : t('groups.add')}
            </button>
          </div>
        </div>
      )}

      {/* Current Groups Overview */}
      {groups.length > 0 && (
        <div>
          <h4 style={{ margin: '0 0 1rem 0', color: 'var(--text)', opacity: 0.9, fontSize: '1rem' }}>
            {t('groups.current')}
          </h4>
          <div style={{ display: 'flex', flexDirection: 'column', gap: '0.75rem' }}>
            {groups.map(group => (
              <div
                key={group.id}
                style={{
                  padding: '1rem',
                  background: 'var(--surface)',
                  borderRadius: '8px',
                  border: '1px solid var(--border)',
                }}
              >
                <div
                  style={{
                    fontWeight: '600',
                    color: 'var(--text)',
                    marginBottom: '0.5rem',
                  }}
                >
                  {t('groups.optionsCount', { name: group.name, count: group.options.length })}
                </div>
                <div
                  style={{
                    display: 'flex',
                    flexWrap: 'wrap',
                    gap: '0.25rem',
                  }}
                >
                  {group.options.map(option => (
                    <span
                      key={option.id}
                      style={{
                        padding: '0.25rem 0.5rem',
                        background: 'var(--bg-card)',
                        border: '1px solid var(--border)',
                        borderRadius: '12px',
                        fontSize: '0.8rem',
                        color: 'var(--text-muted)',
                      }}
                    >
                      {option.name}
                    </span>
                  ))}
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
};

export default GroupManager;
