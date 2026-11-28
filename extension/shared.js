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

