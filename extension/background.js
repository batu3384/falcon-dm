importScripts("media-utils.js");

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

function newPairChallenge() {
  if (crypto.randomUUID) return crypto.randomUUID();
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function getNativePairProof(challenge, timeoutMs = 3000) {
  return new Promise((resolve, reject) => {
    if (!chrome.runtime.sendNativeMessage) {
      reject(new Error("Native messaging is unavailable"));
      return;
    }
    let settled = false;
    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        reject(new Error("Native pairing timed out"));
      }
    }, timeoutMs);
    try {
      chrome.runtime.sendNativeMessage(
        "com.falcondm.native",
        { extension_id: chrome.runtime.id, challenge },
        (response) => {
          if (settled) return;
          settled = true;
          clearTimeout(timer);
          const runtimeError = chrome.runtime.lastError;
          if (runtimeError) {
            reject(new Error(runtimeError.message));
            return;
          }
          if (!response || !response.ok || !response.proof) {
            reject(new Error(response?.error || "Native pairing failed"));
            return;
          }
          resolve(response.proof);
        },
      );
    } catch (error) {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        reject(error);
      }
    }
  });
}

async function requestPair() {
  const challenge = newPairChallenge();
  const proof = await getNativePairProof(challenge);
  return fetchWithTimeout(
    `${FALCON_API}/api/pair`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        extension_id: chrome.runtime.id,
        challenge,
        proof,
      }),
    },
    REQUEST_TIMEOUT_MS,
    "Pair request"
  );
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

async function ensurePaired(force = false) {
  if (pairInFlight) return pairInFlight;
  pairInFlight = (async () => {
    try {
      if (!force) {
        const existing = await getToken();
        if (existing && (await appHealthy())) {
          try {
            const r = await fetchWithTimeout(
              `${FALCON_API}/api/ping`,
              {
                method: "POST",
                headers: {
                  "Content-Type": "application/json",
                  "X-Falcon-Token": existing,
                },
                body: "{}",
              },
              REQUEST_TIMEOUT_MS,
              "Falcon ping"
            );
            if (r.ok) {
              setState("connected");
              return existing;
            }
          } catch (_) {}
        }
      }

      if (!(await appHealthy())) {
        await withTimeout(wakeFalcon(), 5000, "Falcon wake");
        if (!(await waitForHealthy(25000))) {
          throw new Error(
            msg("errorWaking", "Falcon DM başlatılamadı — uygulamayı kurun veya manuel açın")
          );
        }
      }

      const r = await requestPair();
      if (r.status === 403) {
        throw new Error(
          msg(
            "errorExtensionBlocked",
            "Extension blocked — open Falcon DM Settings → Reconnect extension"
          )
        );
      }

      const finishPair = async (resp) => {
        if (resp.status === 202) return { pending: true };
        // 503: app is up but token not provisioned yet (cold-start race). Caller retries.
        if (resp.status === 503) return { retry: true };
        if (!resp.ok) return { error: true };
        const data = await resp.json().catch(() => null);
        if (data && data.pending) return { pending: true };
        if (data && data.token && data.ok) {
          await chrome.storage.local.set({ apiToken: data.token });
          return { token: data.token };
        }
        return { error: true };
      };

      let first = await finishPair(r);
      // Cold-start 503 race: short backoff + retry before giving up.
      for (let i = 0; first.retry && i < 5; i++) {
        await sleep(500);
        const rr = await requestPair();
        if (rr.status === 403) {
          throw new Error(
            msg(
              "errorExtensionBlocked",
              "Extension blocked — open Falcon DM Settings → Reconnect extension"
            )
          );
        }
        first = await finishPair(rr);
      }
      if (first.token) {
        setState("connected");
        return first.token;
      }
      if (first.error) {
        throw new Error(msg("errorAppOffline", "Could not pair with Falcon DM"));
      }

      // Pending approval — poll until Settings approve (or timeout)
      setState("pending");
      notify(
        msg("appName", "Falcon DM"),
        msg(
          "errorPairPending",
          "Approve this extension in Falcon DM Settings, then try again"
        )
      );
      for (let i = 0; i < PAIR_POLL_ATTEMPTS; i++) {
        await new Promise((res) => setTimeout(res, 2000));
        let r2;
        try {
          r2 = await requestPair();
        } catch {
          continue;
        }
        if (r2.status === 403) {
          throw new Error(
            msg(
              "errorExtensionBlocked",
              "Extension blocked — open Falcon DM Settings → Reconnect extension"
            )
          );
        }
        const again = await finishPair(r2);
        if (again.token) {
          setState("connected");
          return again.token;
        }
      }
      throw new Error(
        msg(
          "errorPairPending",
          "Approve this extension in Falcon DM Settings, then try again"
        )
      );
    } catch (e) {
      setState("offline");
      throw e;
    } finally {
      pairInFlight = null;
    }
  })();
  return pairInFlight;
}

