importScripts('media-utils.js', 'shared.js', 'pairing.js', 'api.js');

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
  chrome.alarms.create('falconClipboard', { periodInMinutes: 1 });
  reinitContentScriptsAfterReload();
  ensurePaired(true).catch(() => {
    setState('offline');
    notify(
      msg('appName', 'Falcon DM'),
      msg('popupOnboard', 'Open Falcon DM and approve this extension in Settings'),
    );
  });
});

chrome.runtime.onStartup.addListener(() => {
  refreshBadge();
  reinitContentScriptsAfterReload();
  ensurePaired(false).catch(() => {});
  chrome.alarms.create('falconClipboard', { periodInMinutes: 1 });
});

chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === 'falconClipboard') {
    pollClipboardToFalcon().catch(() => {});
  }
});

chrome.tabs.onActivated.addListener(() => {
  pollClipboardToFalcon().catch(() => {});
});

refreshBadge();
ensurePaired(false).catch(() => {});

chrome.tabs.onRemoved.addListener((tabId) => {
  MEDIA_URLS.delete(tabId);
  MEDIA_META.delete(tabId);
  INJECTED.delete(tabId);
});

function isYoutubePage(url) {
  if (!url) return false;
  try {
    const h = new URL(url).hostname.toLowerCase();
    return (
      h === 'youtu.be' ||
      h === 'youtube.com' ||
      h.endsWith('.youtube.com') ||
      h.endsWith('.youtube-nocookie.com')
    );
  } catch {
    return /youtube\.com|youtu\.be/i.test(url);
  }
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

/** Fail-open: Chrome row already cancelled — start a fresh browser download we own. */
async function fallbackBrowserDownload(item, filename) {
  const name =
    basenameFilename(filename) ||
    basenameFilename(item.filename) ||
    basenameFilename(item.url.split('/').pop().split('?')[0]) ||
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
    const tabs = await chrome.tabs.query({});
    const tab = tabs.find((t) => t.url && t.url.split('#')[0] === key);
    return (tab && tab.title) || '';
  } catch (_) {
    return '';
  }
}

async function resolveHijackFilename(item) {
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
    try {
      const rows = await chrome.downloads.search({ id: item.id });
      const row = rows && rows[0];
      name = pick(row && row.filename);
      if (name) return name;
    } catch (_) {}
  }

  try {
    const seg = decodeURIComponent(new URL(sourceUrl).pathname.split('/').pop() || '');
    if (seg.includes('.') && (!FM || !FM.isJunkHijackFilename || !FM.isJunkHijackFilename(seg, sourceUrl))) {
      return seg;
    }
  } catch (_) {}

  return '';
}

async function buildHijackEnqueue(item) {
  const pageUrl = item.referrer || item.url;
  const FM = self.FalconMedia;
  const hijack = FM && FM.hijackPayloadForFalcon
    ? FM.hijackPayloadForFalcon(item.url, pageUrl)
    : { url: item.url, format: null };
  const urlRewritten = hijack.url !== item.url;

  let filename = takePendingChromeFilename(item) || (await resolveHijackFilename(item));

  // Title rewrite only when URL becomes a YouTube watch page and Chrome had no real name.
  if (
    urlRewritten &&
    (!filename || (FM && FM.isJunkHijackFilename && FM.isJunkHijackFilename(filename, item.url))) &&
    FM &&
    FM.defaultFilename
  ) {
    const pageTitle = await getPageTitle(pageUrl);
    const ext =
      FM.guessExtensionFromDownloadUrl ? FM.guessExtensionFromDownloadUrl(item.url) : 'mp4';
    filename = FM.defaultFilename(pageTitle, { ext });
  }

  if (!filename) {
    try {
      filename = decodeURIComponent(new URL(item.url).pathname.split('/').pop() || '');
    } catch (_) {
      filename = basenameFilename(item.url.split('/').pop().split('?')[0]) || 'download';
    }
  }

  return { pageUrl, filename, hijack, urlRewritten };
}

const hijackDedup = new Map();
/** Sync capture from onDeterminingFilename — async runHijack must not lose Chrome's name. */
const pendingChromeNames = new Map();
const abortedBrowserIds = new Set();
const hijackWatchIds = new Set();
const hijackInFlight = new Set();
/** onCreated often fires before Chrome knows the real filename — wait for onDeterminingFilename. */
const awaitingFilename = new Map(); // id -> timeoutId
const HIJACK_DEDUP_MS = 8000;
const FILENAME_WAIT_MS = 450;

