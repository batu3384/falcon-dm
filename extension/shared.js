const FALCON_API = 'http://127.0.0.1:14201';
const WAKE_URL = 'falcondm://wake';
const MEDIA_URLS = new Map();
const MEDIA_META = new Map();
let pairInFlight = null;
const REQUEST_TIMEOUT_MS = 10000;
const HEALTH_TIMEOUT_MS = 4000;
const PAIR_POLL_ATTEMPTS = 15;

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
// Pause + fail-closed live in chrome.storage.local so Chrome restart keeps them.
let connectionState = 'offline'; // "connected" | "pending" | "offline"
let interceptPaused = false; // when true, automatic hijack is off (browser downloads natively)
let interceptFailClosed = true; // default fail-closed: Chrome does not keep hijacked files
const RECENT = []; // recent sends shown in the popup queue preview
const INJECTED = new Set(); // tab ids that already have the on-demand content script

// Writers set dirty so an in-flight hydrate get() cannot clobber a live toggle.
let pauseDirty = false;
let failClosedDirty = false;
let prefsHydrated = false;

function markPauseWrite() {
  pauseDirty = true;
}

function markFailClosedWrite() {
  failClosedDirty = true;
}

const prefsReady = (async () => {
  try {
    const { falconConnectionState } = await chrome.storage.session.get({
      falconConnectionState: 'offline',
    });
    const { falconInterceptFailClosed } = await chrome.storage.local.get({
      falconInterceptFailClosed: true,
    });
    const localPauseBag = await chrome.storage.local.get('falconInterceptPaused');
    const hasLocalPause = Object.hasOwn(localPauseBag, 'falconInterceptPaused');
    // Session is a one-shot migrate from the old SW-only pause. Explicit local
    // false (Resume) must win — defaulted get() treated Resume as "missing".
    let paused = hasLocalPause ? !!localPauseBag.falconInterceptPaused : false;
    if (!hasLocalPause) {
      const sessionPause = await chrome.storage.session.get({ falconInterceptPaused: false });
      paused = !!sessionPause.falconInterceptPaused;
      if (paused) {
        chrome.storage.local.set({ falconInterceptPaused: true }).catch(() => {});
      }
    }
    chrome.storage.session.remove('falconInterceptPaused').catch(() => {});
    if (!pauseDirty) {
      interceptPaused = paused;
    }
    if (!failClosedDirty) interceptFailClosed = !!falconInterceptFailClosed;
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
    const { falconInterceptFailClosed } = await chrome.storage.local.get({
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
  await chrome.storage.local.set({ falconInterceptFailClosed: interceptFailClosed });
  return interceptFailClosed;
}

function setState(s) {
  if (connectionState === s) return;
  connectionState = s;
  refreshBadge();
  // Best-effort persist (non-blocking); SW restart will restore the badge.
  chrome.storage.session.set({ falconConnectionState: connectionState }).catch(() => {});
}

/** Reflect connection state on the toolbar icon badge. */
function refreshBadge() {
  const map = {
    connected: { text: '✓', color: '#22c55e' },
    pending: { text: '•', color: '#D97706' },
    blocked: { text: '!', color: '#dc2626' },
    offline: { text: '', color: '#dc2626' },
  };
  const b = map[connectionState] || map.offline;
  try {
    chrome.action.setBadgeBackgroundColor({ color: b.color });
    chrome.action.setBadgeText({ text: b.text });
  } catch (_) {}
}

function trackDownload(filename, url, kind) {
  let host = '';
  try {
    host = new URL(url).hostname.replace(/^www\./, '');
  } catch (_) {}
  RECENT.unshift({
    filename: (filename || 'download').slice(0, 80),
    host,
    ts: Date.now(),
    kind,
  });
  if (RECENT.length > 8) RECENT.length = 8;
}

/** Popup window is last-focused while open — never query lastFocusedWindow for page tabs. */
async function getActiveBrowserTab() {
  try {
    const win = await chrome.windows.getLastFocused({ windowTypes: ['normal'] });
    if (win && win.id != null) {
      const [tab] = await chrome.tabs.query({ active: true, windowId: win.id });
      if (tab) return tab;
    }
  } catch (_) {}
  try {
    const [tab] = await chrome.tabs.query({ active: true, windowType: 'normal' });
    if (tab) return tab;
  } catch (_) {}
  return null;
}

/** Inject the on-demand content script once per tab. Idempotent. */
async function resetContentScriptGuard(tabId) {
  try {
    await chrome.scripting.executeScript({
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
    const tabs = await chrome.tabs.query({
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
    await chrome.tabs.sendMessage(tabId, { action: 'ping' });
    INJECTED.add(tabId);
    return true;
  } catch (_) {
    INJECTED.delete(tabId);
  }
  INJECTED.add(tabId);
  try {
    await resetContentScriptGuard(tabId);
    await chrome.scripting.executeScript({
      target: { tabId },
      files: ['media-utils.js', 'content.js'],
    });
    await chrome.tabs.sendMessage(tabId, { action: 'ping' });
    return true;
  } catch (_) {
    INJECTED.delete(tabId); // allow retry (e.g. chrome:// pages reject injection)
    return false;
  }
}

async function getToken() {
  const { apiToken } = await chrome.storage.local.get({ apiToken: '' });
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
    const cookies = await withTimeout(chrome.cookies.getAll({ url }), 3000, 'Cookie lookup');
    if (cookies.length) {
      return cookies.map((c) => `${c.name}=${c.value}`).join('; ');
    }
  } catch (_) {}
  return '';
}

function notify(title, message) {
  chrome.notifications.create({
    type: 'basic',
    iconUrl: 'icon128.png',
    title,
    message,
  });
}

const CLIPBOARD_URL_RE = /^https?:\/\/\S+/i;
let lastClipboardUrl = '';

(async () => {
  try {
    const { falconLastClipboardUrl } = await chrome.storage.local.get({
      falconLastClipboardUrl: '',
    });
    lastClipboardUrl = String(falconLastClipboardUrl || '');
  } catch (_) {}
})();

async function rememberClipboardUrl(url) {
  lastClipboardUrl = url;
  try {
    await chrome.storage.local.set({ falconLastClipboardUrl: url });
  } catch (_) {}
}

function isQueueableClipboardUrl(url) {
  if (!CLIPBOARD_URL_RE.test(url)) return false;
  if (url.length > 8192) return false;
  if (String(url).toLowerCase().startsWith('magnet:')) return false;
  const fm = self.FalconMedia;
  if (!fm || !fm.isPublicHttpUrl || !fm.isPublicHttpUrl(url)) return false;
  if (fm.isJunkUrl && fm.isJunkUrl(url)) return false;
  return true;
}

async function getClipboardMonitorEnabled() {
  try {
    const { falconClipboardMonitor } = await chrome.storage.local.get({
      falconClipboardMonitor: false,
    });
    return !!falconClipboardMonitor;
  } catch (_) {
    return false;
  }
}

async function setClipboardMonitorEnabled(next) {
  await chrome.storage.local.set({ falconClipboardMonitor: !!next });
  return !!next;
}

async function fetchBlobPayload(tabId, blobUrl) {
  const maxBytes =
    (self.FalconMedia && self.FalconMedia.BLOB_MAX_BYTES) || 32 * 1024 * 1024;
  const [{ result }] = await chrome.scripting.executeScript({
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

async function pollClipboardToFalcon() {
  if (!(await getClipboardMonitorEnabled())) return;
  if (connectionState !== 'connected') return;
  if (!(await appHealthy())) return;
  try {
    const text = String(await navigator.clipboard.readText()).trim();
    if (!isQueueableClipboardUrl(text) || text === lastClipboardUrl) return;
    // Remember first so overlapping alarm/tab polls cannot double-POST.
    await rememberClipboardUrl(text);
    try {
      const filename =
        (self.FalconMedia && self.FalconMedia.urlBasename && self.FalconMedia.urlBasename(text)) ||
        'download';
      await sendToFalcon('/api/add', {
        url: text,
        filename,
        referrer: '',
        user_agent: navigator.userAgent,
        cookies: '',
      });
      notify(msg('appName', 'Falcon DM'), msg('clipboardQueued', 'URL from clipboard queued'));
    } catch (_) {
      lastClipboardUrl = '';
      try {
        await chrome.storage.local.remove('falconLastClipboardUrl');
      } catch (_) {}
    }
  } catch (_) {
    /* ponytail: clipboard may be denied without focus — skip quietly */
  }
}
