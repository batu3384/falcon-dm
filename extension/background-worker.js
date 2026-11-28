/* Bundled Chromium service worker; edit the ordered source files instead. */
/* Shared media URL analysis — loaded in background + content scripts */
(function (root) {
  // Firefox exposes promise-based browser.*; Chrome uses promise-capable chrome.* in MV3.
  if (!root.ext) {
    root.ext =
      typeof browser !== 'undefined' ? browser : typeof chrome !== 'undefined' ? chrome : undefined;
  }

  function _i18n(key, fb) {
    try {
      if (typeof chrome !== 'undefined' && chrome.i18n && chrome.i18n.getMessage) {
        return chrome.i18n.getMessage(key) || fb;
      }
    } catch (_) {}
    return fb;
  }

  const YT_ITAG = {
    // Progressive (video+audio muxed) — preferred for direct download
    18: { q: '360p', fmt: 'MP4', type: 'video', muxed: true },
    22: { q: '720p', fmt: 'MP4', type: 'video', muxed: true },
    37: { q: '1080p', fmt: 'MP4', type: 'video', muxed: true },
    38: { q: '3072p', fmt: 'MP4', type: 'video', muxed: true },
    43: { q: '360p', fmt: 'WebM', type: 'video', muxed: true },
    44: { q: '480p', fmt: 'WebM', type: 'video', muxed: true },
    45: { q: '720p', fmt: 'WebM', type: 'video', muxed: true },
    46: { q: '1080p', fmt: 'WebM', type: 'video', muxed: true },
    // Adaptive video-only
    133: { q: '240p', fmt: 'MP4', type: 'video', muxed: false },
    134: { q: '360p', fmt: 'MP4', type: 'video', muxed: false },
    135: { q: '480p', fmt: 'MP4', type: 'video', muxed: false },
    136: { q: '720p', fmt: 'MP4', type: 'video', muxed: false },
    137: { q: '1080p', fmt: 'MP4', type: 'video', muxed: false },
    138: { q: '2160p', fmt: 'MP4', type: 'video', muxed: false },
    160: { q: '144p', fmt: 'MP4', type: 'video', muxed: false },
    242: { q: '240p', fmt: 'WebM', type: 'video', muxed: false },
    243: { q: '360p', fmt: 'WebM', type: 'video', muxed: false },
    244: { q: '480p', fmt: 'WebM', type: 'video', muxed: false },
    247: { q: '720p', fmt: 'WebM', type: 'video', muxed: false },
    248: { q: '1080p', fmt: 'WebM', type: 'video', muxed: false },
    271: { q: '1440p', fmt: 'WebM', type: 'video', muxed: false },
    272: { q: '2160p', fmt: 'WebM', type: 'video', muxed: false },
    298: { q: '720p60', fmt: 'MP4', type: 'video', muxed: false },
    299: { q: '1080p60', fmt: 'MP4', type: 'video', muxed: false },
    302: { q: '720p60', fmt: 'WebM', type: 'video', muxed: false },
    303: { q: '1080p60', fmt: 'WebM', type: 'video', muxed: false },
    398: { q: '720p', fmt: 'MP4', type: 'video', muxed: false },
    399: { q: '1080p', fmt: 'MP4', type: 'video', muxed: false },
    400: { q: '1440p', fmt: 'MP4', type: 'video', muxed: false },
    401: { q: '2160p', fmt: 'MP4', type: 'video', muxed: false },
    // Audio-only
    139: { q: '48k', fmt: 'M4A', type: 'audio', muxed: false },
    140: { q: '128k', fmt: 'M4A', type: 'audio', muxed: false },
    141: { q: '256k', fmt: 'M4A', type: 'audio', muxed: false },
    171: { q: '128k', fmt: 'WebM', type: 'audio', muxed: false },
    249: { q: '50k', fmt: 'Opus', type: 'audio', muxed: false },
    250: { q: '70k', fmt: 'Opus', type: 'audio', muxed: false },
    251: { q: '160k', fmt: 'Opus', type: 'audio', muxed: false },
  };

  const JUNK_PATH_RE =
    /no_input\.mp3|\/generate_204|favicon|\.svg(\?|$)/i;
  const YOUTUBE_UI_PATH_RE =
    /\/s\/search\/|\/ptracking|\/api\/stats|\/log_event|\/youtubei\/|\/timedtext|\/caption|\/ad_status|\/pagead\/|\/img\/|\/static\/|\/yts\/|\/s\/player\/|\/jsbin\//i;

  // ponytail: keep in sync with backend guess_extension + DownloadCategory;
  // shared.js also uses this inventory as the default browser-capture type list.
  const GRABBER_EXTENSIONS =
    'mp4|mkv|webm|mov|avi|wmv|flv|m4v|mp3|m4a|flac|ogg|wav|aac|wma|zip|rar|7z|tar|gz|bz2|xz|tgz|pdf|doc|docx|xls|xlsx|ppt|pptx|txt|epub|exe|dmg|pkg|iso|deb|rpm|msi|app|png|jpg|jpeg|gif|webp|torrent';

  const GRABBER_EXT_RE = new RegExp(`\\.(${GRABBER_EXTENSIONS})(\\?|#|$)`, 'i');

  const BLOB_MAX_BYTES = 32 * 1024 * 1024;
  const MIN_FAB_PX = 120;
  const STREAM_PATH_RE = /\.(m3u8|mpd)(\?|#|$)/i;
  const PROGRESSIVE_MEDIA_RE =
    /\.(mp4|webm|mkv|m4v|mov|avi|wmv|flac|m4a|mp3|ogg|wav|aac|wma)(\?|#|$)/i;

  function formatBytes(n) {
    if (!n || n <= 0) return '';
    const u = ['B', 'KB', 'MB', 'GB'];
    let i = 0;
    let v = n;
    while (v >= 1000 && i < u.length - 1) {
      v /= 1000;
      i++;
    }
    return `${v.toFixed(i > 0 ? 1 : 0)} ${u[i]}`;
  }

  function parseItag(url) {
    const m = String(url).match(/[?&]itag=(\d+)/);
    return m ? parseInt(m[1], 10) : null;
  }

  function isYoutubeHost(host) {
    const h = (host || '').trim().toLowerCase();
    return (
      h === 'youtu.be' ||
      h === 'youtube.com' ||
      h.endsWith('.youtube.com') ||
      h === 'youtube-nocookie.com' ||
      h.endsWith('.youtube-nocookie.com')
    );
  }

  function isGooglevideoUrl(url) {
    try {
      const h = new URL(url).hostname.toLowerCase();
      return h === 'googlevideo.com' || h.endsWith('.googlevideo.com');
    } catch {
      return false;
    }
  }

  function isDirectGooglevideoUrl(url) {
    if (!isGooglevideoUrl(url)) return false;
    try {
      const u = new URL(url);
      const sabr = u.searchParams.get('sabr');
      if (sabr === '1' || sabr === 'true') return false;
      return u.searchParams.has('itag');
    } catch {
      return false;
    }
  }

  function isYoutubeWatchUrl(url) {
    try {
      const u = new URL(url);
      if (!isYoutubeHost(u.hostname)) return false;
      if (u.hostname === 'youtu.be') return u.pathname.length > 1;
      return (
        u.pathname.startsWith('/watch') ||
        u.pathname.startsWith('/shorts/') ||
        u.pathname.startsWith('/live/')
      );
    } catch {
      return false;
    }
  }

  function isYoutubePageUrl(url) {
    try {
      return isYoutubeHost(new URL(url).hostname);
    } catch (_) {
      return false;
    }
  }

  /** Literal hosts matching Rust `is_blocked_download_host` (no DNS). */
  function isBlockedIpv4Octets(o) {
    if (o[0] === 0) return true;
    if (o[0] >= 224) return true;
    if (o[0] === 127) return true;
    if (o[0] === 10) return true;
    if (o[0] === 192 && o[1] === 168) return true;
    if (o[0] === 169 && o[1] === 254) return true;
    if (o[0] === 172 && o[1] >= 16 && o[1] <= 31) return true;
    if (o[0] === 100 && o[1] >= 64 && o[1] <= 127) return true;
    return false;
  }

  function isBlockedIpv6Literal(h) {
    const v6 = String(h).split('%')[0];
    if (v6 === '::1' || v6 === '::' || v6 === '0:0:0:0:0:0:0:1' || v6 === '0:0:0:0:0:0:0:0') {
      return true;
    }
    const mapped = v6.match(/:ffff:(\d+\.\d+\.\d+\.\d+)$/i);
    if (mapped) {
      const p = mapped[1].split('.').map(Number);
      if (p.length === 4 && p.every((n) => n >= 0 && n <= 255)) return isBlockedIpv4Octets(p);
    }
    const hexMapped = v6.match(/^[:0]*ffff:([0-9a-f]{1,4}):([0-9a-f]{1,4})$/i);
    if (hexMapped) {
      const hi = parseInt(hexMapped[1], 16);
      const lo = parseInt(hexMapped[2], 16);
      return isBlockedIpv4Octets([(hi >> 8) & 255, hi & 255, (lo >> 8) & 255, lo & 255]);
    }
    if (v6.startsWith(':')) return false;
    const first = v6.split(':')[0] || '';
    const n = parseInt(first, 16);
    if (!Number.isNaN(n) && first.length) {
      if ((n & 0xfe00) === 0xfc00) return true;
      if ((n & 0xffc0) === 0xfe80) return true;
      if ((n & 0xff00) === 0xff00) return true;
    }
    return false;
  }

  function isBlockedLiteralHost(host) {
    const h = String(host || '')
      .trim()
      .toLowerCase()
      .replace(/^\[|\]$/g, '');
    if (!h) return true;
    if (h === 'localhost' || h.endsWith('.localhost')) return true;
    if (h.includes(':')) return isBlockedIpv6Literal(h);
    const m = h.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
    if (!m) return false;
    const o = [Number(m[1]), Number(m[2]), Number(m[3]), Number(m[4])];
    if (o.some((n) => n > 255)) return false;
    return isBlockedIpv4Octets(o);
  }

  function isPublicHttpUrl(value) {
    const raw = String(value || '').trim();
    if (!raw || raw.length > 8192) return false;
    try {
      const parsed = new URL(raw);
      if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return false;
      if (!parsed.hostname) return false;
      return !isBlockedLiteralHost(parsed.hostname);
    } catch (_) {
      return false;
    }
  }

  function urlBasename(url) {
    try {
      const name = decodeURIComponent(new URL(url).pathname.split('/').pop() || '');
      if (name) return name.split(/[/\\]/).pop() || name;
    } catch (_) {}
    const last = String(url || '').split('/').pop() || '';
    return last.split('?')[0];
  }

  /** Strip byte-range / segment params so full file downloads. */
  function normalizeMediaUrl(raw) {
    if (!raw || typeof raw !== 'string') return '';
    try {
      const u = new URL(raw);
      ['range', 'rn', 'rbuf', 'sq', 'ump', 'alr', 'keepalive', 'cmo'].forEach((k) =>
        u.searchParams.delete(k),
      );
      return u.toString();
    } catch {
      return raw.split('&range=')[0].split('?range=')[0];
    }
  }

  function isJunkUrl(url) {
    if (!url || url.startsWith('data:')) return true;
    if (url.startsWith('blob:')) return false;
    let parsed;
    try {
      parsed = new URL(url);
    } catch {
      return false;
    }
    const host = parsed.hostname.toLowerCase();
    const pathQ = `${parsed.pathname}${parsed.search}`.toLowerCase();
    if (JUNK_PATH_RE.test(pathQ)) return true;
    if (host === 'doubleclick.net' || host.endsWith('.doubleclick.net')) return true;
    if (isYoutubeHost(host)) {
      if (YOUTUBE_UI_PATH_RE.test(pathQ)) return true;
      if (pathQ.includes('.mp3') && !pathQ.includes('videoplayback')) return true;
    }
    return false;
  }

  function isBlobUrl(url) {
    return !!url && String(url).startsWith('blob:');
  }

  function isGrabberLink(url, hasDownloadAttr) {
    if (hasDownloadAttr) return true;
    return GRABBER_EXT_RE.test(String(url || ''));
  }

  function isStreamContentType(contentType) {
    const ct = (contentType || '').toLowerCase();
    return (
      ct.includes('mpegurl') ||
      ct.includes('dash+xml') ||
      ct.includes('application/vnd.apple.mpegurl')
    );
  }

  /** Network sniff + overlay: streams and progressive media only (not pdf/zip/webpack manifest). */
  function isSniffableMedia(url, contentType) {
    if (!url || url.startsWith('data:') || url.startsWith('blob:')) return false;
    if (isJunkUrl(url)) return false;
    const ct = (contentType || '').toLowerCase();
    return (
      isGooglevideoUrl(url) ||
      STREAM_PATH_RE.test(url) ||
      PROGRESSIVE_MEDIA_RE.test(url) ||
      isStreamContentType(ct) ||
      ct.includes('video/') ||
      ct.includes('audio/')
    );
  }

  function isCapturableMedia(url, contentType) {
    return isSniffableMedia(url, contentType);
  }

  function shouldSniffInject(url, contentType, contentLength) {
    if (!isSniffableMedia(url, contentType)) return false;
    if (STREAM_PATH_RE.test(url) || isGooglevideoUrl(url) || isStreamContentType(contentType)) {
      return true;
    }
    if (PROGRESSIVE_MEDIA_RE.test(url)) {
      if (contentLength > 0 && contentLength < 8192) return false;
      return true;
    }
    // ponytail: CT-only sniff without extension needs Content-Length to avoid beacons
    if (contentLength <= 0) return false;
    if (contentLength < 8192) return false;
    return true;
  }

  /** Align hijacked browser downloads with overlay/queue routing. */
  function hijackPayloadForFalcon(downloadUrl, pageUrl) {
    const raw = String(downloadUrl || '').trim();
    const page = String(pageUrl || '').split('#')[0];
    if (isGooglevideoUrl(raw)) {
      if (isYoutubeWatchPage(page)) {
        return { url: page, format: null };
      }
    }
    if (isYoutubeWatchUrl(raw)) {
      return { url: raw.split('#')[0], format: null };
    }
    if (/\.mpd(\?|#|$)/i.test(raw)) {
      return { url: raw, format: 'best' };
    }
    return { url: raw, format: null };
  }

  // ponytail: same selector as the in-app sheet (src/lib/ytFormats.ts) — m4a is
  // YouTube's native audio container, so the download needs no re-encoding.
  const AUDIO_ONLY_FORMAT = 'bestaudio[ext=m4a]/bestaudio';

  function overlayEnqueuePayload(pageUrl, selected) {
    const page = String(pageUrl || '').split('#')[0];
    const src = String((selected && selected.url) || '').trim();
    if (selected && (selected.isBlob || isBlobUrl(src))) {
      return { url: src, format: null };
    }
    if (isYoutubeWatchPage(page)) {
      if (selected && selected.audioOnly) {
        return { url: page, format: AUDIO_ONLY_FORMAT };
      }
      const h = Number(selected && selected.height) || Number(selected && selected.label) || 720;
      const height = Math.min(Math.max(h, 144), 2160);
      return {
        url: page,
        format: `bestvideo[height<=${height}]+bestaudio/best[height<=${height}]/bv*+ba/b`,
      };
    }
    if (isDirectGooglevideoUrl(src)) {
      return { url: normalizeMediaUrl(src), format: null };
    }
    return { url: src, format: null };
  }

  function isMainYoutubePlayerVideo(video) {
    if (video.closest('ytd-ad-slot-renderer, .ytp-ad-module, .video-ads')) return false;
    if (video.classList.contains('html5-main-video')) return true;
    return !!video.closest('#movie_player, .html5-video-player, ytd-player, #ytd-player');
  }

  function videoElementSrc(el) {
    if (!el) return '';
    return el.currentSrc || el.src || '';
  }

  function isYoutubeWatchPage(url) {
    return isYoutubeWatchUrl(url);
  }

  /** FAB target: main player on watch pages, otherwise video must carry capturable src. */
  function isEligibleVideoElement(video, pageUrl) {
    if (!video || video.tagName !== 'VIDEO') return false;
    const r = video.getBoundingClientRect();
    if (r.width < MIN_FAB_PX || r.height < MIN_FAB_PX) return false;
    if (r.bottom < 0 || r.top > window.innerHeight) return false;
    const page = pageUrl || '';
    try {
      if (isYoutubeHost(new URL(page).hostname) && !isYoutubeWatchPage(page)) return false;
    } catch (_) {}
    if (isYoutubeWatchPage(page)) return isMainYoutubePlayerVideo(video);
    const src = videoElementSrc(video);
    if (!src || src.startsWith('data:')) return false;
    if (isBlobUrl(src)) return true;
    if (/^https?:/i.test(src)) return isSniffableMedia(src, video.getAttribute('type') || '');
    return false;
  }

  function detectFormat(url) {
    const u = url.toLowerCase();
    if (/\.m3u8(\?|#|$)/i.test(u)) return { fmt: 'HLS', type: 'stream' };
    if (/\.mpd(\?|#|$)/i.test(u)) return { fmt: 'DASH', type: 'stream' };
    if (u.includes('.webm')) return { fmt: 'WebM', type: 'video' };
    if (u.includes('.mp4') || u.includes('mime=video%2fmp4') || u.includes('mime=video/mp4')) {
      return { fmt: 'MP4', type: 'video' };
    }
    if (u.includes('mime=audio')) return { fmt: 'Audio', type: 'audio' };
    if (isGooglevideoUrl(url)) return { fmt: 'MP4', type: 'video' };
    return { fmt: 'Media', type: 'video' };
  }

  function detectQuality(url, meta) {
    const itag = parseItag(url);
    if (itag && YT_ITAG[itag]) return YT_ITAG[itag].q;
    const u = url.toLowerCase();
    const res = u.match(/(\d{3,4})p/);
    if (res) return res[1] + 'p';
    if (u.includes('1080')) return '1080p';
    if (u.includes('720')) return '720p';
    if (u.includes('480')) return '480p';
    if (meta && meta.contentLength > 0) return formatBytes(meta.contentLength);
    return '';
  }

  function hostLabel(url) {
    try {
      const h = new URL(url).hostname.replace(/^www\./, '');
      if (h === 'googlevideo.com' || h.endsWith('.googlevideo.com')) return 'YouTube CDN';
      if (isYoutubeHost(h)) return 'YouTube';
      return h;
    } catch {
      return 'Media';
    }
  }

  function extFromUrl(url, meta, fmt, isHls, isAudio) {
    if (isHls) return 'mp4';
    const pathMatch = url.match(/\.([a-z0-9]{2,5})(?:\?|#|$)/i);
    if (pathMatch) {
      const e = pathMatch[1].toLowerCase();
      if (!['com', 'net', 'org', 'php', 'htm', 'html', 'asp'].includes(e)) return e;
    }
    const ct = ((meta && meta.contentType) || '').toLowerCase();
    if (ct.includes('webm')) return 'webm';
    if (ct.includes('mp4') || ct.includes('mpeg4')) return 'mp4';
    if (ct.includes('mpegurl')) return 'mp4';
    if (ct.includes('matroska')) return 'mkv';
    if (ct.includes('quicktime')) return 'mov';
    if (ct.includes('ogg')) return 'ogg';
    if (ct.includes('flac')) return 'flac';
    if (ct.includes('mpeg') && ct.includes('audio')) return 'mp3';
    if (isAudio) {
      if (fmt.fmt === 'WebM' || fmt.fmt === 'Opus') return 'webm';
      return 'm4a';
    }
    if (fmt.fmt === 'WebM') return 'webm';
    if (fmt.fmt && !['Media', 'HLS', 'DASH', 'Audio'].includes(fmt.fmt)) {
      return fmt.fmt.toLowerCase();
    }
    return 'bin';
  }

  function analyzeUrl(url, meta) {
    meta = meta || {};
    const clean = normalizeMediaUrl(url);
    const itag = parseItag(clean);
    const yt = itag ? YT_ITAG[itag] : null;
    const fmt = yt ? { fmt: yt.fmt, type: yt.type } : detectFormat(clean);
    const quality = yt ? yt.q : detectQuality(clean, meta);
    const ct = (meta.contentType || '').toLowerCase();
    const isHls = /\.m3u8(\?|#|$)/i.test(clean) || ct.includes('mpegurl');
    const isDash = /\.mpd(\?|#|$)/i.test(clean) || ct.includes('dash+xml');
    const isAudio = fmt.type === 'audio' || (yt && yt.type === 'audio');
    const muxed = yt ? !!yt.muxed : !isAudio && !isHls && !isDash;
    const size = meta.contentLength || 0;

    let score = 0;
    if (muxed && !isAudio) score += 20000; // progressive muxed wins
    if (isHls) score += 8000;
    if (isDash) score += 7500;
    if (!isAudio) score += 4000;
    else score -= 2000;
    const qn = parseInt(String(quality), 10) || 0;
    score += qn;
    if (size > 0) score += Math.min(Math.log10(size) * 100, 500);

    const kind = isHls
      ? _i18n('kindStream', 'Stream')
      : isDash
        ? _i18n('kindDash', 'DASH stream')
      : isAudio
        ? _i18n('kindAudio', 'Audio')
        : muxed
          ? _i18n('kindVideoAudio', 'Video+Audio')
          : _i18n('kindVideoOnly', 'Video only');
    const title = quality ? `${quality} · ${fmt.fmt}` : `${fmt.fmt} · ${kind}`;
    const sizeLabel = size > 0 ? formatBytes(size) : '';
    const subtitle = isGooglevideoUrl(clean)
      ? [kind, sizeLabel].filter(Boolean).join(' · ')
      : [hostLabel(clean), kind, sizeLabel].filter(Boolean).join(' · ');
    const ext = extFromUrl(clean, meta, fmt, isHls, isAudio);

    return {
      url: clean,
      title,
      subtitle,
      quality,
      format: fmt.fmt,
      mediaType: fmt.type,
      isHls,
      isDash,
      isAudio,
      muxed,
      size,
      sizeLabel: size > 0 ? formatBytes(size) : '',
      score,
      ext,
      itag,
    };
  }

  function groupSources(urls, metaMap) {
    const seen = new Set();
    const items = [];
    for (const raw of urls || []) {
      if (!raw || isJunkUrl(raw) || isBlobUrl(raw)) continue;
      const clean = normalizeMediaUrl(raw);
      if (!clean || seen.has(clean)) continue;
      // Dedupe by itag when present (same quality, different CDN hosts)
      const itag = parseItag(clean);
      const dedupeKey = itag ? `itag:${itag}` : clean;
      if (seen.has(dedupeKey)) continue;
      seen.add(clean);
      seen.add(dedupeKey);

      const meta = (metaMap && (metaMap[raw] || metaMap[clean])) || {};
      const isYt = isGooglevideoUrl(clean);
      if (isYt && !isDirectGooglevideoUrl(clean)) continue;
      if (
        meta.contentLength > 0 &&
        meta.contentLength < 2048 &&
        !clean.includes('.m3u8') &&
        !isYt
      ) {
        continue;
      }
      items.push(analyzeUrl(clean, meta));
    }
    items.sort((a, b) => b.score - a.score);
    return items;
  }

  function pickBest(items, preferVideo) {
    if (!items.length) return null;
    const muxed = items.filter((i) => i.muxed && !i.isAudio);
    const pool =
      muxed.length > 0
        ? muxed
        : preferVideo !== false
          ? items.filter((i) => !i.isAudio)
          : items;
    const list = pool.length ? pool : items;
    return list.reduce((best, item) => (item.score > best.score ? item : best));
  }

  function defaultFilename(pageTitle, item) {
    let base = (pageTitle || 'download')
      .replace(/\s*-\s*YouTube\s*$/i, '')
      // Only characters no file system accepts; titles in any script stay readable.
      .replace(/[\u0000-\u001f\u007f/\\:*?"<>|]+/g, ' ')
      .replace(/\s+/g, ' ')
      .trim()
      .slice(0, 120);
    if (!base) base = 'download';
    const ext = item ? item.ext : 'bin';
    if (/\.[a-z0-9]{2,5}$/i.test(base)) return base;
    return `${base}.${ext}`;
  }

  /** CDN/stream placeholders only — never reject a real server filename. */
  function isJunkHijackFilename(name, url) {
    const base = String(name || '')
      .trim()
      .split(/[/\\]/)
      .pop()
      .toLowerCase();
    if (!base) return true;
    if (base === 'videoplayback' || base.startsWith('videoplayback.')) return true;
    if (base === 'watch' || base === 'watch.html') return true;
    if (base === 'download' || base === 'download.bin') return true;
    if (base === 'index.html') return true;
    return false;
  }

  function guessExtensionFromDownloadUrl(url) {
    const lower = String(url || '').toLowerCase();
    if (/\.mpd(\?|#|$)/.test(lower)) return 'mp4';
    if (/\.m3u8(\?|#|$)/.test(lower)) return 'mp4';
    const mime = lower.match(/mime=([a-z0-9%/._-]+)/);
    if (mime) {
      const m = decodeURIComponent(mime[1]);
      if (m.includes('video/mp4') || m.includes('audio/mp4')) return 'mp4';
      if (m.includes('video/webm')) return 'webm';
      if (m.includes('audio/mpeg')) return 'mp3';
      if (m.includes('audio/mp4') || m.includes('audio/m4a')) return 'm4a';
      if (m.includes('application/pdf')) return 'pdf';
      if (m.includes('application/zip')) return 'zip';
    }
    for (const ext of [
      'mp4', 'webm', 'mkv', 'm4a', 'mp3', 'flac', 'zip', 'rar', '7z', 'pdf', 'exe', 'dmg', 'pkg',
    ]) {
      if (lower.includes(`.${ext}`)) return ext;
    }
    return 'bin';
  }

  /** Selector tiers when CDN capture is empty (modern YouTube uses SABR/cipher). */
  function youtubeFallbackSources(pageUrl) {
    const watch = String(pageUrl || '').split('#')[0];
    if (!isYoutubeWatchUrl(watch)) return [];
    const tiers = [
      { height: 360, label: '360' },
      { height: 480, label: '480' },
      { height: 720, label: '720' },
      { height: 1080, label: '1080' },
    ];
    const hint = _i18n('youtubeHint', 'Native YouTube download');
    const cards = tiers.map(({ height, label }) => ({
      url: watch,
      title: `${height}p · MP4 · ${_i18n('kindVideoAudio', 'Video+Audio')}`,
      subtitle: hint,
      quality: `${height}p`,
      format: 'MP4',
      muxed: true,
      isAudio: false,
      isHls: false,
      height,
      label,
      score: height,
      ext: 'mp4',
    }));
    // Audio-only parity with the in-app sheet: same card, same selector.
    cards.push({
      url: watch,
      title: _i18n('qualityAudioOnly', 'Audio only (M4A)'),
      subtitle: hint,
      quality: '',
      format: 'M4A',
      muxed: false,
      isAudio: true,
      audioOnly: true,
      isHls: false,
      score: 0,
      ext: 'm4a',
    });
    return cards;
  }

  function collectPageBlobSources() {
    if (typeof document === 'undefined') return [];
    const out = [];
    const seen = new Set();
    document.querySelectorAll('video, audio').forEach((el) => {
      const src = el.currentSrc || el.src || '';
      if (!isBlobUrl(src) || seen.has(src)) return;
      seen.add(src);
      const ct = String(el.type || '');
      const isAudio = el.tagName === 'AUDIO' || ct.startsWith('audio/');
      const ext = ct.includes('webm') ? 'webm' : ct.includes('ogg') ? 'ogg' : isAudio ? 'm4a' : 'mp4';
      out.push({
        url: src,
        title: isAudio
          ? _i18n('blobAudio', 'Page audio (blob)')
          : _i18n('blobVideo', 'Page video (blob)'),
        subtitle: _i18n('blobHint', 'Fetched from this tab'),
        quality: '',
        format: ext.toUpperCase(),
        mediaType: isAudio ? 'audio' : 'video',
        isHls: false,
        isAudio,
        isBlob: true,
        muxed: true,
        size: 0,
        sizeLabel: '',
        score: 12000,
        ext,
      });
    });
    return out;
  }

  root.FalconMedia = {
    analyzeUrl,
    groupSources,
    pickBest,
    defaultFilename,
    isJunkHijackFilename,
    guessExtensionFromDownloadUrl,
    formatBytes,
    normalizeMediaUrl,
    isJunkUrl,
    isBlobUrl,
    isGrabberLink,
    GRABBER_EXT_RE,
    GRABBER_EXTENSIONS,
    BLOB_MAX_BYTES,
    collectPageBlobSources,
    isCapturableMedia,
    isSniffableMedia,
    shouldSniffInject,
    isEligibleVideoElement,
    isYoutubeWatchPage,
    isMainYoutubePlayerVideo,
    hijackPayloadForFalcon,
    overlayEnqueuePayload,
    videoElementSrc,
    MIN_FAB_PX,
    isYoutubeHost,
    hostLabel,
    isYoutubePageUrl,
    isBlockedLiteralHost,
    isPublicHttpUrl,
    urlBasename,
    isGooglevideoUrl,
    isDirectGooglevideoUrl,
    isYoutubeWatchUrl,
    youtubeFallbackSources,
    YT_ITAG,
  };
})(typeof globalThis !== 'undefined' ? globalThis : window);

const FALCON_API = 'http://127.0.0.1:14201';
const WAKE_URL = 'falcondm://wake';
const MEDIA_URLS = new Map();
const MEDIA_META = new Map();
let pairInFlight = null;
const REQUEST_TIMEOUT_MS = 10000;
const HEALTH_TIMEOUT_MS = 4000;
const PAIR_POLL_ATTEMPTS = 15;

/** Firefox's Origin host is the per-install UUID exposed as @@extension_id, not runtime.id. */
function extensionOriginId() {
  try {
    const internal = chrome.i18n.getMessage('@@extension_id');
    if (internal) return internal;
  } catch (_) {}
  return (chrome.runtime && chrome.runtime.id) || '';
}

function isOwnBrowserDownload(item) {
  const owner = item && item.byExtensionId;
  return !!owner && (owner === chrome.runtime.id || owner === extensionOriginId());
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

function withTimeout(promise, timeoutMs, label) {
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${label} timed out`)), timeoutMs);
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}

function fetchWithTimeout(url, options, timeoutMs, label) {
  const controller = new AbortController();
  return withTimeout(fetch(url, { ...options, signal: controller.signal }), timeoutMs, label).catch(
    (error) => {
      controller.abort();
      throw error;
    },
  );
}

// Connection / UI state shared with popup + toolbar badge.
// ponytail: MV3 service workers die after ~30s idle; in-memory prefs reset.
// Pause + fail-closed live in storage.local so a browser restart keeps them.
let connectionState = 'offline'; // "connected" | "pending" | "blocked" | "offline"
let interceptPaused = false; // when true, automatic hijack is off (browser downloads natively)
let interceptFailClosed = true; // default fail-closed: Chrome does not keep hijacked files
let notifySuccess = false; // success popups are opt-in — errors always notify
const INJECTED = new Set(); // tab ids that already have the on-demand content script

const CAPTURE_RULES_MAX_CHARS = 8192;
const CAPTURE_EXTENSION_MAX_COUNT = 256;
const CAPTURE_HOST_MAX_COUNT = 256;

function defaultCaptureExtensionsText() {
  const fm = typeof FalconMedia !== 'undefined' ? FalconMedia : null;
  return ((fm && fm.GRABBER_EXTENSIONS) || '')
    .split('|')
    .filter(Boolean)
    .join(' ');
}

/** Lowercase alphanumeric extension list; separators are commas, whitespace, or newlines. */
function normalizeCaptureExtensions(input) {
  const raw = String(input ?? '')
    .slice(0, CAPTURE_RULES_MAX_CHARS)
    .toLowerCase();
  const out = [];
  const seen = new Set();
  let dropped = 0;
  for (const token of raw.split(/[\s,]+/)) {
    const ext = token.replace(/^\.+/, '');
    if (!ext) continue;
    if (!/^[a-z0-9]{1,24}$/.test(ext) || out.length >= CAPTURE_EXTENSION_MAX_COUNT) {
      dropped += 1;
      continue;
    }
    if (seen.has(ext)) continue;
    seen.add(ext);
    out.push(ext);
  }
  return { text: out.join(' '), extensions: out, dropped };
}

function canonicalCaptureHost(host) {
  return String(host || '')
    .trim()
    .toLowerCase()
    .replace(/\.+$/g, '');
}

function isValidCaptureHost(host) {
  if (!host || host.length > 253) return false;
  const ipv4 = host.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
  if (ipv4) return ipv4.slice(1).every((part) => Number(part) <= 255);
  return host
    .split('.')
    .every((label) => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label));
}

/** One host per line; exact entries match exactly, *.domain / .domain include subdomains. */
function normalizeCaptureBlockedHosts(input) {
  const raw = String(input ?? '')
    .slice(0, CAPTURE_RULES_MAX_CHARS)
    .toLowerCase();
  const out = [];
  const seen = new Set();
  let dropped = 0;
  for (const line of raw.split(/\r?\n/)) {
    let value = line.trim();
    if (!value) continue;
    let wildcard = false;
    if (value.startsWith('*.')) {
      wildcard = true;
      value = value.slice(2);
    } else if (value.startsWith('.')) {
      wildcard = true;
      value = value.slice(1);
    }
    value = canonicalCaptureHost(value);
    if (!isValidCaptureHost(value) || out.length >= CAPTURE_HOST_MAX_COUNT) {
      dropped += 1;
      continue;
    }
    const rule = `${wildcard ? '*.' : ''}${value}`;
    if (seen.has(rule)) continue;
    seen.add(rule);
    out.push(rule);
  }
  return { text: out.join('\n'), hosts: out, dropped };
}

/** Wildcards cover the base host too, matching browser match-pattern user expectations. */
function captureHostMatches(host, rule) {
  const h = canonicalCaptureHost(host);
  let r = canonicalCaptureHost(rule);
  if (!h || !r) return false;
  if (r.startsWith('.')) r = `*.${r.slice(1)}`;
  if (r.startsWith('*.')) {
    const base = r.slice(2);
    return h === base || h.endsWith(`.${base}`);
  }
  return h === r;
}

function captureUrlHost(value) {
  try {
    return canonicalCaptureHost(new URL(value).hostname);
  } catch (_) {
    return '';
  }
}

function captureDownloadBlocked(item, blockedHosts) {
  const rules = Array.isArray(blockedHosts) ? blockedHosts : [];
  if (!rules.length) return false;
  const hosts = [item && item.referrer, item && item.finalUrl, item && item.url]
    .map((value) => captureUrlHost(value))
    .filter(Boolean);
  return hosts.some((host) => rules.some((rule) => captureHostMatches(host, rule)));
}

function captureFileExtension(value) {
  let raw = String(value || '').trim();
  if (!raw) return '';
  if (/^https?:/i.test(raw)) {
    try {
      raw = new URL(raw).pathname;
    } catch (_) {}
  }
  let base = raw.split(/[?#]/)[0].split(/[/\\]/).pop() || '';
  try {
    base = decodeURIComponent(base);
  } catch (_) {}
  const match = base.match(/\.([a-z0-9]{1,24})$/i);
  return match ? match[1].toLowerCase() : '';
}

function captureDownloadExtension(item) {
  for (const candidate of [item && item.filename, item && item.finalUrl, item && item.url]) {
    const ext = captureFileExtension(candidate);
    if (ext) return ext;
  }
  return '';
}

/** Automatic browser capture only; explicit context-menu sends intentionally bypass this gate. */
function shouldCaptureBrowserDownload(item, rules = {}) {
  const extensions = Array.isArray(rules.extensions) ? rules.extensions : captureExtensions;
  const blockedHosts = Array.isArray(rules.blockedHosts) ? rules.blockedHosts : captureBlockedHosts;
  const ext = captureDownloadExtension(item);
  if (!ext || !extensions.includes(ext)) return false;
  return !captureDownloadBlocked(item, blockedHosts);
}

const initialCaptureExtensions = normalizeCaptureExtensions(defaultCaptureExtensionsText());
let captureExtensions = initialCaptureExtensions.extensions;
let captureBlockedHosts = [];
let captureExtensionsDirty = false;
let captureBlockedHostsDirty = false;

function applyCaptureExtensions(input) {
  const normalized = normalizeCaptureExtensions(input);
  captureExtensions = normalized.extensions;
  return normalized;
}

function applyCaptureBlockedHosts(input) {
  const normalized = normalizeCaptureBlockedHosts(input);
  captureBlockedHosts = normalized.hosts;
  return normalized;
}

try {
  if (ext.storage && ext.storage.onChanged) {
    ext.storage.onChanged.addListener((changes, area) => {
      if (area !== 'local') return;
      if (changes.falconCaptureExtensions) {
        captureExtensionsDirty = true;
        applyCaptureExtensions(
          changes.falconCaptureExtensions.newValue ?? defaultCaptureExtensionsText(),
        );
      }
      if (changes.falconCaptureBlockedHosts) {
        captureBlockedHostsDirty = true;
        applyCaptureBlockedHosts(changes.falconCaptureBlockedHosts.newValue || '');
      }
    });
  }
} catch (_) {}

// Writers set dirty so an in-flight hydrate get() cannot clobber a live toggle.
let pauseDirty = false;
let failClosedDirty = false;
let notifySuccessDirty = false;
let prefsHydrated = false;

function markPauseWrite() {
  pauseDirty = true;
}

function markFailClosedWrite() {
  failClosedDirty = true;
}

function markNotifySuccessWrite() {
  notifySuccessDirty = true;
}

const prefsReady = (async () => {
  try {
    const { falconConnectionState } = await ext.storage.session.get({
      falconConnectionState: 'offline',
    });
    const { falconInterceptFailClosed } = await ext.storage.local.get({
      falconInterceptFailClosed: true,
    });
    const { falconNotifySuccess } = await ext.storage.local.get({
      falconNotifySuccess: false,
    });
    const captureRulesBag = await ext.storage.local.get({
      falconCaptureExtensions: defaultCaptureExtensionsText(),
      falconCaptureBlockedHosts: '',
    });
    const localPauseBag = await ext.storage.local.get('falconInterceptPaused');
    const hasLocalPause = Object.hasOwn(localPauseBag, 'falconInterceptPaused');
    // Session is a one-shot migrate from the old SW-only pause. Explicit local
    // false (Resume) must win — defaulted get() treated Resume as "missing".
    let paused = hasLocalPause ? !!localPauseBag.falconInterceptPaused : false;
    if (!hasLocalPause) {
      const sessionPause = await ext.storage.session.get({ falconInterceptPaused: false });
      paused = !!sessionPause.falconInterceptPaused;
      if (paused) {
        ext.storage.local.set({ falconInterceptPaused: true }).catch(() => {});
      }
    }
    ext.storage.session.remove('falconInterceptPaused').catch(() => {});
    if (!pauseDirty) {
      interceptPaused = paused;
    }
    if (!failClosedDirty) interceptFailClosed = !!falconInterceptFailClosed;
    if (!notifySuccessDirty) notifySuccess = !!falconNotifySuccess;
    if (!captureExtensionsDirty) applyCaptureExtensions(captureRulesBag.falconCaptureExtensions);
    if (!captureBlockedHostsDirty) applyCaptureBlockedHosts(captureRulesBag.falconCaptureBlockedHosts);
    if (connectionState === 'offline') {
      connectionState = falconConnectionState || 'offline';
    }
    refreshBadge();
  } catch (_) {}
  prefsHydrated = true;
})();

async function getInterceptFailClosed() {
  if (failClosedDirty) return interceptFailClosed;
  try {
    const { falconInterceptFailClosed } = await ext.storage.local.get({
      falconInterceptFailClosed: true,
    });
    interceptFailClosed = !!falconInterceptFailClosed;
  } catch (_) {}
  return interceptFailClosed;
}

function eraseBrowserDownload(id) {
  if (id == null) return;
  try {
    chrome.downloads.erase({ id }, () => void chrome.runtime.lastError);
  } catch (_) {}
}

async function setInterceptFailClosed(next) {
  markFailClosedWrite();
  interceptFailClosed = !!next;
  await ext.storage.local.set({ falconInterceptFailClosed: interceptFailClosed });
  return interceptFailClosed;
}

async function setNotifySuccessEnabled(next) {
  markNotifySuccessWrite();
  notifySuccess = !!next;
  await ext.storage.local.set({ falconNotifySuccess: notifySuccess });
  return notifySuccess;
}

function setState(s) {
  if (connectionState === s) return;
  connectionState = s;
  refreshBadge();
  // Best-effort persist (non-blocking); SW restart will restore the badge.
  ext.storage.session.set({ falconConnectionState: connectionState }).catch(() => {});
}

/** Reflect connection state on the toolbar icon badge. */
function refreshBadge() {
  const map = {
    connected: { text: '✓', color: '#22c55e' },
    pending: { text: '•', color: '#D97706' },
    blocked: { text: '!', color: '#dc2626' },
    // Gray dash: an empty badge looked indistinguishable from a working ext.
    offline: { text: '–', color: '#8e8e93' },
  };
  const b = map[connectionState] || map.offline;
  try {
    ext.action.setBadgeBackgroundColor({ color: b.color });
    ext.action.setBadgeText({ text: b.text });
  } catch (_) {}
}


/** Popup window is last-focused while open — never query lastFocusedWindow for page tabs. */
async function getActiveBrowserTab() {
  try {
    const win = await ext.windows.getLastFocused({ windowTypes: ['normal'] });
    if (win && win.id != null) {
      const [tab] = await ext.tabs.query({ active: true, windowId: win.id });
      if (tab) return tab;
    }
  } catch (_) {}
  try {
    const [tab] = await ext.tabs.query({ active: true, windowType: 'normal' });
    if (tab) return tab;
  } catch (_) {}
  return null;
}

/** Inject the on-demand content script once per tab. Idempotent. */
async function resetContentScriptGuard(tabId) {
  try {
    await ext.scripting.executeScript({
      target: { tabId },
      func: () => {
        delete window.__falconDmReady;
        delete window.__falconDmContentScript;
        document.querySelectorAll('[data-falcon-fab]').forEach((node) => node.remove());
      },
    });
  } catch (_) {}
}

async function reinitContentScriptsAfterReload() {
  INJECTED.clear();
  try {
    const tabs = await ext.tabs.query({
      url: [
        '*://*.youtube.com/*',
        '*://youtu.be/*',
        '*://*.youtube-nocookie.com/*',
      ],
    });
    for (const tab of tabs) {
      if (tab.id) ensureContentScript(tab.id).catch(() => {});
    }
  } catch (_) {}
}

/** Inject the on-demand content script once per tab. Idempotent. */
async function ensureContentScript(tabId) {
  if (!tabId || tabId < 0) return false;
  try {
    await ext.tabs.sendMessage(tabId, { action: 'ping' });
    INJECTED.add(tabId);
    return true;
  } catch (_) {
    INJECTED.delete(tabId);
  }
  INJECTED.add(tabId);
  try {
    await resetContentScriptGuard(tabId);
    await ext.scripting.executeScript({
      target: { tabId },
      files: ['media-utils.js', 'content.js'],
    });
    await ext.tabs.sendMessage(tabId, { action: 'ping' });
    return true;
  } catch (_) {
    INJECTED.delete(tabId); // allow retry (e.g. chrome:// pages reject injection)
    return false;
  }
}

async function getToken() {
  const { apiToken } = await ext.storage.local.get({ apiToken: '' });
  return (apiToken || '').trim();
}

async function appHealthy() {
  try {
    const r = await fetchWithTimeout(
      `${FALCON_API}/api/health`,
      { method: 'GET' },
      HEALTH_TIMEOUT_MS,
      'Falcon health check',
    );
    return r.ok;
  } catch {
    return false;
  }
}

function msg(key, fallback) {
  return chrome.i18n.getMessage(key) || fallback;
}

/** Trigger OS handler to launch Falcon DM (IDM-style wake). */
function wakeFalcon() {
  return new Promise((resolve) => {
    chrome.tabs.create({ url: WAKE_URL, active: false }, (tab) => {
      setTimeout(() => {
        if (tab && tab.id) chrome.tabs.remove(tab.id, () => {});
        resolve();
      }, 600);
    });
  });
}

async function waitForHealthy(timeoutMs = 25000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (await appHealthy()) return true;
    await sleep(400);
  }
  return false;
}

/** Ensure Falcon is running — wake + poll like IDM. */
async function ensureAppRunning() {
  if (await appHealthy()) return true;
  await withTimeout(wakeFalcon(), 5000, 'Falcon wake');
  return waitForHealthy(25000);
}
function cookieLookupUrl(downloadUrl, pageUrl) {
  const dl = (downloadUrl || '').trim();
  const page = (pageUrl || '').trim();
  const fm = typeof FalconMedia !== 'undefined' ? FalconMedia : null;
  const isGv = fm ? fm.isGooglevideoUrl(dl) : false;
  let isYtPage = false;
  if (page && /^https?:/i.test(page)) {
    try {
      isYtPage = fm ? fm.isYoutubeHost(new URL(page).hostname) : false;
    } catch (_) {}
  }
  if (isGv && isYtPage) return page.split('#')[0];
  return dl;
}

async function getCookiesHeader(url) {
  if (!url) return '';
  try {
    const cookies = await withTimeout(ext.cookies.getAll({ url }), 3000, 'Cookie lookup');
    if (cookies.length) {
      return cookies.map((c) => `${c.name}=${c.value}`).join('; ');
    }
  } catch (_) {}
  return '';
}

const NOTIFY_DEDUP_MS = 10000;
const notifyLastAt = new Map();

function notify(title, message) {
  // Batch identical popups: a burst of hijacks or errors must not spam the OS.
  const key = `${title}\0${message}`;
  const now = Date.now();
  if (now - (notifyLastAt.get(key) || 0) < NOTIFY_DEDUP_MS) return;
  notifyLastAt.set(key, now);
  ext.notifications.create({
    type: 'basic',
    iconUrl: 'icon128.png',
    title,
    message,
  });
}

async function fetchBlobPayload(tabId, blobUrl) {
  const maxBytes =
    (self.FalconMedia && self.FalconMedia.BLOB_MAX_BYTES) || 32 * 1024 * 1024;
  const [{ result }] = await ext.scripting.executeScript({
    target: { tabId },
    world: 'MAIN',
    func: async (url, limit) => {
      try {
        const response = await fetch(url);
        if (!response.ok) throw new Error(`fetch failed (${response.status})`);
        const blob = await response.blob();
        if (blob.size > limit) {
          throw new Error(`blob too large (${blob.size} bytes, max ${limit})`);
        }
        const buf = await blob.arrayBuffer();
        const bytes = new Uint8Array(buf);
        let binary = '';
        for (let i = 0; i < bytes.length; i += 0x8000) {
          binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
        }
        return {
          ok: true,
          base64: btoa(binary),
          size: blob.size,
          type: blob.type || '',
        };
      } catch (error) {
        return { ok: false, error: error.message || 'blob fetch failed' };
      }
    },
    args: [blobUrl, maxBytes],
  });
  return result || { ok: false, error: 'blob fetch failed' };
}

// Pairing with the desktop app.
//
// The extension shows a 6-digit code and asks Falcon DM to pair. The app shows
// the same code and the user approves it there. The API token only reaches the
// extension after that approval. Web pages cannot forge the browser-extension
// Origin the request carries.

const PAIR_CODE_KEY = 'falconPairCode';
let pairCode = '';

function newPairCode() {
  const bytes = new Uint32Array(1);
  crypto.getRandomValues(bytes);
  return String(bytes[0] % 1000000).padStart(6, '0');
}

/** The code currently shown to the user. Kept until the app answers. */
async function currentPairCode(fresh = false) {
  if (!fresh && pairCode) return pairCode;
  if (!fresh) {
    const stored = await ext.storage.local.get(PAIR_CODE_KEY).catch(() => ({}));
    if (/^\d{6}$/.test(stored[PAIR_CODE_KEY] || '')) {
      pairCode = stored[PAIR_CODE_KEY];
      return pairCode;
    }
  }
  pairCode = newPairCode();
  await ext.storage.local.set({ [PAIR_CODE_KEY]: pairCode }).catch(() => {});
  return pairCode;
}

async function forgetPairCode() {
  pairCode = '';
  await ext.storage.local.remove(PAIR_CODE_KEY).catch(() => {});
}

/** Code to show in the popup while the app waits for approval. */
function visiblePairCode() {
  return connectionState === 'pending' ? pairCode : '';
}

async function requestPair() {
  const code = await currentPairCode();
  return fetchWithTimeout(
    `${FALCON_API}/api/pair`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ extension_id: extensionOriginId(), code }),
    },
    REQUEST_TIMEOUT_MS,
    'Pair request',
  );
}

/** 200 token · 202 waiting · 403 declined · 503 app not ready yet. */
async function readPairResponse(resp) {
  if (resp.status === 202) return { pending: true };
  if (resp.status === 403) return { rejected: true };
  if (resp.status === 503) return { retry: true };
  if (!resp.ok) return { error: true };
  const data = await resp.json().catch(() => null);
  if (data && data.ok && data.token) {
    await ext.storage.local.set({ apiToken: data.token });
    await forgetPairCode();
    return { token: data.token };
  }
  return data && data.pending ? { pending: true } : { error: true };
}

async function pingToken(token) {
  try {
    const r = await fetchWithTimeout(
      `${FALCON_API}/api/ping`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Falcon-Token': token },
        body: '{}',
      },
      REQUEST_TIMEOUT_MS,
      'Falcon ping',
    );
    return r.status;
  } catch {
    return 0;
  }
}

async function declined() {
  // The next attempt shows a new code, so the user can try again on purpose.
  await forgetPairCode();
  setState('blocked');
  throw new Error(
    msg('errorPairRejected', 'Falcon DM declined the request. Try again to get a new code.'),
  );
}

async function ensurePaired(force = false) {
  if (pairInFlight) return pairInFlight;
  pairInFlight = (async () => {
    try {
      if (!(await appHealthy())) {
        await withTimeout(wakeFalcon(), 5000, 'Falcon wake');
        if (!(await waitForHealthy(25000))) {
          throw new Error(msg('errorWaking', 'Falcon DM did not start. Open it manually.'));
        }
      }

      const existing = force ? '' : await getToken();
      if (existing) {
        const status = await pingToken(existing);
        if (status >= 200 && status < 300) {
          setState('connected');
          return existing;
        }
        // 401: token rotated (another extension was disconnected) — pair again.
        // 403: this extension was disconnected — needs a new approval.
        await ext.storage.local.remove('apiToken');
      }

      let result = { retry: true };
      for (let i = 0; result.retry && i < 6; i++) {
        if (i) await sleep(500);
        result = await readPairResponse(await requestPair());
      }
      if (result.token) {
        setState('connected');
        return result.token;
      }
      if (result.rejected) return declined();
      if (!result.pending) {
        throw new Error(msg('errorAppOffline', 'Could not pair with Falcon DM'));
      }

      setState('pending');
      notify(
        msg('appName', 'Falcon DM'),
        msg('pairNotify', 'Approve code {code} in Falcon DM').replace('{code}', formatPairCode(pairCode)),
      );
      for (let i = 0; i < PAIR_POLL_ATTEMPTS; i++) {
        await sleep(2000);
        let next;
        try {
          next = await readPairResponse(await requestPair());
        } catch {
          continue;
        }
        if (next.token) {
          setState('connected');
          return next.token;
        }
        if (next.rejected) return declined();
      }
      throw new Error(msg('errorPairPending', 'Approve the code in Falcon DM, then try again.'));
    } catch (e) {
      if (connectionState !== 'blocked' && connectionState !== 'pending') setState('offline');
      throw e;
    } finally {
      pairInFlight = null;
    }
  })();
  return pairInFlight;
}

function formatPairCode(code) {
  return code ? `${code.slice(0, 3)} ${code.slice(3)}` : '';
}

function mapHttpError(status, bodyError) {
  if (status === 401)
    return msg('errorTokenInvalid', 'Connection lost — Falcon DM will pair again automatically');
  if (status === 403)
    return msg('errorExtensionBlocked', 'This extension is not approved in Falcon DM');
  if (status === 429) return msg('errorRateLimit', 'Too many requests — try again shortly');
  if (bodyError) return bodyError;
  if (status) return msg('errorHttp', 'Request failed') + ` (HTTP ${status})`;
  return msg('errorAppOffline', 'Falcon DM is not running — open the desktop app');
}

async function postFalcon(path, body, timeoutMs = REQUEST_TIMEOUT_MS) {
  let token = await ensurePaired(false);

  let response;
  try {
    response = await fetchWithTimeout(
      `${FALCON_API}${path}`,
      {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'X-Falcon-Token': token,
        },
        body: JSON.stringify(body),
      },
      timeoutMs,
      `Falcon ${path}`,
    );
  } catch {
    setState('offline');
    throw new Error(msg('errorAppOffline', 'Falcon DM is not running — open the desktop app'));
  }

  if (response.status === 401) {
    token = await ensurePaired(true);
    try {
      response = await fetchWithTimeout(
        `${FALCON_API}${path}`,
        {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'X-Falcon-Token': token,
          },
          body: JSON.stringify(body),
        },
        timeoutMs,
        `Falcon ${path} retry`,
      );
    } catch {
      setState('offline');
      throw new Error(msg('errorAppOffline', 'Falcon DM is not running — open the desktop app'));
    }
  }

  let data = null;
  try {
    data = await response.json();
  } catch {
    // Error responses without a JSON body fall through with data = null.
  }

  if (response.status === 403) setState('blocked');
  if (!response.ok) {
    throw new Error(mapHttpError(response.status, data && data.error));
  }
  if (data && data.success === false) {
    const err = data.error || msg('errorInvalidUrl', 'Invalid download URL');
    if (err === 'ERR_UNSUPPORTED_MAGNET') {
      throw new Error(
        msg('errorUnsupportedMagnet', 'Magnet and BitTorrent links are not supported yet'),
      );
    }
    if (err === 'invalid url') throw new Error(msg('errorInvalidUrl', 'No valid URL'));
    throw new Error(err);
  }
  return data;
}

async function sendToFalcon(path, body, timeoutMs = REQUEST_TIMEOUT_MS) {
  // Connected badge: skip health RTT — postFalcon fails fast if socket is dead.
  if (connectionState === 'connected') {
    try {
      return await postFalcon(path, body, timeoutMs);
    } catch (e) {
      if (!(await appHealthy())) {
        await withTimeout(wakeFalcon(), 5000, 'Falcon wake');
        if (!(await waitForHealthy(15000))) {
          setState('offline');
          throw new Error(msg('errorWaking', 'Falcon DM did not start. Open it manually.'));
        }
        return postFalcon(path, body, timeoutMs);
      }
      throw e;
    }
  }

  if (await appHealthy()) {
    return postFalcon(path, body, timeoutMs);
  }

  await withTimeout(wakeFalcon(), 5000, 'Falcon wake');
  if (await waitForHealthy(30000)) {
    return postFalcon(path, body, timeoutMs);
  }

  setState('offline');
  throw new Error(msg('errorWaking', 'Falcon DM did not start. Open it manually.'));
}

/** Live and recent downloads for the popup. Returns null when not paired. */
async function fetchDownloads() {
  const token = await getToken();
  if (!token) return null;
  try {
    const response = await fetchWithTimeout(
      `${FALCON_API}/api/downloads`,
      { method: 'GET', headers: { 'X-Falcon-Token': token } },
      HEALTH_TIMEOUT_MS,
      'Falcon downloads',
    );
    if (response.status === 401 || response.status === 403) return null;
    if (!response.ok) return null;
    const data = await response.json();
    return data && Array.isArray(data.downloads) ? data : null;
  } catch {
    return null;
  }
}

// Loaded after media-utils.js, shared.js, pairing.js and api.js (see manifest.json).

const GRAB_BATCH_LIMIT = 100;

function setupMenus() {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: 'download_with_falcon',
      title: msg('contextDownload', 'Download with Falcon DM'),
      contexts: ['link', 'image', 'video', 'audio'],
    });
    chrome.contextMenus.create({
      id: 'grab_page_links',
      title: msg('contextGrabber', 'Grab page links with Falcon'),
      contexts: ['page'],
    });
  });
}

chrome.runtime.onInstalled.addListener(() => {
  setupMenus();
  refreshBadge();
  reinitContentScriptsAfterReload();
  // Pairing starts from the popup, where the user can see the code.
  prefsReady.then(() => ensurePaired(false).catch(() => {}));
});

chrome.runtime.onStartup.addListener(() => {
  refreshBadge();
  reinitContentScriptsAfterReload();
  prefsReady.then(() => ensurePaired(false).catch(() => {}));
});

refreshBadge();
prefsReady.then(() => ensurePaired(false).catch(() => {}));

chrome.tabs.onRemoved.addListener((tabId) => {
  MEDIA_URLS.delete(tabId);
  MEDIA_META.delete(tabId);
  INJECTED.delete(tabId);
});

function isYoutubePage(url) {
  return !!(self.FalconMedia && self.FalconMedia.isYoutubePageUrl && self.FalconMedia.isYoutubePageUrl(url));
}

chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
  if (changeInfo.url) {
    MEDIA_URLS.delete(tabId);
    MEDIA_META.delete(tabId);
    INJECTED.delete(tabId);
  }
  const url = changeInfo.url || tab?.url || '';
  if (isYoutubePage(url) && (changeInfo.status === 'complete' || !!changeInfo.url)) {
    ensureContentScript(tabId);
  }
});

function headerValue(headers, name) {
  const h = (headers || []).find((x) => x.name.toLowerCase() === name.toLowerCase());
  return h ? h.value : '';
}

function basenameFilename(raw) {
  if (!raw) return '';
  const s = String(raw).trim();
  if (!s) return '';
  const parts = s.split(/[/\\]/);
  return parts[parts.length - 1] || s;
}

function splitHeaderParameters(value) {
  const parts = [];
  let current = '';
  let quoted = false;
  let escaped = false;
  for (const ch of String(value || '')) {
    if (escaped) {
      current += ch;
      escaped = false;
      continue;
    }
    if (ch === '\\' && quoted) {
      current += ch;
      escaped = true;
      continue;
    }
    if (ch === '"') quoted = !quoted;
    if (ch === ';' && !quoted) {
      parts.push(current);
      current = '';
    } else {
      current += ch;
    }
  }
  parts.push(current);
  return parts;
}

function unquoteHeaderValue(value) {
  const trimmed = String(value || '').trim();
  if (trimmed.length >= 2 && trimmed.startsWith('"') && trimmed.endsWith('"')) {
    return trimmed.slice(1, -1).replace(/\\(.)/g, '$1');
  }
  return trimmed;
}

function decodeRfc5987Value(value) {
  const match = String(value || '').match(/^[^']*'[^']*'(.*)$/);
  const encoded = match ? match[1] : String(value || '');
  try {
    return decodeURIComponent(encoded);
  } catch (_) {
    return encoded;
  }
}

function contentDispositionFilename(header) {
  const parts = splitHeaderParameters(header);
  let plain = '';
  let extended = '';
  for (const part of parts.slice(1)) {
    const eq = part.indexOf('=');
    if (eq < 0) continue;
    const key = part.slice(0, eq).trim().toLowerCase();
    const value = unquoteHeaderValue(part.slice(eq + 1));
    if (key === 'filename*') extended = decodeRfc5987Value(value);
    else if (key === 'filename') plain = value;
  }
  return basenameFilename(extended || plain);
}

function isAttachmentDisposition(header) {
  return splitHeaderParameters(header)[0].trim().toLowerCase() === 'attachment';
}

function isPublicEnqueueUrl(url) {
  if (!url || !/^https?:/i.test(String(url))) return false;
  if (self.FalconMedia && self.FalconMedia.isPublicHttpUrl) {
    return !!self.FalconMedia.isPublicHttpUrl(url);
  }
  return false;
}

function urlBasename(url) {
  if (self.FalconMedia && self.FalconMedia.urlBasename) return self.FalconMedia.urlBasename(url);
  try {
    const name = decodeURIComponent(new URL(url).pathname.split('/').pop() || '');
    if (name) return basenameFilename(name);
  } catch (_) {}
  const last = String(url || '').split('/').pop() || '';
  return basenameFilename(last.split('?')[0]);
}

/** Fail-open: Chrome row already cancelled — start a fresh browser download we own. */
async function fallbackBrowserDownload(item, filename) {
  const name =
    basenameFilename(filename) ||
    basenameFilename(item.filename) ||
    urlBasename(item.url) ||
    'download';
  return new Promise((resolve, reject) => {
    chrome.downloads.download(
      {
        url: item.url,
        filename: name,
        conflictAction: 'uniquify',
      },
      (id) => {
        const err = chrome.runtime.lastError;
        if (err) reject(new Error(err.message));
        else resolve(id);
      },
    );
  });
}

async function getPageTitle(pageUrl) {
  if (!pageUrl) return '';
  try {
    const key = pageUrl.split('#')[0];
    const tabs = await ext.tabs.query({});
    const tab = tabs.find((t) => t.url && t.url.split('#')[0] === key);
    return (tab && tab.title) || '';
  } catch (_) {
    return '';
  }
}

async function resolveHijackFilename(item, waitMs = 0) {
  const FM = self.FalconMedia;
  const sourceUrl = item.url || '';
  const pick = (raw) => {
    const name = basenameFilename(raw);
    if (!name) return '';
    if (FM && FM.isJunkHijackFilename && FM.isJunkHijackFilename(name, sourceUrl)) return '';
    return name;
  };

  let name = pick(item.filename);
  if (name) return name;

  if (item.id != null) {
    const deadline = Date.now() + waitMs;
    for (;;) {
      try {
        const rows = await ext.downloads.search({ id: item.id });
        const row = rows && rows[0];
        name = pick(row && row.filename);
        if (name) return name;
      } catch (_) {}
      const remaining = deadline - Date.now();
      if (remaining <= 0) break;
      const changed = await waitForFilenameChange(item.id, Math.min(120, remaining));
      name = pick(changed);
      if (name) return name;
    }
  }

  try {
    const seg = decodeURIComponent(new URL(sourceUrl).pathname.split('/').pop() || '');
    if (seg.includes('.') && (!FM || !FM.isJunkHijackFilename || !FM.isJunkHijackFilename(seg, sourceUrl))) {
      return seg;
    }
  } catch (_) {}

  return '';
}

async function buildHijackEnqueue(item, opts = {}) {
  const pageUrl = item.referrer || item.url;
  const FM = self.FalconMedia;
  const hijack = FM && FM.hijackPayloadForFalcon
    ? FM.hijackPayloadForFalcon(item.url, pageUrl)
    : { url: item.url, format: null };
  const urlRewritten = hijack.url !== item.url;

  let filename =
    takePendingChromeFilename(item) ||
    (await resolveHijackFilename(item, opts.waitForFilename ? FILENAME_WAIT_MS : 0));

  // Title rewrite only when URL becomes a YouTube watch page and Chrome had no real name.
  if (
    urlRewritten &&
    (!filename || (FM && FM.isJunkHijackFilename && FM.isJunkHijackFilename(filename, item.url))) &&
    FM &&
    FM.defaultFilename
  ) {
    const pageTitle = await getPageTitle(pageUrl);
    const fileExt =
      FM.guessExtensionFromDownloadUrl ? FM.guessExtensionFromDownloadUrl(item.url) : 'mp4';
    filename = FM.defaultFilename(pageTitle, { ext: fileExt });
  }

  if (!filename) {
    filename = urlBasename(item.url) || 'download';
  }

  return { pageUrl, filename, hijack, urlRewritten };
}

const hijackDedup = new Map();
/** Sync capture from onDeterminingFilename — async runHijack must not lose Chrome's name. */
const pendingChromeNames = new Map();
const abortedBrowserIds = new Set();
const hijackWatchIds = new Set();
const hijackInFlight = new Set();
const deferredPurgeIds = new Set();
/** onCreated often fires before Chrome knows the real filename — wait for onDeterminingFilename. */
const awaitingFilename = new Map(); // id -> timeoutId
const HIJACK_DEDUP_MS = 8000;
const FILENAME_WAIT_MS = 450;
const filenameWaiters = new Map(); // Firefox fallback: id -> pending onChanged watchers

function waitForFilenameChange(id, timeoutMs) {
  return new Promise((resolve) => {
    let waiters = filenameWaiters.get(id);
    if (!waiters) {
      waiters = new Set();
      filenameWaiters.set(id, waiters);
    }
    const waiter = { resolve, timer: 0 };
    waiter.timer = setTimeout(() => {
      const current = filenameWaiters.get(id);
      if (current) {
        current.delete(waiter);
        if (!current.size) filenameWaiters.delete(id);
      }
      resolve('');
    }, timeoutMs);
    waiters.add(waiter);
  });
}

function notifyFilenameWaiters(delta) {
  const name = delta && delta.filename && delta.filename.current;
  if (!name) return;
  const waiters = filenameWaiters.get(delta.id);
  if (!waiters) return;
  filenameWaiters.delete(delta.id);
  for (const waiter of waiters) {
    clearTimeout(waiter.timer);
    waiter.resolve(name);
  }
}

function hijackKey(item) {
  if (item && item.id != null) return `id:${item.id}`;
  return `${item.url || ''}\0${item.referrer || ''}`;
}

function automaticCaptureAllowed(item) {
  try {
    return shouldCaptureBrowserDownload(item, {
      extensions: captureExtensions,
      blockedHosts: captureBlockedHosts,
    });
  } catch (_) {
    return false;
  }
}

function hijackRecentlyHandled(item) {
  const prev = hijackDedup.get(hijackKey(item));
  return !!(prev && Date.now() - prev < HIJACK_DEDUP_MS);
}

function markHijackStarted(item) {
  hijackDedup.set(hijackKey(item), Date.now());
}

function takePendingChromeFilename(item) {
  if (item.id == null) return '';
  const name = pendingChromeNames.get(item.id) || '';
  if (name) pendingChromeNames.delete(item.id);
  return name;
}

function clearAwaitingFilename(id) {
  if (id == null) return;
  const t = awaitingFilename.get(id);
  if (t) clearTimeout(t);
  awaitingFilename.delete(id);
}

function scheduleCreatedBackup(item) {
  if (item.id == null) return;
  clearAwaitingFilename(item.id);
  const timeoutId = setTimeout(() => {
    awaitingFilename.delete(item.id);
    if (interceptPaused) return;
    if (!automaticCaptureAllowed(item)) return;
    if (hijackWatchIds.has(item.id) || abortedBrowserIds.has(item.id)) return;
    if (hijackInFlight.has(hijackKey(item))) return;
    markHijackStarted(item);
    stashPendingChromeFilename(item);
    abortAndEraseBrowserDownload(item.id);
    runHijack(item, { skipDedup: true }).catch((err) => console.error(err));
  }, FILENAME_WAIT_MS);
  awaitingFilename.set(item.id, timeoutId);
}

function stashPendingChromeFilename(item) {
  if (item.id == null) return;
  const name = basenameFilename(item.filename);
  if (name) pendingChromeNames.set(item.id, name);
}

/** Delete Chrome history row + any file already written to disk. */
function purgeBrowserDownload(id) {
  if (id == null) return;
  ext.downloads
    .search({ id })
    .then((rows) => {
      const row = rows && rows[0];
      if (!row) {
        abortedBrowserIds.delete(id);
        hijackWatchIds.delete(id);
        return;
      }
      const finish = () => {
        eraseBrowserDownload(id);
        abortedBrowserIds.delete(id);
        hijackWatchIds.delete(id);
      };
      if (row.state === 'complete') {
        chrome.downloads.removeFile(id, () => {
          void chrome.runtime.lastError;
          finish();
        });
        return;
      }
      if (row.state === 'in_progress') {
        chrome.downloads.cancel(id, () => {
          void chrome.runtime.lastError;
          finish();
        });
        return;
      }
      finish();
    })
    .catch(() => {
      abortedBrowserIds.delete(id);
      hijackWatchIds.delete(id);
    });
}

/** Cancellation without purge: Firefox reads the row briefly for a late filename. */
function cancelBrowserDownloadOnly(id) {
  if (id == null) return;
  hijackWatchIds.add(id);
  if (abortedBrowserIds.has(id)) return;
  abortedBrowserIds.add(id);
  chrome.downloads.cancel(id, () => void chrome.runtime.lastError);
}

/** Cancel the browser download immediately (IDM-style). purge deletes partial/complete files. */
function abortAndEraseBrowserDownload(id) {
  if (id == null) return;
  hijackWatchIds.add(id);
  if (abortedBrowserIds.has(id)) {
    purgeBrowserDownload(id);
    return;
  }
  abortedBrowserIds.add(id);
  chrome.downloads.cancel(id, () => {
    void chrome.runtime.lastError;
    purgeBrowserDownload(id);
  });
}

/**
 * IDM model: Chrome never owns the file.
 * 1) Caller already cancelled Chrome (or will).
 * 2) Enqueue Falcon.
 * 3) Fail-open restarts a NEW Chrome download (resume is useless after cancel).
 */
async function runHijack(item, opts = {}) {
  const key = hijackKey(item);
  if (hijackInFlight.has(key)) return;
  // Listeners pass skipDedup after markHijackStarted — must not early-return.
  const skipDedup = !!(opts.skipDedup || opts.skipDedupCheck);
  if (!skipDedup && hijackRecentlyHandled(item)) return;
  if (!skipDedup) markHijackStarted(item);

  hijackInFlight.add(key);
  const failClosed = await getInterceptFailClosed();
  const deferBrowserPurge = !!opts.deferBrowserPurge;
  const finishBrowserDownload = () => {
    if (!deferBrowserPurge) abortAndEraseBrowserDownload(item.id);
  };
  let filename = '';

  try {
    const built = await buildHijackEnqueue(item, opts);
    filename = built.filename;
    const { pageUrl, hijack } = built;
    const cookieLookup = cookieLookupUrl(item.url, pageUrl);
    const cookiesHeader = await getCookiesHeader(cookieLookup);

    await sendToFalcon('/api/add', {
      url: hijack.url,
      filename,
      referrer: item.referrer || '',
      user_agent: navigator.userAgent,
      cookies: cookiesHeader,
      cookie_url: cookieLookup,
      format: hijack.format || undefined,
    });
    finishBrowserDownload();
    if (notifySuccess) {
      notify(
        'Falcon DM',
        msg('interceptQueued', 'Browser download cancelled — queued in Falcon DM'),
      );
    }
  } catch (e) {
    console.error(e);
    if (failClosed) {
      finishBrowserDownload();
      notify(
        'Falcon DM',
        e.message || msg('interceptBlocked', 'Falcon DM offline — download blocked'),
      );
    } else {
      finishBrowserDownload();
      try {
        await fallbackBrowserDownload(item, filename);
        notify(
          'Falcon DM',
          msg('appClosedFallback', 'Falcon DM offline — browser download kept'),
        );
      } catch (fallbackErr) {
        console.error(fallbackErr);
        notify(
          'Falcon DM',
          fallbackErr.message || msg('errorAppOffline', 'Falcon DM offline — download failed'),
        );
      }
    }
  } finally {
    if (deferBrowserPurge) {
      deferredPurgeIds.delete(item.id);
      purgeBrowserDownload(item.id);
    }
    hijackInFlight.delete(key);
  }
}

const supportsDeterminingFilename = !!(
  chrome.downloads && chrome.downloads.onDeterminingFilename
);
const isFirefoxRuntime =
  typeof browser !== 'undefined' &&
  !!(browser.runtime && typeof browser.runtime.getBrowserInfo === 'function');
const useFirefoxBlockingCapture = isFirefoxRuntime && !supportsDeterminingFilename;

/* Types Firefox cannot normally render inline. Renderable pdf/video/audio stay out:
   navigation alone is not proof that the user asked for a download. */
const DOWNLOADABLE_RESPONSE_TYPES = new Set([
  'application/octet-stream',
  'binary/octet-stream',
  'application/x-binary',
  'application/x-download',
  'application/x-unknown-content-type',
  'application/zip',
  'application/x-zip-compressed',
  'application/x-7z-compressed',
  'application/x-rar-compressed',
  'application/vnd.rar',
  'application/x-tar',
  'application/x-gtar',
  'application/gzip',
  'application/x-gzip',
  'application/x-bzip',
  'application/x-bzip2',
  'application/x-xz',
  'application/x-msdownload',
  'application/x-ms-dos-executable',
  'application/vnd.microsoft.portable-executable',
  'application/x-msi',
  'application/x-ms-installer',
  'application/x-apple-diskimage',
  'application/x-debian-package',
  'application/vnd.debian.binary-package',
  'application/x-rpm',
  'application/x-iso9660-image',
  'application/x-bittorrent',
]);
const FIREFOX_DOCUMENT_REQUEST_TYPES = new Set(['main_frame', 'sub_frame', 'object']);

function isDownloadableResponseType(contentType) {
  const base = String(contentType || '').split(';')[0].trim().toLowerCase();
  return DOWNLOADABLE_RESPONSE_TYPES.has(base);
}

function firefoxCaptureItemFromResponse(details) {
  if (!details || details.tabId == null || details.tabId < 0) return null;
  if (!FIREFOX_DOCUMENT_REQUEST_TYPES.has(details.type || 'main_frame')) return null;
  const url = details.url || '';
  if (!isPublicEnqueueUrl(url)) return null;

  const disposition = headerValue(details.responseHeaders, 'content-disposition');
  const type = headerValue(details.responseHeaders, 'content-type');
  const attachment = isAttachmentDisposition(disposition);
  if (!attachment && !isDownloadableResponseType(type)) return null;

  const item = {
    url,
    finalUrl: url,
    referrer: details.originUrl || details.documentUrl || '',
    filename: contentDispositionFilename(disposition) || urlBasename(url),
  };
  return automaticCaptureAllowed(item) ? item : null;
}

async function captureFirefoxResponse(details) {
  try {
    await prefsReady;
    if (interceptPaused) return {};
    const item = firefoxCaptureItemFromResponse(details);
    if (!item) return {};
    if (hijackRecentlyHandled(item) || hijackInFlight.has(hijackKey(item))) {
      // Redirect/duplicate bytes must not become a second browser copy.
      return { cancel: true };
    }
    markHijackStarted(item);
    runHijack(item, { skipDedup: true }).catch((err) => console.error(err));
    return { cancel: true };
  } catch (err) {
    console.error(err);
    return {};
  }
}

if (useFirefoxBlockingCapture) {
  chrome.webRequest.onHeadersReceived.addListener(
    captureFirefoxResponse,
    { urls: ['<all_urls>'], types: ['main_frame', 'sub_frame', 'object'] },
    ['blocking', 'responseHeaders'],
  );
}

async function notifyOverlayMedia(tabId) {
  if (!tabId || tabId < 0) return;
  const injected = await ensureContentScript(tabId);
  if (!injected) return;
  try {
    await ext.tabs.sendMessage(tabId, { action: 'media_updated' });
  } catch (_) {
    /* ponytail: tab may be navigating — FAB resyncs on next sniff */
  }
}

chrome.webRequest.onHeadersReceived.addListener(
  (details) => {
    const url = details.url;
    const ct = headerValue(details.responseHeaders, 'content-type').toLowerCase();
    const cl = parseInt(headerValue(details.responseHeaders, 'content-length'), 10) || 0;

    if (
      !self.FalconMedia ||
      !self.FalconMedia.shouldSniffInject(url, ct, cl)
    ) {
      return;
    }

    const clean = self.FalconMedia.normalizeMediaUrl(url);
    if (!clean || self.FalconMedia.isJunkUrl(clean)) return;
    if (
      self.FalconMedia.isGooglevideoUrl(clean) &&
      !self.FalconMedia.isDirectGooglevideoUrl(clean)
    ) {
      return;
    }

    if (details.tabId > -1) {
      const set = MEDIA_URLS.get(details.tabId) || new Set();
      set.add(clean);
      MEDIA_URLS.set(details.tabId, set);

      const metaMap = MEDIA_META.get(details.tabId) || new Map();
      const prev = metaMap.get(clean);
      metaMap.set(clean, {
        contentLength: Math.max(cl, (prev && prev.contentLength) || 0),
        contentType: ct || (prev && prev.contentType) || '',
        ts: Date.now(),
      });
      MEDIA_META.set(details.tabId, metaMap);

      notifyOverlayMedia(details.tabId);
    }
  },
  { urls: ['<all_urls>'] },
  ['responseHeaders'],
);

if (chrome.downloads.onDeterminingFilename)
  chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  const run = () => {
    if (interceptPaused) {
      suggest();
      return;
    }
    // Fail-open fallback uses chrome.downloads.download — must not re-hijack our own item.
    if (isOwnBrowserDownload(item)) {
      suggest();
      return;
    }
    if (!isPublicEnqueueUrl(item.url)) {
      suggest();
      return;
    }
    if (!automaticCaptureAllowed(item)) {
      clearAwaitingFilename(item.id);
      suggest();
      return;
    }
    clearAwaitingFilename(item.id);
    // Chrome's real name is available HERE — stash before any cancel/erase.
    stashPendingChromeFilename(item);
    if (hijackWatchIds.has(item.id) || abortedBrowserIds.has(item.id) || hijackInFlight.has(hijackKey(item))) {
      suggest();
      abortAndEraseBrowserDownload(item.id);
      return;
    }
    markHijackStarted(item);
    suggest();
    // IDM: cancel Chrome first, then hand URL to Falcon with Chrome's filename.
    abortAndEraseBrowserDownload(item.id);
    runHijack(item, { skipDedup: true }).catch((err) => console.error(err));
  };
  if (!prefsHydrated) {
    prefsReady.then(run).catch(() => {
      try {
        suggest();
      } catch (_) {}
    });
    return true;
  }
  run();
});

function captureFirefoxCreatedDownload(item) {
  markHijackStarted(item);
  stashPendingChromeFilename(item);
  if (item.id != null) deferredPurgeIds.add(item.id);
  // Cancel before any filename wait: Firefox may already be writing partial bytes.
  cancelBrowserDownloadOnly(item.id);
  runHijack(item, {
    skipDedup: true,
    waitForFilename: true,
    deferBrowserPurge: true,
  }).catch((err) => console.error(err));
}

// onCreated fires before filename is known — never enqueue here (wrong URL basename).
chrome.downloads.onCreated.addListener((item) => {
  if (isOwnBrowserDownload(item)) return;
  if (!isPublicEnqueueUrl(item.url)) return;
  if (hijackInFlight.has(hijackKey(item)) || hijackWatchIds.has(item.id) || abortedBrowserIds.has(item.id)) {
    abortAndEraseBrowserDownload(item.id);
    return;
  }
  (async () => {
    await prefsReady;
    if (interceptPaused) return;
    if (!automaticCaptureAllowed(item)) return;
    if (hijackWatchIds.has(item.id) || abortedBrowserIds.has(item.id) || hijackInFlight.has(hijackKey(item))) {
      abortAndEraseBrowserDownload(item.id);
      return;
    }
    if (supportsDeterminingFilename) {
      if (connectionState !== 'connected' && !(await appHealthy())) return;
      // Wait briefly for onDeterminingFilename to supply the server filename.
      scheduleCreatedBackup(item);
      return;
    }
    captureFirefoxCreatedDownload(item);
  })().catch((err) => console.error(err));
});

chrome.downloads.onChanged.addListener((delta) => {
  notifyFilenameWaiters(delta);
  if (delta.id == null || !hijackWatchIds.has(delta.id)) return;
  const state = delta.state && delta.state.current;
  if ((state === 'complete' || state === 'interrupted') && !deferredPurgeIds.has(delta.id)) {
    purgeBrowserDownload(delta.id);
  }
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === 'download_with_falcon') {
    const url = info.linkUrl || info.srcUrl;
    if (!url) return;
    if (!isPublicEnqueueUrl(url)) {
      notify('Falcon DM', msg('errorInvalidUrl', 'No valid URL'));
      return;
    }
    if (self.FalconMedia && self.FalconMedia.isJunkUrl && self.FalconMedia.isJunkUrl(url)) {
      notify('Falcon DM', msg('errorJunkUrl', 'Not a valid media URL'));
      return;
    }
    const filename = urlBasename(url) || 'download';
    try {
      const cookieLookup = cookieLookupUrl(url, info.pageUrl || url);
      const cookiesHeader = await getCookiesHeader(cookieLookup);
      await sendToFalcon('/api/add', {
        url,
        filename,
        referrer: info.pageUrl || '',
        user_agent: navigator.userAgent,
        cookies: cookiesHeader,
        cookie_url: cookieLookup,
      });
      if (notifySuccess) notify('Falcon DM', msg('sentToApp', 'Download sent to Falcon DM'));
    } catch (e) {
      console.error(e);
      notify('Falcon DM', e.message || msg('errorAppOffline', 'Failed'));
    }
  }

  if (info.menuItemId === 'grab_page_links' && tab?.id) {
    ensureContentScript(tab.id).then((ok) => {
      if (!ok) {
        notify('Falcon DM', msg('errorMediaUtils', 'Cannot run on this page'));
        return;
      }
      chrome.tabs.sendMessage(tab.id, { action: 'open_grabber' }, () => {
        if (chrome.runtime.lastError) {
          notify(
            'Falcon DM',
            chrome.runtime.lastError.message || msg('errorMediaUtils', 'Cannot run on this page'),
          );
        }
      });
    });
  }
});

chrome.runtime.onMessage.addListener((request, sender, sendResponse) => {
  // --- popup: live status + queue preview ---
  if (request.action === 'check_status') {
    (async () => {
      await prefsReady;
      const status = () => ({
        state: connectionState,
        paused: interceptPaused,
        failClosed: interceptFailClosed,
        notifySuccess,
        pairCode: formatPairCode(visiblePairCode()),
        extensionId: extensionOriginId(),
      });
      if (!(await appHealthy())) {
        setState('offline');
        sendResponse(status());
        return;
      }
      const token = await getToken();
      if (token) {
        const code = await pingToken(token);
        if (code >= 200 && code < 300) setState('connected');
        else if (code === 0) setState('offline');
        // 401/403: the token is stale or revoked — pair again with a fresh prompt.
        else ensurePaired(false).catch(() => {});
      } else if (connectionState !== 'blocked') {
        ensurePaired(false).catch(() => {});
        // Give the first pair request a moment so the popup can show the code.
        await sleep(400);
      }
      sendResponse(status());
    })();
    return true;
  }

  if (request.action === 'get_downloads') {
    (async () => {
      sendResponse(await fetchDownloads());
    })();
    return true;
  }

  if (request.action === 'get_status') {
    (async () => {
      await prefsReady;
      sendResponse({
        state: connectionState,
        paused: interceptPaused,
        failClosed: interceptFailClosed,
      });
    })();
    return true;
  }

  if (request.action === 'set_paused') {
    const next = !!request.paused;
    // Respond immediately, then durably persist so an MV3 SW restart keeps the
    // user's hijack preference (previously a SW restart would reset to false).
    markPauseWrite();
    interceptPaused = next;
    ext.storage.local.set({ falconInterceptPaused: next }).catch(() => {});
    ext.storage.session.remove('falconInterceptPaused').catch(() => {});
    sendResponse({ ok: true, paused: interceptPaused });
    return true;
  }

  if (request.action === 'set_fail_closed') {
    (async () => {
      try {
        const next = await setInterceptFailClosed(!!request.failClosed);
        sendResponse({ ok: true, failClosed: next });
      } catch (e) {
        sendResponse({ ok: false, error: e.message || 'Failed' });
      }
    })();
    return true;
  }

  if (request.action === 'set_notify_success') {
    (async () => {
      try {
        const next = await setNotifySuccessEnabled(!!request.enabled);
        sendResponse({ ok: true, enabled: next });
      } catch (e) {
        sendResponse({ ok: false, error: e.message || 'Failed' });
      }
    })();
    return true;
  }

  if (request.action === 'grab_tab_media') {
    (async () => {
      try {
        const fromPopup = !sender.tab;
        const requested = Number(request.tabId);
        const active = fromPopup && Number.isInteger(requested) && requested > 0
          ? { id: requested }
          : sender.tab || (await getActiveBrowserTab());
        if (!active || !active.id) {
          sendResponse({
            ok: false,
            error: msg('errorNoActiveTab', 'No browser tab — click the page, then try again'),
          });
          return;
        }
        const injected = await ensureContentScript(active.id);
        if (!injected) {
          sendResponse({ ok: false, error: msg('errorMediaUtils', 'Cannot run on this page') });
          return;
        }
        const overlay = request.mode === 'links' ? 'open_grabber' : 'open_download_modal';
        chrome.tabs.sendMessage(active.id, { action: overlay }, (resp) => {
          if (chrome.runtime.lastError) {
            sendResponse({ ok: false, error: chrome.runtime.lastError.message });
          } else if (resp && resp.ok === false) {
            sendResponse({ ok: false, error: resp.error || msg('errorAppOffline', 'Failed') });
          } else {
            sendResponse({ ok: true });
          }
        });
      } catch (e) {
        sendResponse({ ok: false, error: e.message });
      }
    })();
    return true;
  }

  // --- popup: download an arbitrary URL via /api/add ---
  if (request.action === 'add_url') {
    const rawUrl = (request.url || '').trim();
    if (!rawUrl || !/^https?:/i.test(rawUrl) || !isPublicEnqueueUrl(rawUrl)) {
      sendResponse({ success: false, error: msg('errorInvalidUrl', 'No valid URL') });
      return true;
    }
    (async () => {
      try {
        const tab = await getActiveBrowserTab();
        const pageUrl = (tab && tab.url) || rawUrl;
        const cookieLookup = cookieLookupUrl(rawUrl, pageUrl);
        const cookies = await getCookiesHeader(cookieLookup);
        await sendToFalcon('/api/add', {
          url: rawUrl,
          filename: urlBasename(rawUrl) || 'download',
          referrer: pageUrl,
          user_agent: navigator.userAgent,
          cookies,
          cookie_url: cookieLookup,
        });
        sendResponse({ success: true });
      } catch (e) {
        sendResponse({ success: false, error: e.message });
      }
    })();
    return true;
  }

  if (request.action === 'get_real_media_url') {
    const tabId = sender.tab ? sender.tab.id : -1;
    const set = MEDIA_URLS.get(tabId);
    const urls = set ? Array.from(set) : [];
    const pageUrl = (sender.tab && sender.tab.url) || request.page_url || '';
    const title = (sender.tab && sender.tab.title) || '';

    const metaMap = {};
    const rawMeta = MEDIA_META.get(tabId);
    if (rawMeta) {
      rawMeta.forEach((v, k) => {
        metaMap[k] = v;
      });
    }

    const finish = async (extraUrls, captions) => {
      const merged = [...new Set([...(urls || []), ...(extraUrls || [])])];
      const cookiePage = (pageUrl || '').split('#')[0];
      const cookies = cookiePage ? await getCookiesHeader(cookiePage) : '';
      sendResponse({
        url: merged[merged.length - 1] || null,
        urls: merged,
        metaMap,
        title,
        cookies,
        userAgent: navigator.userAgent,
        captions: Array.isArray(captions) ? captions.slice(0, 64) : [],
      });
    };

    // Pull YouTube player_response from MAIN world (isolated world cannot see it)
    const isYt = isYoutubePage(pageUrl) && tabId > -1 && ext.scripting;
    if (isYt) {
      ext.scripting
        .executeScript({
          target: { tabId },
          world: 'MAIN',
          func: () => {
            const out = [];
            const captions = [];
            const seenCaptions = new Set();
            const captionLabel = (track) => {
              const name = track && track.name;
              if (!name) return '';
              if (typeof name.simpleText === 'string') return name.simpleText;
              if (Array.isArray(name.runs)) return name.runs.map((run) => run.text || '').join('');
              return '';
            };
            const pushPr = (pr) => {
              if (!pr) return;
              if (pr.streamingData) {
                for (const f of pr.streamingData.formats || []) if (f.url) out.push(f.url);
                for (const f of pr.streamingData.adaptiveFormats || []) if (f.url) out.push(f.url);
              }
              for (const track of pr.captions?.playerCaptionsTracklistRenderer?.captionTracks || []) {
                const lang = String(track.languageCode || '').trim();
                if (!track.baseUrl || !lang) continue;
                const kind = typeof track.kind === 'string' && track.kind ? track.kind : null;
                const key = `${lang}|${kind || ''}`;
                if (seenCaptions.has(key)) continue;
                seenCaptions.add(key);
                captions.push({
                  lang,
                  kind,
                  label: captionLabel(track),
                  baseUrl: track.baseUrl,
                });
              }
            };
            try {
              pushPr(window.ytInitialPlayerResponse);
            } catch (_) {}
            try {
              const cfg = window.ytplayer?.config?.args;
              let raw = cfg?.player_response || cfg?.raw_player_response;
              if (typeof raw === 'string') raw = JSON.parse(raw);
              pushPr(raw);
            } catch (_) {}
            try {
              const el = document.getElementById('movie_player');
              if (el && typeof el.getPlayerResponse === 'function') {
                pushPr(el.getPlayerResponse());
              }
            } catch (_) {}
            return { urls: out, captions };
          },
        })
        .then((results) => {
          const result = (results && results[0] && results[0].result) || {};
          // Older mocked/test responses are bare arrays of URLs.
          const extra = Array.isArray(result) ? result : result.urls || [];
          finish(extra, Array.isArray(result) ? [] : result.captions || []);
        })
        .catch(() => finish([]));
      return true;
    }

    finish([]);
    return true;
  }

  if (request.action === 'check_connection') {
    (async () => {
      try {
        await ensureAppRunning();
        await ensurePaired(true);
        await postFalcon('/api/ping', {});
        sendResponse({ ok: true });
      } catch (e) {
        sendResponse({ ok: false, error: e.message });
      }
    })();
    return true;
  }

  if (request.action === 'auto_pair') {
    if (connectionState === 'blocked') setState('offline');
    ensureAppRunning()
      .then(() => ensurePaired(true))
      .then(() => sendResponse({ ok: true }))
      .catch((e) => sendResponse({ ok: false, error: e.message }));
    return true;
  }

  if (request.action === 'download_blob') {
    const blobUrl = (request.blob_url || request.url || '').trim();
    const tabId = sender.tab && sender.tab.id;
    if (!blobUrl.startsWith('blob:') || tabId == null) {
      sendResponse({
        success: false,
        error: msg(
          'errorNoValidSource',
          'No downloadable URL found — play the video and try again',
        ),
      });
      return true;
    }

    (async () => {
      try {
        const payload = await fetchBlobPayload(tabId, blobUrl);
        if (!payload.ok) throw new Error(payload.error || 'blob fetch failed');
        const pageUrl = (sender.tab && sender.tab.url) || request.page_url || '';
        await sendToFalcon(
          '/api/upload',
          {
            filename: basenameFilename(request.filename) || 'download.mp4',
            data_base64: payload.base64,
            page_url: pageUrl || null,
          },
          120000,
        );
        sendResponse({ success: true });
      } catch (err) {
        sendResponse({ success: false, error: err.message });
      }
    })().catch((err) => sendResponse({ success: false, error: err.message }));
    return true;
  }

  if (request.action === 'download_video' || request.action === 'download_url') {
    const rawUrl = (request.url || '').trim();
    if (!rawUrl || rawUrl.startsWith('blob:')) {
      sendResponse({
        success: false,
        error: msg(
          'errorNoValidSource',
          'No downloadable URL found — play the video and try again',
        ),
      });
      return true;
    }

    if (!isPublicEnqueueUrl(rawUrl)) {
      sendResponse({
        success: false,
        error: msg('errorJunkUrl', 'Not a valid media URL'),
      });
      return true;
    }
    if (self.FalconMedia.isJunkUrl(rawUrl)) {
      sendResponse({
        success: false,
        error: msg(
          'errorJunkUrl',
          'Not a real video URL — play the video and pick a quality card',
        ),
      });
      return true;
    }

    const url =
      self.FalconMedia.normalizeMediaUrl
        ? self.FalconMedia.normalizeMediaUrl(rawUrl)
        : rawUrl;

    (async () => {
      const tabPage = (sender.tab && sender.tab.url) || '';
      const pageUrl = (tabPage || request.page_url || '').trim();
      const cookieLookup = cookieLookupUrl(url, pageUrl);
      const cookies = await getCookiesHeader(cookieLookup);
      sendToFalcon('/api/intercept', {
        url,
        page_url: pageUrl || null,
        title: request.title || '',
        cookies,
        cookie_url: cookieLookup,
        user_agent: request.user_agent || navigator.userAgent,
        referer: pageUrl || null,
        filename: request.filename || null,
        media_type: request.media_type || 'application/octet-stream',
        format: request.format || null,
        subtitle_tracks: Array.isArray(request.subtitle_tracks)
          ? request.subtitle_tracks
              .slice(0, 32)
              .map((track) => ({
                lang: String((track && track.lang) || '').slice(0, 40),
                kind: typeof track?.kind === 'string' && track.kind ? track.kind.slice(0, 16) : undefined,
              }))
              .filter((track) => track.lang)
          : undefined,
      })
        .then((data) => sendResponse({ success: true, data }))
        .catch((err) => sendResponse({ success: false, error: err.message }));
    })().catch((err) => sendResponse({ success: false, error: err.message }));
    return true;
  }

  if (request.action === 'batch_download') {
    const items = (request.items || []).slice(0, GRAB_BATCH_LIMIT);
    (async () => {
      const settled = await Promise.allSettled(
        items.map(async (it) => {
          try {
            if (!isPublicEnqueueUrl(it.url)) {
              throw new Error(msg('errorJunkUrl', 'Not a valid media URL'));
            }
            if (self.FalconMedia.isJunkUrl(it.url)) {
              throw new Error(msg('errorJunkUrl', 'Not a valid media URL'));
            }
            const pageUrl = (request.page_url || '').trim();
            const cookieLookup = cookieLookupUrl(it.url, pageUrl);
            const cookies = await getCookiesHeader(cookieLookup);
            const data = await sendToFalcon('/api/add', {
              url: it.url,
              filename: it.filename || urlBasename(it.url) || 'download',
              referrer: pageUrl || '',
              user_agent: navigator.userAgent,
              cookies,
              cookie_url: cookieLookup,
            });
            return {
              url: it.url,
              ok: true,
              id: data?.id || data?.download?.id,
            };
          } catch (error) {
            return {
              url: it.url,
              ok: false,
              error: error?.message || String(error),
            };
          }
        }),
      );
      const results = settled.map((result, index) =>
        result.status === 'fulfilled'
          ? result.value
          : {
              url: items[index].url,
              ok: false,
              error: result.reason?.message || String(result.reason),
            },
      );
      sendResponse({
        success: results.every((result) => result.ok),
        count: items.length,
        results,
      });
    })().catch((err) => sendResponse({ success: false, error: err.message }));
    return true;
  }
});