function mapHttpError(status, bodyError) {
  if (status === 401) return msg("errorTokenInvalid", "Connection lost — Falcon will re-pair automatically");
  if (status === 403)
    return msg(
      "errorExtensionBlocked",
      "Extension blocked — Falcon DM Settings → Reconnect extension"
    );
  if (status === 429) return msg("errorRateLimit", "Too many requests — try again shortly");
  if (bodyError) return bodyError;
  if (status) return msg("errorHttp", "Request failed") + ` (HTTP ${status})`;
  return msg("errorAppOffline", "Falcon DM is not running — open the desktop app");
}

async function postFalcon(path, body) {
  let token = await ensurePaired(false);

  let response;
  try {
    response = await fetchWithTimeout(
      `${FALCON_API}${path}`,
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-Falcon-Token": token,
        },
        body: JSON.stringify(body),
      },
      REQUEST_TIMEOUT_MS,
      `Falcon ${path}`
    );
  } catch {
    setState("offline");
    throw new Error(msg("errorAppOffline", "Falcon DM is not running — open the desktop app"));
  }

  if (response.status === 401) {
    token = await ensurePaired(true);
    try {
      response = await fetchWithTimeout(
        `${FALCON_API}${path}`,
        {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "X-Falcon-Token": token,
          },
          body: JSON.stringify(body),
        },
        REQUEST_TIMEOUT_MS,
        `Falcon ${path} retry`
      );
    } catch {
      setState("offline");
      throw new Error(msg("errorAppOffline", "Falcon DM is not running — open the desktop app"));
    }
  }

  let data = null;
  try {
    data = await response.json();
  } catch {
    data = null;
  }

  if (!response.ok) {
    throw new Error(mapHttpError(response.status, data && data.error));
  }
  if (data && data.success === false) {
    const err = data.error || msg("errorInvalidUrl", "Invalid download URL");
    if (err === "invalid url") throw new Error(msg("errorInvalidUrl", "No valid URL"));
    throw new Error(err);
  }
  try {
    if (body && body.url) {
      trackDownload(body.filename || "", body.url, path === "/api/intercept" ? "media" : "file");
    }
  } catch (_) {}
  return data;
}

async function getCookiesHeader(url, fallbackUrl = "") {
  for (const target of [url, fallbackUrl]) {
    if (!target) continue;
    try {
      const cookies = await withTimeout(
        chrome.cookies.getAll({ url: target }),
        3000,
        "Cookie lookup"
      );
      if (cookies.length) {
        return cookies.map((c) => `${c.name}=${c.value}`).join("; ");
      }
    } catch (_) {}
  }
  return "";
}

/** Deep-link download removed — token-in-URL leaks secrets. Cold path = wake + HTTP only. */
async function sendToFalcon(path, body) {
  if (await appHealthy()) {
    return postFalcon(path, body);
  }

  await withTimeout(wakeFalcon(), 5000, "Falcon wake");
  if (await waitForHealthy(30000)) {
    return postFalcon(path, body);
  }

  setState("offline");
  throw new Error(msg("errorWaking", "Falcon DM başlatılamadı"));
}

function notify(title, message) {
  chrome.notifications.create({
    type: "basic",
    iconUrl: "icon128.png",
    title,
    message,
  });
}

function setupMenus() {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: "download_with_falcon",
      title: msg("contextDownload", "Download with Falcon DM"),
      contexts: ["link", "image", "video", "audio"],
    });
    chrome.contextMenus.create({
      id: "grab_page_links",
      title: msg("contextGrabber", "Grab page links with Falcon"),
      contexts: ["page"],
    });
  });
}

