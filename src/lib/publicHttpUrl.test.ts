import { describe, expect, it } from 'vitest';
import { isBlockedLiteralHost, isPublicHttpUrl, publicHttpUrlsFromMixedText, publicHttpUrlsFromText, enqueueCookieUrl, isFalconGithubReleaseUrl } from './publicHttpUrl';

describe('isPublicHttpUrl', () => {
  it('accepts public https', () => {
    expect(isPublicHttpUrl('https://example.com/file.zip')).toBe(true);
  });

  it('rejects loopback and private literals', () => {
    expect(isPublicHttpUrl('http://127.0.0.1/secret')).toBe(false);
    expect(isPublicHttpUrl('http://localhost/x')).toBe(false);
    expect(isPublicHttpUrl('http://[::1]/x')).toBe(false);
    expect(isPublicHttpUrl('http://192.168.1.1/a')).toBe(false);
    expect(isPublicHttpUrl('http://10.0.0.2/a')).toBe(false);
    expect(isPublicHttpUrl('http://172.16.0.1/a')).toBe(false);
    expect(isPublicHttpUrl('http://100.64.0.1/a')).toBe(false);
    expect(isPublicHttpUrl('http://[fc00::1]/a')).toBe(false);
    expect(isPublicHttpUrl('http://[fe80::1]/a')).toBe(false);
    expect(isPublicHttpUrl('http://foo.localhost/a')).toBe(false);
    expect(isPublicHttpUrl('http://0.0.0.1/a')).toBe(false);
    expect(isPublicHttpUrl('http://224.0.0.1/a')).toBe(false);
    expect(isPublicHttpUrl('http://[ff02::1]/a')).toBe(false);
    expect(isPublicHttpUrl('http://[::ffff:127.0.0.1]/a')).toBe(false);
    expect(isPublicHttpUrl('http://[::ffff:7f00:1]/a')).toBe(false);
    expect(isPublicHttpUrl('http://127.1/a')).toBe(false);
    expect(isPublicHttpUrl('http://0x7f.0.0.1/a')).toBe(false);
    expect(isPublicHttpUrl('http://2130706433/a')).toBe(false);
    expect(isPublicHttpUrl('http://[::ffff:808:808]/a')).toBe(true);
  });

  it('accepts public IPv4 literals', () => {
    expect(isPublicHttpUrl('https://8.8.8.8/file.bin')).toBe(true);
  });

  it('rejects non-http schemes and junk', () => {
    expect(isPublicHttpUrl('file:///etc/passwd')).toBe(false);
    expect(isPublicHttpUrl('magnet:?xt=urn:btih:abc')).toBe(false);
    expect(isPublicHttpUrl('not-a-url')).toBe(false);
  });

  it('allows signed CDN URLs up to 8192 chars', () => {
    expect(isPublicHttpUrl(`https://rr1---sn-abc.googlevideo.com/videoplayback?${'a'.repeat(3000)}`)).toBe(
      true,
    );
    expect(isPublicHttpUrl(`https://example.com/${'a'.repeat(8200)}`)).toBe(false);
  });
});

describe('publicHttpUrlsFromText', () => {
  it('parses uri-list lines and skips comments/private', () => {
    const urls = publicHttpUrlsFromText(
      '# comment\nhttps://cdn.example.com/a.mp4\nhttp://127.0.0.1/x\nhttps://cdn.example.com/b.zip\nhttps://cdn.example.com/a.mp4\n',
    );
    expect(urls).toEqual(['https://cdn.example.com/a.mp4', 'https://cdn.example.com/b.zip']);
  });
});

describe('publicHttpUrlsFromMixedText', () => {
  it('splits sibling URLs but keeps query commas', () => {
    expect(
      publicHttpUrlsFromMixedText(
        '# skip\nhttps://cdn.example.com/a.mp4,https://cdn.example.com/b.zip',
      ),
    ).toEqual(['https://cdn.example.com/a.mp4', 'https://cdn.example.com/b.zip']);
    expect(publicHttpUrlsFromMixedText('https://cdn.example.com/x?ids=1,2,3')).toEqual([
      'https://cdn.example.com/x?ids=1,2,3',
    ]);
    expect(
      publicHttpUrlsFromMixedText('https://cdn.example.com/a.zip https://cdn.example.com/b.zip'),
    ).toEqual(['https://cdn.example.com/a.zip', 'https://cdn.example.com/b.zip']);
  });
});

describe('isBlockedLiteralHost', () => {
  it('treats empty host as blocked', () => {
    expect(isBlockedLiteralHost('')).toBe(true);
    expect(isBlockedLiteralHost('example.com')).toBe(false);
  });
});

describe('enqueueCookieUrl', () => {
  it('omits cookie_url when cookies are empty', () => {
    expect(enqueueCookieUrl('', '', ['https://cdn.example.com/a.mp4'])).toBeUndefined();
  });

  it('uses one origin for a mixed batch, not each download URL', () => {
    expect(
      enqueueCookieUrl('sid=1', '', [
        'https://cdn.example.com/a.mp4',
        'https://other.example.net/b.zip',
      ]),
    ).toBe('https://cdn.example.com/a.mp4');
    expect(
      enqueueCookieUrl('sid=1', 'https://www.youtube.com/watch?v=a', [
        'https://rr1---sn.googlevideo.com/videoplayback',
      ]),
    ).toBe('https://www.youtube.com/watch?v=a');
  });
});

describe('isFalconGithubReleaseUrl', () => {
  it('accepts this repo release pages only', () => {
    expect(
      isFalconGithubReleaseUrl('https://github.com/batu3384/falcon-dm/releases/tag/v0.2.0'),
    ).toBe(true);
    expect(isFalconGithubReleaseUrl('https://github.com/batu3384/falcon-dm')).toBe(true);
    expect(isFalconGithubReleaseUrl('https://github.com/evil/falcon-dm/releases')).toBe(false);
    expect(isFalconGithubReleaseUrl('https://github.com/batu3384/falcon-dm.evil/x')).toBe(false);
    expect(isFalconGithubReleaseUrl('http://github.com/batu3384/falcon-dm/releases')).toBe(false);
    expect(isFalconGithubReleaseUrl('javascript:alert(1)')).toBe(false);
  });
});
