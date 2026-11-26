import { describe, expect, it } from 'vitest';
import {
  downloadEngine,
  hlsDownloadUrl,
  isYoutubePageUrl,
  youtubeDownloadHost,
} from './publicHttpUrl';

describe('youtubeDownloadHost', () => {
  it('accepts YouTube and its CDN by hostname only', () => {
    expect(youtubeDownloadHost('https://www.youtube.com/watch?v=abc')).toBe(true);
    expect(youtubeDownloadHost('https://youtu.be/abc')).toBe(true);
    expect(youtubeDownloadHost('https://rr1---sn.googlevideo.com/videoplayback')).toBe(true);
    expect(youtubeDownloadHost('https://evil.com/youtube.com/watch')).toBe(false);
    expect(youtubeDownloadHost('https://notyoutube.com/watch')).toBe(false);
  });
});

describe('downloadEngine', () => {
  it('mirrors the backend routing', () => {
    expect(downloadEngine('https://www.youtube.com/watch?v=abc')).toBe('yt-dlp');
    expect(downloadEngine('https://cdn.example.com/stream.mpd')).toBe('yt-dlp');
    expect(downloadEngine('https://cdn.example.com/live/index.m3u8')).toBe('HLS');
    expect(downloadEngine('https://cdn.example.com/file.mp4?x=.m3u8')).toBe('HTTP');
    expect(hlsDownloadUrl('https://cdn.example.com/a.m3u8/b')).toBe(true);
  });
});

describe('isYoutubePageUrl', () => {
  it('only matches pages that offer a quality choice', () => {
    expect(isYoutubePageUrl('https://www.youtube.com/watch?v=dQw4w9WgXcQ')).toBe(true);
    expect(isYoutubePageUrl('https://youtube.com/shorts/abc')).toBe(true);
    expect(isYoutubePageUrl('https://youtu.be/dQw4w9WgXcQ')).toBe(true);
    expect(isYoutubePageUrl('https://youtu.be/')).toBe(false);
    expect(isYoutubePageUrl('https://www.youtube.com/feed/library')).toBe(false);
    expect(isYoutubePageUrl('https://rr1---sn.googlevideo.com/videoplayback')).toBe(false);
  });
});
