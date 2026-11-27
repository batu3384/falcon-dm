import { describe, it, expect, beforeEach, vi } from 'vitest';
import { useToastStore } from './toast';

// ponytail: toast store is the global notification channel — verify add, auto-
// dismiss, and manual dismiss so a regression here can't silently swallow
// user-facing error messages.

describe('useToastStore', () => {
  beforeEach(() => {
    useToastStore.setState({ toasts: [] });
    vi.useFakeTimers();
  });

  it('adds a toast', () => {
    useToastStore.getState().showToast('error', 'boom');
    expect(useToastStore.getState().toasts).toHaveLength(1);
    expect(useToastStore.getState().toasts[0].msg).toBe('boom');
  });

  it('auto-dismisses after the timeout', () => {
    useToastStore.getState().showToast('info', 'hi');
    expect(useToastStore.getState().toasts).toHaveLength(1);
    vi.advanceTimersByTime(3500);
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });

  it('manual dismiss removes a specific toast', () => {
    useToastStore.getState().showToast('success', 'ok');
    const id = useToastStore.getState().toasts[0].id;
    useToastStore.getState().dismiss(id);
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });

  it('keeps an optional clickable action with the toast', () => {
    const run = vi.fn();
    useToastStore.getState().showToast('info', 'update', { label: 'Download', run });
    const action = useToastStore.getState().toasts[0].action;
    expect(action?.label).toBe('Download');
    action?.run();
    expect(run).toHaveBeenCalledOnce();
  });

  it('folds identical visible toasts into one with a count', () => {
    const { showToast } = useToastStore.getState();
    for (let i = 0; i < 8; i += 1) showToast('error', 'boom');
    const { toasts } = useToastStore.getState();
    expect(toasts).toHaveLength(1);
    expect(toasts[0].count).toBe(8);
  });

  it('does not dedupe different messages or kinds', () => {
    const { showToast } = useToastStore.getState();
    showToast('error', 'boom');
    showToast('info', 'boom');
    showToast('error', 'other');
    expect(useToastStore.getState().toasts).toHaveLength(3);
  });

  it('re-arms the dismissal timer when a duplicate folds in', () => {
    const { showToast } = useToastStore.getState();
    showToast('error', 'boom');
    vi.advanceTimersByTime(3000);
    showToast('error', 'boom');
    vi.advanceTimersByTime(3000);
    // 6s after the first push but only 3s since the dupe — still visible.
    expect(useToastStore.getState().toasts).toHaveLength(1);
    vi.advanceTimersByTime(500);
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });

  it('stacks a repeat once the original has dismissed itself', () => {
    const { showToast } = useToastStore.getState();
    showToast('error', 'boom');
    vi.advanceTimersByTime(3500);
    showToast('error', 'boom');
    const { toasts } = useToastStore.getState();
    expect(toasts).toHaveLength(1);
    expect(toasts[0].count).toBe(1);
  });
});
