import { describe, expect, it } from 'vitest';
import { hlsDownloadUrl, youtubeDownloadHost } from './InspectorPanel';

describe('youtubeDownloadHost', () => {
  it('accepts YouTube hosts only', () => {
    expect(youtubeDownloadHost('https://www.youtube.com/watch?v=a')).toBe(true);
    expect(youtubeDownloadHost('https://youtu.be/a')).toBe(true);
    expect(
      youtubeDownloadHost('https://rr3---sn-abc.googlevideo.com/videoplayback?id=1'),
    ).toBe(true);
  });

  it('rejects path spoof', () => {
    expect(youtubeDownloadHost('https://evil.com/youtube.com')).toBe(false);
    expect(youtubeDownloadHost('https://evil-youtube.com/')).toBe(false);
    expect(youtubeDownloadHost('https://evil-googlevideo.com/videoplayback')).toBe(false);
  });
});

describe('hlsDownloadUrl', () => {
  it('detects m3u8 path', () => {
    expect(hlsDownloadUrl('https://cdn.example.com/live.m3u8')).toBe(true);
    expect(hlsDownloadUrl('https://cdn.example.com/video.mp4')).toBe(false);
    expect(hlsDownloadUrl('https://cdn.example.com/video.mp4?x=.m3u8')).toBe(false);
  });
});
