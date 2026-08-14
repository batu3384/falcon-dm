const FALCON_API = "http://127.0.0.1:14201";
const WAKE_URL = "falcondm://wake";
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
  return withTimeout(
    fetch(url, { ...options, signal: controller.signal }),
    timeoutMs,
    label
  ).catch((error) => {
    controller.abort();
    throw error;
  });
}

// Connection / UI state shared with popup + toolbar badge.
// ponytail: MV3 service workers are killed after ~30s of inactivity and these
// in-memory values reset on restart — most importantly `interceptPaused`, which
// is a durable user preference. We mirror it (and connectionState) into
// chrome.storage.session so a SW restart restores the user's choice instead of
// silently re-enabling download hijacking.
let connectionState = "offline"; // "connected" | "pending" | "offline"
let interceptPaused = false; // when true, automatic hijack is off (browser downloads natively)
const RECENT = []; // recent sends shown in the popup queue preview
const INJECTED = new Set(); // tab ids that already have the on-demand content script

// On SW startup, hydrate the durable preference from session storage.
(async () => {
  try {
    const { falconInterceptPaused, falconConnectionState } =
      await chrome.storage.session.get({
        falconInterceptPaused: false,
        falconConnectionState: "offline",
      });
    interceptPaused = !!falconInterceptPaused;
    connectionState = falconConnectionState || "offline";
    refreshBadge();
  } catch (_) {}
})();

function setState(s) {
  if (connectionState === s) return;
  connectionState = s;
  refreshBadge();
  // Best-effort persist (non-blocking); SW restart will restore the badge.
  chrome.storage.session
    .set({ falconConnectionState: connectionState })
    .catch(() => {});
}

/** Reflect connection state on the toolbar icon badge. */
function refreshBadge() {
  const map = {
    connected: { text: "✓", color: "#22c55e" },
    pending: { text: "•", color: "#D97706" },
    offline: { text: "", color: "#dc2626" },
  };
  const b = map[connectionState] || map.offline;
  try {
    chrome.action.setBadgeBackgroundColor({ color: b.color });
    chrome.action.setBadgeText({ text: b.text });
  } catch (_) {}
}

function trackDownload(filename, url, kind) {
  let host = "";
  try {
    host = new URL(url).hostname.replace(/^www\./, "");
  } catch (_) {}
  RECENT.unshift({
    filename: (filename || "download").slice(0, 80),
    host,
    ts: Date.now(),
    kind,
  });
  if (RECENT.length > 8) RECENT.length = 8;
}

/** Inject the on-demand content script once per tab. Idempotent. */
async function ensureContentScript(tabId) {
  if (!tabId || tabId < 0 || INJECTED.has(tabId)) return true;
  INJECTED.add(tabId);
  try {
    await chrome.scripting.executeScript({
      target: { tabId },
      files: ["media-utils.js", "content.js"],
    });
    return true;
  } catch (_) {
    INJECTED.delete(tabId); // allow retry (e.g. chrome:// pages reject injection)
    return false;
  }
}

async function getToken() {
  const { apiToken } = await chrome.storage.local.get({ apiToken: "" });
  return (apiToken || "").trim();
}

async function appHealthy() {
  try {
    const r = await fetchWithTimeout(
      `${FALCON_API}/api/health`,
      { method: "GET" },
      HEALTH_TIMEOUT_MS,
      "Falcon health check"
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
  await withTimeout(wakeFalcon(), 5000, "Falcon wake");
  return waitForHealthy(25000);
}
async function getCookiesHeader(url) {
  if (!url) return "";
  try {
    const cookies = await withTimeout(
      chrome.cookies.getAll({ url }),
      3000,
      "Cookie lookup"
    );
    if (cookies.length) {
      return cookies.map((c) => `${c.name}=${c.value}`).join("; ");
    }
  } catch (_) {
  }
  return "";
}

function notify(title, message) {
  chrome.notifications.create({
    type: "basic",
    iconUrl: "icon128.png",
    title,
    message,
  });
}

