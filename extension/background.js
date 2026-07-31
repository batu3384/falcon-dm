importScripts("media-utils.js");

const FALCON_API = "http://127.0.0.1:14201";
const WAKE_URL = "falcondm://wake";
const MEDIA_URLS = new Map();
const MEDIA_META = new Map();
let pairInFlight = null;

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

async function getToken() {
  const { apiToken } = await chrome.storage.local.get({ apiToken: "" });
  return (apiToken || "").trim();
}

async function appHealthy() {
  try {
    const r = await fetch(`${FALCON_API}/api/health`, { method: "GET" });
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

/** @deprecated Deep-link enqueue removed (token-in-URL leak). Kept as no-op for safety. */
function deepLinkDownload(_payload) {
  return wakeFalcon();
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
  await wakeFalcon();
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
            const r = await fetch(`${FALCON_API}/api/ping`, {
              method: "POST",
              headers: {
                "Content-Type": "application/json",
                "X-Falcon-Token": existing,
              },
              body: "{}",
            });
            if (r.ok) return existing;
          } catch (_) {}
        }
      }

      if (!(await appHealthy())) {
        await wakeFalcon();
        if (!(await waitForHealthy(25000))) {
          throw new Error(
            msg("errorWaking", "Falcon DM başlatılamadı — uygulamayı kurun veya manuel açın")
          );
        }
      }

      const r = await fetch(`${FALCON_API}/api/pair`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: "{}",
      });
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
      if (first.token) return first.token;
      if (first.error) {
        throw new Error(msg("errorAppOffline", "Could not pair with Falcon DM"));
      }

      // Pending approval — poll until Settings approve (or timeout)
      notify(
        msg("appName", "Falcon DM"),
        msg(
          "errorPairPending",
          "Approve this extension in Falcon DM Settings, then try again"
        )
      );
      for (let i = 0; i < 90; i++) {
        await new Promise((res) => setTimeout(res, 2000));
        let r2;
        try {
          r2 = await fetch(`${FALCON_API}/api/pair`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: "{}",
          });
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
        if (again.token) return again.token;
      }
      throw new Error(
        msg(
          "errorPairPending",
          "Approve this extension in Falcon DM Settings, then try again"
        )
      );
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
    response = await fetch(`${FALCON_API}${path}`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Falcon-Token": token,
      },
      body: JSON.stringify(body),
    });
  } catch {
    throw new Error(msg("errorAppOffline", "Falcon DM is not running — open the desktop app"));
  }

  if (response.status === 401) {
    token = await ensurePaired(true);
    try {
      response = await fetch(`${FALCON_API}${path}`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-Falcon-Token": token,
        },
        body: JSON.stringify(body),
      });
    } catch {
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
  return data;
}

/** Deep-link download removed — token-in-URL leaks secrets. Cold path = wake + HTTP only. */
async function sendToFalcon(path, body) {
  if (await appHealthy()) {
    return postFalcon(path, body);
  }

  await wakeFalcon();
  if (await waitForHealthy(30000)) {
    return postFalcon(path, body);
  }

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
  ensurePaired(true).catch(() => {});
});

chrome.runtime.onStartup.addListener(() => {
  ensurePaired(false).catch(() => {});
});

ensurePaired(false).catch(() => {});

chrome.tabs.onRemoved.addListener((tabId) => {
  MEDIA_URLS.delete(tabId);
  MEDIA_META.delete(tabId);
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
    }
  },
  { urls: ["<all_urls>"] },
  ["responseHeaders"]
);

chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  (async () => {
    try {
      let cookiesHeader = "";
      try {
        const cookies = await chrome.cookies.getAll({ url: item.url });
        cookiesHeader = cookies.map((c) => `${c.name}=${c.value}`).join("; ");
      } catch (_) {}

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
      let cookiesHeader = "";
      try {
        const cookies = await chrome.cookies.getAll({ url });
        cookiesHeader = cookies.map((c) => `${c.name}=${c.value}`).join("; ");
      } catch (_) {}
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
    chrome.tabs.sendMessage(tab.id, { action: "open_grabber" });
  }
});

chrome.runtime.onMessage.addListener((request, sender, sendResponse) => {
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

    const finish = (extraUrls) => {
      chrome.cookies.getAll({ url: pageUrl }, (cookies) => {
        const cookieString = (cookies || []).map((c) => `${c.name}=${c.value}`).join("; ");
        const merged = [...new Set([...(urls || []), ...(extraUrls || [])])];
        sendResponse({
          url: merged[merged.length - 1] || null,
          urls: merged,
          metaMap,
          title,
          cookies: cookieString,
          userAgent: navigator.userAgent,
        });
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
      let cookies = request.cookies || "";
      if (!cookies && request.page_url) {
        try {
          const list = await chrome.cookies.getAll({ url: request.page_url });
          cookies = (list || []).map((c) => `${c.name}=${c.value}`).join("; ");
        } catch (_) {}
      }
      await Promise.all(
        items.map((it) =>
          sendToFalcon("/api/add", {
            url: it.url,
            filename: it.filename || it.url.split("/").pop().split("?")[0] || "download",
            referrer: request.page_url || "",
            user_agent: navigator.userAgent,
            cookies,
          })
        )
      );
      sendResponse({ success: true, count: items.length });
    })().catch((err) => sendResponse({ success: false, error: err.message }));
    return true;
  }
});
