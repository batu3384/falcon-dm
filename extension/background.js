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

    const finish = async (extraUrls) => {
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
            const pushPr = (pr) => {
              if (!pr || !pr.streamingData) return;
              for (const f of pr.streamingData.formats || []) if (f.url) out.push(f.url);
              for (const f of pr.streamingData.adaptiveFormats || []) if (f.url) out.push(f.url);
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
            return out;
          },
        })
        .then((results) => {
          const extra = (results && results[0] && results[0].result) || [];
          finish(extra);
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
