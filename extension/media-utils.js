/* Shared media URL analysis — loaded in background + content scripts */
(function (root) {
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

  const JUNK_RE =
    /no_input\.mp3|\/s\/search\/|\/generate_204|\/ptracking|\/api\/stats|\/log_event|\/youtubei\/|\/timedtext|\/caption|\/ad_status|\/pagead\/|\/doubleclick|favicon|\/img\/|\.svg(\?|$)|\/static\/|\/yts\/|\/s\/player\/|\/jsbin\//i;

  // ponytail: keep in sync with backend guess_extension + DownloadCategory
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
    while (v >= 1024 && i < u.length - 1) {
      v /= 1024;
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
    if (JUNK_RE.test(url)) return true;
    // YouTube UI placeholder audio
    if (url.includes('youtube.com') && url.includes('.mp3') && !url.includes('videoplayback')) {
      return true;
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
    const u = url.toLowerCase();
    return (
      u.includes('videoplayback') ||
      isGooglevideoUrl(url) ||
      u.includes('mime=video') ||
      u.includes('mime=audio') ||
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

  /** Align hijacked browser downloads with overlay/yt-dlp routing. */
  function hijackPayloadForFalcon(downloadUrl, pageUrl) {
    const raw = String(downloadUrl || '').trim();
    const page = String(pageUrl || '').split('#')[0];
    if (isGooglevideoUrl(raw) || raw.includes('videoplayback')) {
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

  function isMainYoutubePlayerVideo(video) {
    if (video.closest('ytd-ad-slot-renderer, .ytp-ad-module, .video-ads')) return false;
    return !!video.closest('#movie_player, .html5-video-player, ytd-player');
  }

  function videoElementSrc(el) {
    if (!el) return '';
    return el.currentSrc || el.src || '';
  }

  function isYoutubeWatchPage(url) {
    if (isYoutubeWatchUrl(url)) return true;
    try {
      const u = new URL(url);
      return u.hostname === 'youtu.be' || u.pathname.startsWith('/shorts/');
    } catch {
      return false;
    }
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
      if (h.includes('googlevideo')) return 'YouTube CDN';
      if (h.includes('youtube')) return 'YouTube';
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
    // Prefer muxed progressive video
    const muxed = items.filter((i) => i.muxed && !i.isAudio);
    if (muxed.length) return muxed[0];
    const pool = preferVideo !== false ? items.filter((i) => !i.isAudio) : items;
    return (pool.length ? pool : items)[0];
  }

  function defaultFilename(pageTitle, item) {
    let base = (pageTitle || 'download')
      .replace(/\s*-\s*YouTube\s*$/i, '')
      .replace(/[^\w\s\u00C0-\u024F.-]/g, '')
      .trim()
      .slice(0, 80);
    if (!base) base = 'download';
    const ext = item ? item.ext : 'bin';
    if (/\.[a-z0-9]{2,5}$/i.test(base)) return base;
    return `${base}.${ext}`;
  }

  /** yt-dlp tiers when CDN capture is empty (modern YouTube uses SABR/cipher). */
  function youtubeFallbackSources(pageUrl) {
    const watch = String(pageUrl || '').split('#')[0];
    try {
      const u = new URL(watch);
      if (!isYoutubeHost(u.hostname)) return [];
      if (!isYoutubeWatchUrl(watch) && u.hostname !== 'youtu.be' && !u.pathname.startsWith('/shorts/')) {
        return [];
      }
    } catch (_) {
      return [];
    }
    const tiers = [
      { height: 360, label: '360' },
      { height: 480, label: '480' },
      { height: 720, label: '720' },
      { height: 1080, label: '1080' },
    ];
    const hint = _i18n('youtubeHint', 'YouTube via yt-dlp');
    return tiers.map(({ height, label }) => ({
      url: watch,
      title: `${height}p · MP4 · ${_i18n('kindVideoAudio', 'Video+Audio')}`,
      subtitle: hint,
      quality: `${height}p`,
      format: 'MP4',
      muxed: false,
      isAudio: false,
      isHls: false,
      height,
      label,
      score: height,
      ext: 'mp4',
    }));
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
    formatBytes,
    normalizeMediaUrl,
    isJunkUrl,
    isBlobUrl,
    isGrabberLink,
    GRABBER_EXT_RE,
    BLOB_MAX_BYTES,
    collectPageBlobSources,
    isCapturableMedia,
    isSniffableMedia,
    shouldSniffInject,
    isEligibleVideoElement,
    isYoutubeWatchPage,
    isMainYoutubePlayerVideo,
    hijackPayloadForFalcon,
    videoElementSrc,
    MIN_FAB_PX,
    isYoutubeHost,
    isGooglevideoUrl,
    isDirectGooglevideoUrl,
    isYoutubeWatchUrl,
    youtubeFallbackSources,
    YT_ITAG,
  };
})(typeof globalThis !== 'undefined' ? globalThis : window);
