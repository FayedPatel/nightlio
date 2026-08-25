import { useState, useEffect } from 'react';
import apiService from '../services/api';
import { useI18n } from '../i18n';
import type { Group } from '../types/api';

export const useGroups = () => {
  const { t } = useI18n();
  const [groups, setGroups] = useState<Group[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadGroups = async () => {
    setLoading(true);
    setError(null);

    try {
      const data = await apiService.getGroups();
      setGroups(data);
    } catch (error) {
      console.error('Failed to load groups:', error);
      setError(t('errors.loadCategories'));
    } finally {
      setLoading(false);
    }
  };

  const createGroup = async (name: string): Promise<boolean> => {
    try {
      // The API expects an object body; a bare string serialized to JSON
      // ("name") made request.json a string server-side and creation 500'd.
      await apiService.createGroup({ name });
      await loadGroups(); // Refresh the list
      return true;
    } catch (error) {
      console.error('Failed to create group:', error);
      setError(t('errors.createCategory'));
      return false;
    }
  };

  const createGroupOption = async (groupId: number, name: string): Promise<boolean> => {
    try {
      await apiService.createGroupOption(groupId, { name });
      await loadGroups(); // Refresh the list
      return true;
    } catch (error) {
      console.error('Failed to create group option:', error);
      setError(t('errors.createOption'));
      return false;
    }
  };

  const deleteGroup = async (groupId: number): Promise<boolean> => {
    try {
      await apiService.deleteGroup(groupId);
      await loadGroups(); // Refresh the list
      return true;
    } catch (error) {
      console.error('Failed to delete group:', error);
      setError(t('errors.deleteCategory'));
      return false;
    }
  };

  useEffect(() => {
    loadGroups();
  }, []);

  return {
    groups,
    loading,
    error,
    createGroup,
    createGroupOption,
    deleteGroup,
    refreshGroups: loadGroups,
  };
};