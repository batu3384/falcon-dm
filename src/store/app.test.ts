import { describe, it, expect, beforeEach, vi } from 'vitest';
import { useAppStore } from './app';
import { getExtensionStatus } from '../api/commands';
import type { PairRequest } from '../types';

vi.mock('../api/commands', () => ({ getExtensionStatus: vi.fn() }));
const mockedGetExtensionStatus = vi.mocked(getExtensionStatus);

function pairRequest(idLetter: string, code: string): PairRequest {
  return { extension_id: idLetter.repeat(32), code };
}

function extensionStatus(pending: PairRequest[]) {
  return { approved_extension_ids: [], pending };
}

const flushAsync = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

describe('useAppStore pair prompts', () => {
  beforeEach(() => {
    mockedGetExtensionStatus.mockReset();
    useAppStore.setState({
      stats: null,
      statsError: null,
      extension: null,
      schedule: null,
      pairPrompt: null,
      dismissedPairKeys: new Set(),
    });
  });

  it('does not reopen the same request after Escape hides it', async () => {
    const pending = pairRequest('a', '123456');
    mockedGetExtensionStatus.mockResolvedValue(extensionStatus([pending]));

    useAppStore.getState().showPairPrompt(pending);
    expect(useAppStore.getState().pairPrompt).toEqual(pending);
    useAppStore.getState().dismissPairPrompt();
    await flushAsync();

    expect(useAppStore.getState().pairPrompt).toBeNull();
    useAppStore.getState().showPairPrompt(pending);
    expect(useAppStore.getState().pairPrompt).toBeNull();
  });

  it('shows a new pairing code from the same extension', async () => {
    const first = pairRequest('b', '123456');
    const second = { ...first, code: '654321' };
    mockedGetExtensionStatus.mockResolvedValue(extensionStatus([first]));

    useAppStore.getState().showPairPrompt(first);
    useAppStore.getState().dismissPairPrompt();
    await flushAsync();

    mockedGetExtensionStatus.mockResolvedValue(extensionStatus([second]));
    await useAppStore.getState().refreshExtension();
    expect(useAppStore.getState().pairPrompt).toEqual(second);
  });
});