chrome.runtime.onInstalled.addListener(() => {
  setupMenus();
  refreshBadge();
  ensurePaired(true).catch(() => {
    setState("offline");
    notify(
      msg("appName", "Falcon DM"),
      msg("popupOnboard", "Open Falcon DM and approve this extension in Settings")
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

chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (!changeInfo.url) return;
  MEDIA_URLS.delete(tabId);
  MEDIA_META.delete(tabId);
  INJECTED.delete(tabId);
});

function headerValue(headers, name) {
  const h = (headers || []).find((x) => x.name.toLowerCase() === name.toLowerCase());
  return h ? h.value : "";
}

chrome.webRequest.onHeadersReceived.addListener(
  (details) => {
    const url = details.url;
    const ct = headerValue(details.responseHeaders, "content-type").toLowerCase();
    const cl = parseInt(headerValue(details.responseHeaders, "content-length"), 10) || 0;

    if (!self.FalconMedia || !self.FalconMedia.isCapturableMedia(url, ct)) return;

    const clean = self.FalconMedia.normalizeMediaUrl(url);
    if (!clean || self.FalconMedia.isJunkUrl(clean)) return;

    if (details.tabId > -1) {
      const set = MEDIA_URLS.get(details.tabId) || new Set();
      set.add(clean);
      MEDIA_URLS.set(details.tabId, set);

      const metaMap = MEDIA_META.get(details.tabId) || new Map();
      const prev = metaMap.get(clean);
      // Keep largest content-length seen for this URL
      metaMap.set(clean, {
        contentLength: Math.max(cl, (prev && prev.contentLength) || 0),
        contentType: ct || (prev && prev.contentType) || "",
        ts: Date.now(),
      });
      MEDIA_META.set(details.tabId, metaMap);

      // Inject the on-demand overlay so the user sees the Falcon button on this media.
      ensureContentScript(details.tabId);
    }
  },
  { urls: ["<all_urls>"] },
  ["responseHeaders"]
);

chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  if (interceptPaused) {
    // Paused: let the browser handle the download natively.
    suggest({ cancel: false });
    return;
  }
  (async () => {
    try {
      const cookiesHeader = await getCookiesHeader(item.url);

      const filename =
        item.filename ||
        item.url.split("/").pop().split("?")[0] ||
        "download";

      await sendToFalcon("/api/add", {
        url: item.url,
        filename,
        referrer: item.referrer || "",
        user_agent: navigator.userAgent,
        cookies: cookiesHeader,
      });
      suggest({ cancel: true });
      notify("Falcon DM", msg("sentToApp", "Download sent to Falcon DM"));
    } catch (e) {
      console.error(e);
      suggest({ cancel: false });
      notify("Falcon DM", e.message || msg("appClosedFallback", "Falcon DM offline — browser download kept"));
    }
  })();
  return true;
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === "download_with_falcon") {
    const url = info.linkUrl || info.srcUrl;
    if (!url) return;
    const filename = url.split("/").pop().split("?")[0] || "download";
    try {
      const cookiesHeader = await getCookiesHeader(url);
      await sendToFalcon("/api/add", {
        url,
        filename,
        referrer: info.pageUrl || "",
        user_agent: navigator.userAgent,
        cookies: cookiesHeader,
      });
      notify("Falcon DM", msg("sentToApp", "Download sent to Falcon DM"));
    } catch (e) {
      console.error(e);
      notify("Falcon DM", e.message || msg("errorAppOffline", "Failed"));
    }
  }

  if (info.menuItemId === "grab_page_links" && tab?.id) {
    ensureContentScript(tab.id).then(() => {
      chrome.tabs.sendMessage(tab.id, { action: "open_grabber" });
    });
  }
});

