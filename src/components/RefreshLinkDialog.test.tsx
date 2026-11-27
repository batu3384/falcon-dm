import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import i18n from '../i18n';
import { RefreshLinkDialog } from './RefreshLinkDialog';
import { refreshDownloadUrl } from '../api/commands';
import { makeRow } from '../test/fixtures';
import type * as Commands from '../api/commands';

vi.mock('../api/commands', async (importOriginal) => {
  const actual = await importOriginal<typeof Commands>();
  return { ...actual, refreshDownloadUrl: vi.fn() };
});

describe('RefreshLinkDialog', () => {
  beforeEach(() => {
    void i18n.changeLanguage('en');
    vi.mocked(refreshDownloadUrl).mockReset().mockResolvedValue(undefined);
  });

  it('sends the new link for the row and closes on success', async () => {
    const row = makeRow({ status: 'Failed' });
    const onClose = vi.fn();
    render(<RefreshLinkDialog row={row} onClose={onClose} />);
    await userEvent.type(
      screen.getByLabelText('New link'),
      'https://cdn.example.com/fresh.bin?sig=new',
    );
    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await waitFor(() =>
      expect(refreshDownloadUrl).toHaveBeenCalledWith(
        row.id,
        'https://cdn.example.com/fresh.bin?sig=new',
      ),
    );
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it('keeps the submit disabled without a link', () => {
    render(<RefreshLinkDialog row={makeRow({ status: 'Failed' })} onClose={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'Refresh' })).toBeDisabled();
  });

  it('shows the engine error inline and stays open', async () => {
    vi.mocked(refreshDownloadUrl).mockRejectedValueOnce('invalid url');
    const onClose = vi.fn();
    render(<RefreshLinkDialog row={makeRow({ status: 'Failed' })} onClose={onClose} />);
    await userEvent.type(screen.getByLabelText('New link'), 'https://cdn.example.com/x.bin');
    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('valid web link');
    expect(onClose).not.toHaveBeenCalled();
  });
});
