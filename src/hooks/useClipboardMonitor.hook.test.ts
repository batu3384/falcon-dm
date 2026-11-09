import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useClipboardMonitor } from './useClipboardMonitor';
import * as api from '../api/commands';

vi.mock('../api/commands', () => ({
  readClipboard: vi.fn(),
}));

const mockedReadClipboard = vi.mocked(api.readClipboard);

describe('useClipboardMonitor polling', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockedReadClipboard.mockReset();
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      value: {},
      configurable: true,
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
  });

  it('uses readClipboard in Tauri and opens queueable URLs once', async () => {
    mockedReadClipboard.mockResolvedValue('https://cdn.example.com/file.zip');
    const onUrl = vi.fn();
    renderHook(() => useClipboardMonitor(true, onUrl));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });
    expect(mockedReadClipboard).toHaveBeenCalled();
    expect(onUrl).toHaveBeenCalledTimes(1);
    expect(onUrl).toHaveBeenCalledWith('https://cdn.example.com/file.zip');

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });
    expect(onUrl).toHaveBeenCalledTimes(1);
  });

  it('notifies onError once when read fails', async () => {
    mockedReadClipboard.mockRejectedValue(new Error('clipboard read failed'));
    const onError = vi.fn();
    renderHook(() => useClipboardMonitor(true, vi.fn(), onError));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });
    expect(onError).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });
    expect(onError).toHaveBeenCalledTimes(1);
  });

  it('keeps polling same URL when onUrl returns false', async () => {
    mockedReadClipboard.mockResolvedValue('https://cdn.example.com/file.zip');
    const onUrl = vi.fn(() => false);
    renderHook(() => useClipboardMonitor(true, onUrl));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });
    expect(onUrl).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });
    expect(onUrl).toHaveBeenCalledTimes(2);
  });
});
