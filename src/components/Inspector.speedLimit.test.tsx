import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import i18n from '../i18n';
import { Inspector } from './Inspector';
import { makeRow } from '../test/fixtures';
import type { DownloadActions } from '../lib/useDownloadActions';

function actionsWithSpeedLimit(speedLimit = vi.fn()) {
  return {
    actions: { speedLimit } as unknown as DownloadActions,
    speedLimit,
  };
}

describe('Inspector per-download speed limit', () => {
  beforeEach(() => {
    void i18n.changeLanguage('en');
  });

  it('commits a positive KB/s value on blur', () => {
    const row = makeRow({ speed_limit_kbps: null });
    const { actions, speedLimit } = actionsWithSpeedLimit();
    render(<Inspector rows={[row]} primary={row} actions={actions} onClose={vi.fn()} />);

    const input = screen.getByLabelText('Speed limit');
    expect(input).toHaveValue(null);
    fireEvent.change(input, { target: { value: '512' } });
    fireEvent.blur(input);

    expect(speedLimit).toHaveBeenCalledTimes(1);
    expect(speedLimit).toHaveBeenCalledWith(row, 512);
  });

  it('commits empty input as zero, meaning the global limit', () => {
    const row = makeRow({ speed_limit_kbps: 384 });
    const { actions, speedLimit } = actionsWithSpeedLimit();
    render(<Inspector rows={[row]} primary={row} actions={actions} onClose={vi.fn()} />);

    const input = screen.getByLabelText('Speed limit');
    expect(input).toHaveValue(384);
    fireEvent.change(input, { target: { value: '' } });
    fireEvent.blur(input);

    expect(speedLimit).toHaveBeenCalledWith(row, 0);
  });

  it('Enter commits once and invalid input restores the last value', () => {
    const row = makeRow({ speed_limit_kbps: 384 });
    const { actions, speedLimit } = actionsWithSpeedLimit();
    render(<Inspector rows={[row]} primary={row} actions={actions} onClose={vi.fn()} />);

    const input = screen.getByLabelText('Speed limit');
    fireEvent.change(input, { target: { value: '512' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    fireEvent.blur(input);
    expect(speedLimit).toHaveBeenCalledTimes(1);
    expect(speedLimit).toHaveBeenCalledWith(row, 512);

    fireEvent.change(input, { target: { value: '1.5' } });
    fireEvent.blur(input);
    expect(speedLimit).toHaveBeenCalledTimes(1);
    expect(input).toHaveValue(512);
  });
});
