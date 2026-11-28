import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { extractTauriError, localizeDownloadError, setDownloadSpeedLimit } from './commands';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const t = (key: string, options?: Record<string, unknown>) =>
  options?.code ? `${key}:${options.code}` : key;

describe('setDownloadSpeedLimit', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset().mockResolvedValue(undefined);
  });

  it('invokes the backend row mutation with a camelCase IPC payload', async () => {
    await setDownloadSpeedLimit(7, 512);
    expect(invoke).toHaveBeenCalledWith('set_download_speed_limit', {
      id: 7,
      speedLimitKbps: 512,
    });
  });
});

describe('extractTauriError', () => {
  it('maps missing invoke to a human message', () => {
    document.documentElement.lang = 'en';
    expect(
      extractTauriError(new TypeError("Cannot read properties of undefined (reading 'invoke')")),
    ).toBe('Falcon DM engine is not connected');
  });

  it('keeps normal backend strings', () => {
    expect(extractTauriError('invalid url')).toBe('invalid url');
  });
});

describe('localizeDownloadError', () => {
  it('maps engine codes to messages', () => {
    expect(localizeDownloadError('ERR_UNSUPPORTED_MAGNET', t)).toBe('errors.unsupported_magnet');
    expect(localizeDownloadError('invalid url', t)).toBe('errors.invalid_url');
    expect(localizeDownloadError('not a real media url', t)).toBe('errors.junk_media_url');
    expect(localizeDownloadError('ERR_DISK_FULL', t)).toBe('errors.disk_full');
    expect(localizeDownloadError('ERR_YTDLP_MISSING', t)).toBe('errors.ytdlp_missing');
    expect(localizeDownloadError('ERR_YTDLP_PATH_INVALID: /bad/path', t)).toBe(
      'errors.ytdlp_path_invalid',
    );
    expect(localizeDownloadError('ERR_HLS_LIVE', t)).toBe('errors.hls_live');
    expect(localizeDownloadError('ERR_HLS_DRM', t)).toBe('errors.hls_drm');
    expect(localizeDownloadError('ERR_DASH_LIVE', t)).toBe('errors.dash_live');
    expect(localizeDownloadError('ERR_DASH_DRM', t)).toBe('errors.dash_drm');
    expect(localizeDownloadError('ERR_PARTIAL_RESPONSE', t)).toBe('errors.partial_response');
    expect(localizeDownloadError('ERR_CONTENT_ENCODING', t)).toBe('errors.content_encoding');
    expect(localizeDownloadError('ERR_VIDEO_UNAVAILABLE', t)).toBe('errors.video_unavailable');
    expect(localizeDownloadError('ERR_VIDEO_PRIVATE', t)).toBe('errors.video_private');
    expect(localizeDownloadError('ERR_LOGIN_REQUIRED', t)).toBe('errors.login_required');
    expect(localizeDownloadError('ERR_CHECKSUM_MISMATCH', t)).toBe('errors.checksum_mismatch');
    expect(localizeDownloadError('ERR_CHECKSUM_INVALID', t)).toBe('errors.checksum_invalid');
  });

  it('groups HTTP status codes by what the user can do', () => {
    expect(localizeDownloadError('HTTP 403', t)).toBe('errors.http_forbidden:403');
    expect(localizeDownloadError('HTTP 404', t)).toBe('errors.http_not_found:404');
    expect(localizeDownloadError('HTTP 429', t)).toBe('errors.http_rate_limited:429');
    expect(localizeDownloadError('HTTP 503', t)).toBe('errors.http_server:503');
    expect(localizeDownloadError('HTTP 418', t)).toBe('errors.http_other:418');
  });

  it('strips the yt-dlp prefix and keeps unknown text', () => {
    expect(localizeDownloadError('yt-dlp: Private video', t)).toBe('Private video');
    expect(localizeDownloadError('something odd', t)).toBe('something odd');
  });
});