function hijackKey(item) {
  return `${item.url}\0${item.referrer || ''}`;
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
    if (hijackRecentlyHandled(item) || hijackInFlight.has(hijackKey(item))) return;
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
  chrome.downloads
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

/** Cancel Chrome immediately (IDM-style). purge deletes any partial/complete file. */
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
  let filename = '';

  try {
    const built = await buildHijackEnqueue(item);
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
    abortAndEraseBrowserDownload(item.id);
    notify(
      'Falcon DM',
      msg('interceptQueued', 'Browser download cancelled — queued in Falcon DM'),
    );
  } catch (e) {
    console.error(e);
    if (failClosed) {
      abortAndEraseBrowserDownload(item.id);
      notify('Falcon DM', msg('interceptBlocked', 'Falcon DM offline — download blocked'));
    } else {
      abortAndEraseBrowserDownload(item.id);
      try {
        await fallbackBrowserDownload(item, filename);
      } catch (fallbackErr) {
        console.error(fallbackErr);
      }
      notify(
        'Falcon DM',
        e.message || msg('appClosedFallback', 'Falcon DM offline — browser download kept'),
      );
    }
  } finally {
    hijackInFlight.delete(key);
  }
}

async function notifyOverlayMedia(tabId) {
  if (!tabId || tabId < 0) return;
  const injected = await ensureContentScript(tabId);
  if (!injected) return;
  try {
    await chrome.tabs.sendMessage(tabId, { action: 'media_updated' });
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

chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  if (interceptPaused) {
    suggest();
    return;
  }
  // Fail-open fallback uses chrome.downloads.download — must not re-hijack our own item.
  if (item.byExtensionId === chrome.runtime.id) {
    suggest();
    return;
  }
  clearAwaitingFilename(item.id);
  // Chrome's real name is available HERE — stash before any cancel/erase.
  stashPendingChromeFilename(item);
  if (hijackRecentlyHandled(item) || hijackInFlight.has(hijackKey(item))) {
    suggest();
    abortAndEraseBrowserDownload(item.id);
    return;
  }
  markHijackStarted(item);
  suggest();
  // IDM: cancel Chrome first, then hand URL to Falcon with Chrome's filename.
  abortAndEraseBrowserDownload(item.id);
  runHijack(item, { skipDedup: true }).catch((err) => console.error(err));
});

// onCreated fires before filename is known — never enqueue here (wrong URL basename).
chrome.downloads.onCreated.addListener((item) => {
  if (interceptPaused) return;
  if (item.byExtensionId === chrome.runtime.id) return;
  const key = hijackKey(item);
  if (hijackInFlight.has(key) || hijackRecentlyHandled(item) || hijackWatchIds.has(item.id)) {
    abortAndEraseBrowserDownload(item.id);
    return;
  }
  (async () => {
    if (connectionState !== 'connected' && !(await appHealthy())) return;
    // Wait briefly for onDeterminingFilename to supply the server filename.
    scheduleCreatedBackup(item);
  })().catch((err) => console.error(err));
});

chrome.downloads.onChanged.addListener((delta) => {
  if (delta.id == null || !hijackWatchIds.has(delta.id)) return;
  const state = delta.state && delta.state.current;
  if (state === 'complete' || state === 'interrupted') {
    purgeBrowserDownload(delta.id);
  }
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === 'download_with_falcon') {
    const url = info.linkUrl || info.srcUrl;
    if (!url) return;
    if (self.FalconMedia && self.FalconMedia.isJunkUrl && self.FalconMedia.isJunkUrl(url)) {
      notify('Falcon DM', msg('errorJunkUrl', 'Not a valid media URL'));
      return;
    }
    const filename = url.split('/').pop().split('?')[0] || 'download';
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
      notify('Falcon DM', msg('sentToApp', 'Download sent to Falcon DM'));
    } catch (e) {
      console.error(e);
      notify('Falcon DM', e.message || msg('errorAppOffline', 'Failed'));
    }
  }

  if (info.menuItemId === 'grab_page_links' && tab?.id) {
    ensureContentScript(tab.id).then(() => {
      chrome.tabs.sendMessage(tab.id, { action: 'open_grabber' });
    });
  }
});

