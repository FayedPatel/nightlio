import { describe, expect, it, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import MoodPicker from './MoodPicker';
import { ConfigProvider } from '../../contexts/ConfigContext';

vi.mock('../../services/api', async () => {
  const { createMockApiService } = await import('../../test/mockApiService');
  return { default: createMockApiService() };
});

const renderPicker = (onMoodSelect) =>
  render(
    <ConfigProvider>
      <MoodPicker onMoodSelect={onMoodSelect} />
    </ConfigProvider>,
  );

describe('MoodPicker', () => {
  it('renders all five moods', () => {
    renderPicker(vi.fn());
    for (const label of ['Terrible', 'Bad', 'Okay', 'Good', 'Amazing']) {
      expect(screen.getByTitle(label)).toBeInTheDocument();
    }
  });

  it('reports the picked mood value', () => {
    const onMoodSelect = vi.fn();
    renderPicker(onMoodSelect);
    fireEvent.click(screen.getByTitle('Good'));
    expect(onMoodSelect).toHaveBeenCalledWith(4);
  });
});
