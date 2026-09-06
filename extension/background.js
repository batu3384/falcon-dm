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
  ensurePaired(false).catch(() => {});
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

chrome.webRequest.onHeadersReceived.addListener(
  (details) => {
    const url = details.url;
    const ct = headerValue(details.responseHeaders, 'content-type').toLowerCase();
    const cl = parseInt(headerValue(details.responseHeaders, 'content-length'), 10) || 0;

    if (!self.FalconMedia || !self.FalconMedia.isCapturableMedia(url, ct)) return;

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
      // Keep largest content-length seen for this URL
      metaMap.set(clean, {
        contentLength: Math.max(cl, (prev && prev.contentLength) || 0),
        contentType: ct || (prev && prev.contentType) || '',
        ts: Date.now(),
      });
      MEDIA_META.set(details.tabId, metaMap);

      // Inject the on-demand overlay so the user sees the Falcon button on this media.
      ensureContentScript(details.tabId);
    }
  },
  { urls: ['<all_urls>'] },
  ['responseHeaders'],
);

chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  if (interceptPaused) {
    // Paused: let the browser handle the download natively.
    suggest({ cancel: false });
    return;
  }
  (async () => {
    try {
      const pageUrl = item.referrer || item.url;
      const cookieLookup = cookieLookupUrl(item.url, pageUrl);
      const cookiesHeader = await getCookiesHeader(cookieLookup);

      const filename = item.filename || item.url.split('/').pop().split('?')[0] || 'download';

      await sendToFalcon('/api/add', {
        url: item.url,
        filename,
        referrer: item.referrer || '',
        user_agent: navigator.userAgent,
        cookies: cookiesHeader,
        cookie_url: cookieLookup,
      });
      suggest({ cancel: true });
      notify('Falcon DM', msg('sentToApp', 'Download sent to Falcon DM'));
    } catch (e) {
      console.error(e);
      const failClosed = await getInterceptFailClosed();
      suggest({ cancel: failClosed });
      notify(
        'Falcon DM',
        failClosed
          ? msg('interceptBlocked', 'Falcon DM offline — download blocked')
          : e.message || msg('appClosedFallback', 'Falcon DM offline — browser download kept'),
      );
    }
  })();
  return true;
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === 'download_with_falcon') {
    const url = info.linkUrl || info.srcUrl;
    if (!url) return;
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
    sendResponse({
      state: connectionState,
      paused: interceptPaused,
      failClosed: interceptFailClosed,
      recent: RECENT.slice(0, 3),
    });
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

  // --- popup: download current tab's media (inject overlay + open picker) ---
  if (request.action === 'grab_tab_media') {
    (async () => {
      try {
        const [active] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
        if (!active || !active.id) {
          sendResponse({ ok: false, error: msg('errorInvalidUrl', 'No active tab') });
          return;
        }
        const injected = await ensureContentScript(active.id);
        if (!injected) {
          sendResponse({ ok: false, error: msg('errorMediaUtils', 'Cannot run on this page') });
          return;
        }
        chrome.tabs.sendMessage(active.id, { action: 'open_download_modal' }, () => {
          if (chrome.runtime.lastError) {
            sendResponse({ ok: false, error: chrome.runtime.lastError.message });
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
        const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
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
      sendResponse({
        url: merged[merged.length - 1] || null,
        urls: merged,
        metaMap,
        title,
        cookies: '',
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
