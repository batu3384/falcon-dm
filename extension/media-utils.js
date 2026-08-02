/* Shared media URL analysis — loaded in background + content scripts */
(function (root) {
  function _i18n(key, fb) {
    try {
      if (typeof chrome !== "undefined" && chrome.i18n && chrome.i18n.getMessage) {
        return chrome.i18n.getMessage(key) || fb;
      }
    } catch (_) {}
    return fb;
  }

  const YT_ITAG = {
    // Progressive (video+audio muxed) — preferred for direct download
    18: { q: "360p", fmt: "MP4", type: "video", muxed: true },
    22: { q: "720p", fmt: "MP4", type: "video", muxed: true },
    37: { q: "1080p", fmt: "MP4", type: "video", muxed: true },
    38: { q: "3072p", fmt: "MP4", type: "video", muxed: true },
    43: { q: "360p", fmt: "WebM", type: "video", muxed: true },
    44: { q: "480p", fmt: "WebM", type: "video", muxed: true },
    45: { q: "720p", fmt: "WebM", type: "video", muxed: true },
    46: { q: "1080p", fmt: "WebM", type: "video", muxed: true },
    // Adaptive video-only
    133: { q: "240p", fmt: "MP4", type: "video", muxed: false },
    134: { q: "360p", fmt: "MP4", type: "video", muxed: false },
    135: { q: "480p", fmt: "MP4", type: "video", muxed: false },
    136: { q: "720p", fmt: "MP4", type: "video", muxed: false },
    137: { q: "1080p", fmt: "MP4", type: "video", muxed: false },
    138: { q: "2160p", fmt: "MP4", type: "video", muxed: false },
    160: { q: "144p", fmt: "MP4", type: "video", muxed: false },
    242: { q: "240p", fmt: "WebM", type: "video", muxed: false },
    243: { q: "360p", fmt: "WebM", type: "video", muxed: false },
    244: { q: "480p", fmt: "WebM", type: "video", muxed: false },
    247: { q: "720p", fmt: "WebM", type: "video", muxed: false },
    248: { q: "1080p", fmt: "WebM", type: "video", muxed: false },
    271: { q: "1440p", fmt: "WebM", type: "video", muxed: false },
    272: { q: "2160p", fmt: "WebM", type: "video", muxed: false },
    298: { q: "720p60", fmt: "MP4", type: "video", muxed: false },
    299: { q: "1080p60", fmt: "MP4", type: "video", muxed: false },
    302: { q: "720p60", fmt: "WebM", type: "video", muxed: false },
    303: { q: "1080p60", fmt: "WebM", type: "video", muxed: false },
    398: { q: "720p", fmt: "MP4", type: "video", muxed: false },
    399: { q: "1080p", fmt: "MP4", type: "video", muxed: false },
    400: { q: "1440p", fmt: "MP4", type: "video", muxed: false },
    401: { q: "2160p", fmt: "MP4", type: "video", muxed: false },
    // Audio-only
    139: { q: "48k", fmt: "M4A", type: "audio", muxed: false },
    140: { q: "128k", fmt: "M4A", type: "audio", muxed: false },
    141: { q: "256k", fmt: "M4A", type: "audio", muxed: false },
    171: { q: "128k", fmt: "WebM", type: "audio", muxed: false },
    249: { q: "50k", fmt: "Opus", type: "audio", muxed: false },
    250: { q: "70k", fmt: "Opus", type: "audio", muxed: false },
    251: { q: "160k", fmt: "Opus", type: "audio", muxed: false },
  };

  const JUNK_RE =
    /no_input\.mp3|\/s\/search\/|\/generate_204|\/ptracking|\/api\/stats|\/log_event|\/youtubei\/|\/timedtext|\/caption|\/ad_status|\/pagead\/|\/doubleclick|favicon|\/img\/|\.svg(\?|$)|\/static\/|\/yts\/|\/s\/player\/|\/jsbin\//i;

  function formatBytes(n) {
    if (!n || n <= 0) return "";
    const u = ["B", "KB", "MB", "GB"];
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

  /** Strip byte-range / segment params so full file downloads. */
  function normalizeMediaUrl(raw) {
    if (!raw || typeof raw !== "string") return "";
    try {
      const u = new URL(raw);
      [
        "range",
        "rn",
        "rbuf",
        "sq",
        "ump",
        "alr",
        "keepalive",
        "cmo",
      ].forEach((k) => u.searchParams.delete(k));
      return u.toString();
    } catch {
      return raw.split("&range=")[0].split("?range=")[0];
    }
  }

  function isJunkUrl(url) {
    if (!url || url.startsWith("blob:") || url.startsWith("data:")) return true;
    if (JUNK_RE.test(url)) return true;
    // YouTube UI placeholder audio
    if (url.includes("youtube.com") && url.includes(".mp3") && !url.includes("videoplayback")) {
      return true;
    }
    return false;
  }

  function isCapturableMedia(url, contentType) {
    if (isJunkUrl(url)) return false;
    const ct = (contentType || "").toLowerCase();
    const u = url.toLowerCase();
    return (
      u.includes(".m3u8") ||
      u.includes("/manifest/") ||
      u.includes("videoplayback") ||
      u.includes("googlevideo.com") ||
      u.includes("mime=video") ||
      u.includes("mime=audio") ||
      /\.(mp4|webm|mkv|m4a|mp3|flac|ogg|mov)(\?|#|$)/i.test(url) ||
      ct.includes("video/") ||
      ct.includes("audio/") ||
      ct.includes("mpegurl") ||
      ct.includes("application/vnd.apple.mpegurl")
    );
  }

  function detectFormat(url) {
    const u = url.toLowerCase();
    if (u.includes(".m3u8") || u.includes("/manifest/")) return { fmt: "HLS", type: "stream" };
    if (u.includes(".mpd")) return { fmt: "DASH", type: "stream" };
    if (u.includes(".webm")) return { fmt: "WebM", type: "video" };
    if (u.includes(".mp4") || u.includes("mime=video%2fmp4") || u.includes("mime=video/mp4")) {
      return { fmt: "MP4", type: "video" };
    }
    if (u.includes("mime=audio")) return { fmt: "Audio", type: "audio" };
    if (u.includes("videoplayback") || u.includes("googlevideo")) return { fmt: "MP4", type: "video" };
    return { fmt: "Media", type: "video" };
  }

  function detectQuality(url, meta) {
    const itag = parseItag(url);
    if (itag && YT_ITAG[itag]) return YT_ITAG[itag].q;
    const u = url.toLowerCase();
    const res = u.match(/(\d{3,4})p/);
    if (res) return res[1] + "p";
    if (u.includes("1080")) return "1080p";
    if (u.includes("720")) return "720p";
    if (u.includes("480")) return "480p";
    if (meta && meta.contentLength > 0) return formatBytes(meta.contentLength);
    return "";
  }

  function hostLabel(url) {
    try {
      const h = new URL(url).hostname.replace(/^www\./, "");
      if (h.includes("googlevideo")) return "YouTube CDN";
      if (h.includes("youtube")) return "YouTube";
      return h;
    } catch {
      return "Media";
    }
  }

  function extFromUrl(url, meta, fmt, isHls, isAudio) {
    if (isHls) return "mp4";
    const pathMatch = url.match(/\.([a-z0-9]{2,5})(?:\?|#|$)/i);
    if (pathMatch) {
      const e = pathMatch[1].toLowerCase();
      if (!["com", "net", "org", "php", "htm", "html", "asp"].includes(e)) return e;
    }
    const ct = ((meta && meta.contentType) || "").toLowerCase();
    if (ct.includes("webm")) return "webm";
    if (ct.includes("mp4") || ct.includes("mpeg4")) return "mp4";
    if (ct.includes("mpegurl")) return "mp4";
    if (ct.includes("matroska")) return "mkv";
    if (ct.includes("quicktime")) return "mov";
    if (ct.includes("ogg")) return "ogg";
    if (ct.includes("flac")) return "flac";
    if (ct.includes("mpeg") && ct.includes("audio")) return "mp3";
    if (isAudio) {
      if (fmt.fmt === "WebM" || fmt.fmt === "Opus") return "webm";
      return "m4a";
    }
    if (fmt.fmt === "WebM") return "webm";
    if (fmt.fmt && !["Media", "HLS", "DASH", "Audio"].includes(fmt.fmt)) {
      return fmt.fmt.toLowerCase();
    }
    return "bin";
  }

  function analyzeUrl(url, meta) {
    meta = meta || {};
    const clean = normalizeMediaUrl(url);
    const itag = parseItag(clean);
    const yt = itag ? YT_ITAG[itag] : null;
    const fmt = yt ? { fmt: yt.fmt, type: yt.type } : detectFormat(clean);
    const quality = yt ? yt.q : detectQuality(clean, meta);
    const isHls = clean.includes(".m3u8") || clean.includes("/manifest/");
    const isAudio = fmt.type === "audio" || (yt && yt.type === "audio");
    const muxed = yt ? !!yt.muxed : !isAudio && !isHls;
    const size = meta.contentLength || 0;

    let score = 0;
    if (muxed && !isAudio) score += 20000; // progressive muxed wins
    if (isHls) score += 8000;
    if (!isAudio) score += 4000;
    else score -= 2000;
    const qn = parseInt(String(quality), 10) || 0;
    score += qn;
    if (size > 0) score += Math.min(Math.log10(size) * 100, 500);

    const kind = isHls
      ? _i18n("kindStream", "Stream")
      : isAudio
        ? _i18n("kindAudio", "Audio")
        : muxed
          ? _i18n("kindVideoAudio", "Video+Audio")
          : _i18n("kindVideoOnly", "Video only");
    const title = [quality, fmt.fmt, kind].filter(Boolean).join(" · ");
    const subtitle = hostLabel(clean);
    const ext = extFromUrl(clean, meta, fmt, isHls, isAudio);

    return {
      url: clean,
      title,
      subtitle,
      quality,
      format: fmt.fmt,
      mediaType: fmt.type,
      isHls,
      isAudio,
      muxed,
      size,
      sizeLabel: size > 0 ? formatBytes(size) : "",
      score,
      ext,
      itag,
    };
  }

  function groupSources(urls, metaMap) {
    const seen = new Set();
    const items = [];
    for (const raw of urls || []) {
      if (!raw || isJunkUrl(raw)) continue;
      const clean = normalizeMediaUrl(raw);
      if (!clean || seen.has(clean)) continue;
      // Dedupe by itag when present (same quality, different CDN hosts)
      const itag = parseItag(clean);
      const dedupeKey = itag ? `itag:${itag}` : clean;
      if (seen.has(dedupeKey)) continue;
      seen.add(clean);
      seen.add(dedupeKey);

      const meta = (metaMap && (metaMap[raw] || metaMap[clean])) || {};
      const isYt = clean.includes("googlevideo") || clean.includes("videoplayback");
      if (
        meta.contentLength > 0 &&
        meta.contentLength < 2048 &&
        !clean.includes(".m3u8") &&
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
    let base = (pageTitle || "download")
      .replace(/\s*-\s*YouTube\s*$/i, "")
      .replace(/[^\w\s\u00C0-\u024F.-]/g, "")
      .trim()
      .slice(0, 80);
    if (!base) base = "download";
    const ext = item ? item.ext : "bin";
    if (/\.[a-z0-9]{2,5}$/i.test(base)) return base;
    return `${base}.${ext}`;
  }

  root.FalconMedia = {
    analyzeUrl,
    groupSources,
    pickBest,
    defaultFilename,
    formatBytes,
    normalizeMediaUrl,
    isJunkUrl,
    isCapturableMedia,
    YT_ITAG,
  };
})(typeof globalThis !== "undefined" ? globalThis : window);