chrome.runtime.onMessage.addListener((request, sender, sendResponse) => {
  // --- popup: live status + queue preview ---
  if (request.action === "check_status") {
    (async () => {
      const healthy = await appHealthy();
      if (healthy) {
        const token = await getToken();
        setState(token ? "connected" : "pending");
      } else {
        setState("offline");
      }
      sendResponse({
        state: connectionState,
        paused: interceptPaused,
        recent: RECENT.slice(0, 3),
      });
    })();
    return true;
  }

  if (request.action === "get_status") {
    sendResponse({
      state: connectionState,
      paused: interceptPaused,
      recent: RECENT.slice(0, 3),
    });
    return true;
  }

  if (request.action === "set_paused") {
    const next = !!request.paused;
    // Respond immediately, then durably persist so an MV3 SW restart keeps the
    // user's hijack preference (previously a SW restart would reset to false).
    interceptPaused = next;
    chrome.storage.session.set({ falconInterceptPaused: next }).catch(() => {});
    sendResponse({ ok: true, paused: interceptPaused });
    return true;
  }

  // --- popup: download current tab's media (inject overlay + open picker) ---
  if (request.action === "grab_tab_media") {
    (async () => {
      try {
        const [active] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
        if (!active || !active.id) {
          sendResponse({ ok: false, error: msg("errorInvalidUrl", "No active tab") });
          return;
        }
        const injected = await ensureContentScript(active.id);
        if (!injected) {
          sendResponse({ ok: false, error: msg("errorMediaUtils", "Cannot run on this page") });
          return;
        }
        chrome.tabs.sendMessage(active.id, { action: "open_download_modal" }, () => {
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
  if (request.action === "add_url") {
    const rawUrl = (request.url || "").trim();
    if (!rawUrl || !/^https?:/i.test(rawUrl)) {
      sendResponse({ success: false, error: msg("errorInvalidUrl", "No valid URL") });
      return true;
    }
    (async () => {
      try {
        const cookies = await getCookiesHeader(rawUrl);
        await sendToFalcon("/api/add", {
          url: rawUrl,
          filename: rawUrl.split("/").pop().split("?")[0] || "download",
          referrer: rawUrl,
          user_agent: navigator.userAgent,
          cookies,
        });
        sendResponse({ success: true });
      } catch (e) {
        sendResponse({ success: false, error: e.message });
      }
    })();
    return true;
  }

  if (request.action === "get_real_media_url") {
    const tabId = sender.tab ? sender.tab.id : -1;
    const set = MEDIA_URLS.get(tabId);
    const urls = set ? Array.from(set) : [];
    const pageUrl = (sender.tab && sender.tab.url) || request.page_url || "";
    const title = (sender.tab && sender.tab.title) || "";

    const metaMap = {};
    const rawMeta = MEDIA_META.get(tabId);
    if (rawMeta) {
      rawMeta.forEach((v, k) => {
        metaMap[k] = v;
      });
    }

    const finish = async (extraUrls) => {
      const cookieString = await getCookiesHeader(pageUrl);
      const merged = [...new Set([...(urls || []), ...(extraUrls || [])])];
      sendResponse({
        url: merged[merged.length - 1] || null,
        urls: merged,
        metaMap,
        title,
        cookies: cookieString,
        userAgent: navigator.userAgent,
      });
    };

    // Pull YouTube player_response from MAIN world (isolated world cannot see it)
    const isYt =
      /youtube\.com|youtu\.be/i.test(pageUrl) && tabId > -1 && chrome.scripting;
    if (isYt) {
      chrome.scripting
        .executeScript({
          target: { tabId },
          world: "MAIN",
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
              if (typeof raw === "string") raw = JSON.parse(raw);
              pushPr(raw);
            } catch (_) {}
            try {
              const el = document.getElementById("movie_player");
              if (el && typeof el.getPlayerResponse === "function") {
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

  if (request.action === "check_connection") {
    (async () => {
      try {
        await ensureAppRunning();
        await ensurePaired(true);
        await postFalcon("/api/ping", {});
        sendResponse({ ok: true });
      } catch (e) {
        sendResponse({ ok: false, error: e.message });
      }
    })();
    return true;
  }

  if (request.action === "auto_pair") {
    ensureAppRunning()
      .then(() => ensurePaired(true))
      .then(() => sendResponse({ ok: true }))
      .catch((e) => sendResponse({ ok: false, error: e.message }));
    return true;
  }

  if (request.action === "download_video" || request.action === "download_url") {
    const rawUrl = (request.url || "").trim();
    if (!rawUrl || rawUrl.startsWith("blob:")) {
      sendResponse({
        success: false,
        error: msg("errorNoValidSource", "No downloadable URL found — play the video and try again"),
      });
      return true;
    }

    if (self.FalconMedia && self.FalconMedia.isJunkUrl(rawUrl)) {
      sendResponse({
        success: false,
        error: msg("errorJunkUrl", "Bu adres gerçek video değil (YouTube UI sesi) — videoyu oynatıp tekrar dene"),
      });
      return true;
    }

    const url =
      self.FalconMedia && self.FalconMedia.normalizeMediaUrl
        ? self.FalconMedia.normalizeMediaUrl(rawUrl)
        : rawUrl;

    sendToFalcon("/api/intercept", {
      url,
      page_url: request.page_url,
      title: request.title || "",
      cookies: request.cookies || "",
      user_agent: request.user_agent || navigator.userAgent,
      referer: request.page_url,
      filename: request.filename || null,
      media_type: request.media_type || "application/octet-stream",
      format: request.format || null,
    })
      .then((data) => sendResponse({ success: true, data }))
      .catch((err) => sendResponse({ success: false, error: err.message }));
    return true;
  }

  if (request.action === "batch_download") {
    const items = (request.items || []).slice(0, 20);
    (async () => {
      const settled = await Promise.allSettled(
        items.map(async (it) => {
          try {
            const cookies = await getCookiesHeader(it.url, request.page_url);
            const data = await sendToFalcon("/api/add", {
              url: it.url,
              filename: it.filename || it.url.split("/").pop().split("?")[0] || "download",
              referrer: request.page_url || "",
              user_agent: navigator.userAgent,
              cookies,
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
        })
      );
      const results = settled.map((result, index) =>
        result.status === "fulfilled"
          ? result.value
          : { url: items[index].url, ok: false, error: result.reason?.message || String(result.reason) }
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