chrome.runtime.onMessage.addListener((request, sender, sendResponse) => {
  // --- popup: live status + queue preview ---
  if (request.action === 'check_status') {
    (async () => {
      const healthy = await appHealthy();
      if (!healthy) {
        setState('offline');
        sendResponse({
          state: connectionState,
          paused: interceptPaused,
          failClosed: interceptFailClosed,
          recent: RECENT.slice(0, 3),
          extensionId: chrome.runtime.id,
        });
        return;
      }
      const token = await getToken();
      if (token) {
        try {
          const r = await fetchWithTimeout(
            `${FALCON_API}/api/ping`,
            {
              method: 'POST',
              headers: {
                'Content-Type': 'application/json',
                'X-Falcon-Token': token,
              },
              body: '{}',
            },
            REQUEST_TIMEOUT_MS,
            'Falcon ping',
          );
          if (r.ok) setState('connected');
          else if (r.status === 403) setState('blocked');
          else if (r.status === 401) setState('pending');
          else setState('offline');
        } catch {
          setState('offline');
        }
      } else {
        setState('pending');
        ensurePaired(false).catch(() => {});
      }
      sendResponse({
        state: connectionState,
        paused: interceptPaused,
        failClosed: interceptFailClosed,
        recent: RECENT.slice(0, 3),
        extensionId: chrome.runtime.id,
      });
    })();
    return true;
  }

  if (request.action === 'save_token') {
    (async () => {
      const token = (request.token || '').trim();
      if (!token) {
        sendResponse({ ok: false, error: msg('errorTokenEmpty', 'Token is empty') });
        return;
      }
      await chrome.storage.local.set({ apiToken: token });
      try {
        const r = await fetchWithTimeout(
          `${FALCON_API}/api/ping`,
          {
            method: 'POST',
            headers: {
              'Content-Type': 'application/json',
              'X-Falcon-Token': token,
            },
            body: '{}',
          },
          REQUEST_TIMEOUT_MS,
          'Falcon ping',
        );
        if (r.ok) {
          setState('connected');
          sendResponse({ ok: true, state: 'connected' });
        } else if (r.status === 403) {
          setState('blocked');
          sendResponse({
            ok: false,
            state: 'blocked',
            error: msg(
              'errorExtensionBlocked',
              'Extension blocked — approve in Falcon DM Settings',
            ),
            extensionId: chrome.runtime.id,
          });
        } else {
          setState('pending');
          sendResponse({ ok: false, error: msg('errorTokenInvalid', 'Invalid token') });
        }
      } catch (e) {
        setState('offline');
        sendResponse({ ok: false, error: e.message || msg('errorAppOffline', 'Failed') });
      }
    })();
    return true;
  }

  if (request.action === 'get_status') {
    (async () => {
      sendResponse({
        state: connectionState,
        paused: interceptPaused,
        failClosed: interceptFailClosed,
        clipboardMonitor: await getClipboardMonitorEnabled(),
        recent: RECENT.slice(0, 3),
      });
    })();
    return true;
  }

  if (request.action === 'set_paused') {
    const next = !!request.paused;
    // Respond immediately, then durably persist so an MV3 SW restart keeps the
    // user's hijack preference (previously a SW restart would reset to false).
    interceptPaused = next;
    chrome.storage.session.set({ falconInterceptPaused: next }).catch(() => {});
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

  if (request.action === 'set_clipboard_monitor') {
    (async () => {
      try {
        const next = await setClipboardMonitorEnabled(!!request.enabled);
        sendResponse({ ok: true, clipboardMonitor: next });
      } catch (e) {
        sendResponse({ ok: false, error: e.message || 'Failed' });
      }
    })();
    return true;
  }

  // --- popup: download current tab's media (inject overlay + open picker) ---
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
        chrome.tabs.sendMessage(active.id, { action: 'open_download_modal' }, (resp) => {
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
    if (!rawUrl || !/^https?:/i.test(rawUrl)) {
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
          filename: rawUrl.split('/').pop().split('?')[0] || 'download',
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
    const isYt = /youtube\.com|youtu\.be/i.test(pageUrl) && tabId > -1 && chrome.scripting;
    if (isYt) {
      chrome.scripting
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
        await postFalcon(
          '/api/upload',
          {
            filename: request.filename || 'download.mp4',
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

    if (self.FalconMedia && self.FalconMedia.isJunkUrl(rawUrl)) {
      sendResponse({
        success: false,
        error: msg(
          'errorJunkUrl',
          'Bu adres gerçek video değil (YouTube UI sesi) — videoyu oynatıp tekrar dene',
        ),
      });
      return true;
    }

    const url =
      self.FalconMedia && self.FalconMedia.normalizeMediaUrl
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
            if (self.FalconMedia && self.FalconMedia.isJunkUrl && self.FalconMedia.isJunkUrl(it.url)) {
              throw new Error(msg('errorJunkUrl', 'Not a valid media URL'));
            }
            const pageUrl = (request.page_url || '').trim();
            const cookieLookup = cookieLookupUrl(it.url, pageUrl);
            const cookies = await getCookiesHeader(cookieLookup);
            const data = await sendToFalcon('/api/add', {
              url: it.url,
              filename: it.filename || it.url.split('/').pop().split('?')[0] || 'download',
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
