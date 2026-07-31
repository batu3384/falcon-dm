(function () {
  if (window.falconDmInjected) return;
  window.falconDmInjected = true;

  const FM = window.FalconMedia;
  const TOKENS = {
    primary: "#2563EB",
    accent: "#D97706",
    surface: "#0f172a",
    surface2: "#1e293b",
    card: "#1e293b",
    cardActive: "rgba(37,99,235,0.18)",
    text: "#f8fafc",
    muted: "#94a3b8",
    border: "#334155",
    success: "#22c55e",
    danger: "#ef4444",
    radius: "14px",
  };

  const ATTACHED_VIDEOS = new WeakSet();

  function msg(key, fallback) {
    return chrome.i18n.getMessage(key) || fallback;
  }

  function esc(s) {
    return String(s || "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  function el(tag, styles, props) {
    const node = document.createElement(tag);
    if (styles) Object.assign(node.style, styles);
    if (props) Object.assign(node, props);
    return node;
  }

  function sendBg(payload) {
    return new Promise((resolve, reject) => {
      if (!chrome?.runtime?.id) {
        reject(new Error(msg("errorExtensionReload", "Eklenti yenilendi — sayfayı yenileyin")));
        return;
      }
      chrome.runtime.sendMessage(payload, (response) => {
        const err = chrome.runtime.lastError;
        if (err) reject(new Error(err.message));
        else resolve(response);
      });
    });
  }

  /**
   * Content scripts run in isolated world — cannot read page `window.yt*`.
   * Bridge: inject page-world script → postMessage player_response URLs.
   */
  function extractUrlsFromPlayerResponse(pr) {
    const out = [];
    if (!pr || !pr.streamingData) return out;
    for (const fmt of pr.streamingData.formats || []) {
      if (fmt.url) out.push(fmt.url);
    }
    for (const fmt of pr.streamingData.adaptiveFormats || []) {
      if (fmt.url) out.push(fmt.url);
      // Some streams only expose cipher — skip (no url)
    }
    return out;
  }

  function scrapePlayerResponseFromDom() {
    const out = [];
    const scripts = document.querySelectorAll("script");
    for (const s of scripts) {
      const t = s.textContent || "";
      if (!t.includes("ytInitialPlayerResponse") && !t.includes("streamingData")) continue;
      const m = t.match(/ytInitialPlayerResponse\s*=\s*(\{[\s\S]*?\});\s*(?:var|const|let|window|if|<\/)/);
      if (!m) continue;
      try {
        const pr = JSON.parse(m[1]);
        out.push(...extractUrlsFromPlayerResponse(pr));
        if (out.length) break;
      } catch (_) {}
    }
    return out;
  }

  function extractYouTubeUrlsFromPageWorld(timeoutMs) {
    return new Promise((resolve) => {
      const requestId = "falcon-yt-" + Math.random().toString(36).slice(2);
      let done = false;
      const finish = (urls) => {
        if (done) return;
        done = true;
        window.removeEventListener("message", onMsg);
        resolve(urls || []);
      };
      const onMsg = (ev) => {
        if (ev.source !== window) return;
        const d = ev.data;
        if (!d || d.source !== "falcon-dm-page" || d.id !== requestId) return;
        finish(Array.isArray(d.urls) ? d.urls : []);
      };
      window.addEventListener("message", onMsg);
      const script = document.createElement("script");
      script.textContent = `(() => {
        try {
          const id = ${JSON.stringify(requestId)};
          const out = [];
          const pushPr = (pr) => {
            if (!pr || !pr.streamingData) return;
            for (const f of (pr.streamingData.formats || [])) if (f.url) out.push(f.url);
            for (const f of (pr.streamingData.adaptiveFormats || [])) if (f.url) out.push(f.url);
          };
          pushPr(window.ytInitialPlayerResponse);
          try {
            const cfg = window.ytplayer && window.ytplayer.config && window.ytplayer.config.args;
            if (cfg) {
              let raw = cfg.player_response || cfg.raw_player_response;
              if (typeof raw === "string") raw = JSON.parse(raw);
              pushPr(raw);
            }
          } catch (e) {}
          try {
            const p = document.querySelector("ytd-player, #movie_player");
            const player = p && (p.getPlayer ? p.getPlayer() : (window.yt && window.yt.player && window.yt.player.getPlayerByElement && window.yt.player.getPlayerByElement(p)));
            if (player && player.getPlayerResponse) pushPr(player.getPlayerResponse());
          } catch (e) {}
          window.postMessage({ source: "falcon-dm-page", id, urls: out }, "*");
        } catch (e) {
          window.postMessage({ source: "falcon-dm-page", id: ${JSON.stringify(requestId)}, urls: [] }, "*");
        }
      })();`;
      (document.documentElement || document.head).appendChild(script);
      script.remove();
      setTimeout(() => finish([]), timeoutMs || 800);
    });
  }

  async function extractYouTubeUrls() {
    const fromDom = scrapePlayerResponseFromDom();
    if (fromDom.length) return [...new Set(fromDom)];
    const fromPage = await extractYouTubeUrlsFromPageWorld(900);
    return [...new Set(fromPage)];
  }

  function injectStyles(shadow) {
    const style = document.createElement("style");
    style.textContent = `
      * { box-sizing: border-box; }
      .fm-title { margin: 0; font-size: 18px; font-weight: 700; color: #fff; }
      .fm-sub { margin: 4px 0 0; font-size: 13px; color: ${TOKENS.muted}; line-height: 1.4; }
      .fm-label { font-size: 11px; font-weight: 700; color: ${TOKENS.muted}; text-transform: uppercase; letter-spacing: .06em; margin-bottom: 8px; display: block; }
      .fm-cards { display: flex; flex-direction: column; gap: 8px; max-height: 220px; overflow: auto; }
      .fm-card {
        display: flex; align-items: center; gap: 12px; padding: 12px 14px;
        border: 1px solid ${TOKENS.border}; border-radius: 10px; background: ${TOKENS.card};
        cursor: pointer; transition: border-color .15s, background .15s;
      }
      .fm-card:hover { border-color: #475569; }
      .fm-card.active { border-color: ${TOKENS.primary}; background: ${TOKENS.cardActive}; }
      .fm-card input { accent-color: ${TOKENS.primary}; width: 16px; height: 16px; flex-shrink: 0; }
      .fm-card-body { flex: 1; min-width: 0; }
      .fm-card-title { font-size: 14px; font-weight: 600; color: #fff; }
      .fm-card-meta { font-size: 12px; color: ${TOKENS.muted}; margin-top: 2px; }
      .fm-badge {
        font-size: 10px; font-weight: 700; padding: 3px 8px; border-radius: 999px;
        background: rgba(217,119,6,.2); color: #fbbf24; text-transform: uppercase; letter-spacing: .04em;
      }
      .fm-badge.hls { background: rgba(37,99,235,.2); color: #93c5fd; }
      .fm-input {
        width: 100%; padding: 10px 12px; border-radius: 8px; border: 1px solid ${TOKENS.border};
        background: ${TOKENS.surface2}; color: #fff; font-size: 13px;
      }
      .fm-info {
        padding: 10px 12px; border-radius: 8px; font-size: 12px; line-height: 1.45;
        background: rgba(37,99,235,.12); border: 1px solid rgba(37,99,235,.3); color: #bfdbfe;
      }
      .fm-error {
        padding: 10px 12px; border-radius: 8px; font-size: 12px; line-height: 1.45;
        background: rgba(239,68,68,.12); border: 1px solid rgba(239,68,68,.35); color: #fecaca;
      }
      .fm-actions { display: flex; justify-content: flex-end; gap: 10px; margin-top: 4px; }
      .fm-btn {
        padding: 10px 18px; border-radius: 8px; font-size: 13px; font-weight: 600; cursor: pointer; border: none;
      }
      .fm-btn-ghost { background: transparent; color: #fff; border: 1px solid ${TOKENS.border}; }
      .fm-btn-primary { background: ${TOKENS.accent}; color: #fff; }
      .fm-btn:disabled { opacity: .6; cursor: default; }
    `;
    shadow.appendChild(style);
  }

  function createModal(pageTitle, pageUrl, cookies, ua, sources) {
    if (!FM) {
      alert(msg("errorMediaUtils", "Falcon eklenti modülü yüklenemedi — eklentiyi yenileyin"));
      return;
    }

    let selected = FM.pickBest(sources) || sources[0] || null;

    const host = el("div", {
      position: "fixed", inset: "0", zIndex: "2147483647",
      display: "flex", alignItems: "center", justifyContent: "center",
      background: "rgba(0,0,0,0.6)", backdropFilter: "blur(8px)",
    });
    const shadow = host.attachShadow({ mode: "open" });
    injectStyles(shadow);

    const panel = el("div", {
      width: "min(500px, calc(100vw - 32px))",
      background: TOKENS.surface,
      borderRadius: TOKENS.radius,
      border: `1px solid ${TOKENS.border}`,
      boxShadow: "0 24px 48px rgba(0,0,0,.5)",
      padding: "22px",
      display: "flex", flexDirection: "column", gap: "16px",
      fontFamily: "-apple-system, BlinkMacSystemFont, 'Inter', sans-serif",
      color: TOKENS.text,
    });

    const header = el("div");
    header.innerHTML = `<h2 class="fm-title">${esc(msg("downloadVideo", "Download with Falcon DM"))}</h2><p class="fm-sub">${esc(pageTitle || pageUrl)}</p>`;
    panel.appendChild(header);

    const errorBox = el("div", { display: "none" });
    errorBox.className = "fm-error";

    const nameWrap = el("div");
    nameWrap.innerHTML = `<label class="fm-label">${esc(msg("labelFilename", "File name"))}</label>`;
    const nameInput = el("input");
    nameInput.className = "fm-input";
    nameInput.value = FM.defaultFilename(pageTitle, selected);
    nameWrap.appendChild(nameInput);

    if (!sources.length) {
      const err = el("div");
      err.className = "fm-info";
      err.textContent = msg(
        "errorNoValidSource",
        "No source found — play the video first, then try again"
      );
      panel.appendChild(err);
    } else {
      const info = el("div");
      info.className = "fm-info";
      info.textContent = /youtube\.com|youtu\.be/i.test(pageUrl)
        ? msg("youtubeHint", "YouTube: Falcon yt-dlp ile sayfa adresinden indirir (CDN 403 olmaz). En iyi kalite otomatik birleşir.")
        : msg("qualityHint", "Choose quality and format. HLS streams are merged automatically by Falcon DM.");
      panel.appendChild(info);

      const label = el("label");
      label.className = "fm-label";
      label.textContent = msg("labelQuality", "Quality & format");
      panel.appendChild(label);

      const cards = el("div");
      cards.className = "fm-cards";

      function renderCards() {
        cards.innerHTML = "";
        sources.forEach((item) => {
          const card = el("label");
          card.className = "fm-card" + (selected && selected.url === item.url ? " active" : "");

          const radio = el("input");
          radio.type = "radio";
          radio.name = "falcon-quality";
          radio.checked = !!(selected && selected.url === item.url);

          const body = el("div");
          body.className = "fm-card-body";
          const metaParts = [item.subtitle];
          if (item.sizeLabel) metaParts.push(item.sizeLabel);
          body.innerHTML = `<div class="fm-card-title">${esc(item.title)}</div><div class="fm-card-meta">${esc(metaParts.join(" · "))}</div>`;

          const badge = el("span");
          badge.className = "fm-badge" + (item.isHls ? " hls" : "");
          badge.textContent = item.isHls ? "HLS" : item.format;

          card.appendChild(radio);
          card.appendChild(body);
          card.appendChild(badge);

          card.addEventListener("click", () => {
            selected = item;
            renderCards();
            errorBox.style.display = "none";
            nameInput.value = FM.defaultFilename(pageTitle, item);
          });

          cards.appendChild(card);
        });
      }
      renderCards();
      panel.appendChild(cards);
    }

    panel.appendChild(nameWrap);
    panel.appendChild(errorBox);

    const actions = el("div");
    actions.className = "fm-actions";
    const cancelBtn = el("button");
    cancelBtn.className = "fm-btn fm-btn-ghost";
    cancelBtn.textContent = msg("cancel", "Cancel");
    cancelBtn.onclick = () => host.remove();

    const goBtn = el("button");
    goBtn.className = "fm-btn fm-btn-primary";
    goBtn.textContent = msg("startDownload", "Start Download");
      goBtn.onclick = async () => {
      errorBox.style.display = "none";
      if (!selected) {
        errorBox.textContent = msg("errorNoValidSource", "No valid source");
        errorBox.style.display = "block";
        return;
      }
      goBtn.disabled = true;
      goBtn.textContent = msg("sending", "Sending...");

      // YouTube: never send googlevideo CDN — watch URL + format field → yt-dlp -f
      const isYt =
        /youtube\.com|youtu\.be/i.test(pageUrl) ||
        (selected.url || "").includes("googlevideo") ||
        (selected.url || "").includes("videoplayback");
      let downloadUrl = isYt ? pageUrl.split("#")[0] : selected.url;
      let format = null;
      if (isYt) {
        const h = Number(selected.height) || Number(selected.label) || 1080;
        const height = Math.min(Math.max(h, 144), 2160);
        format = `bestvideo[height<=${height}]+bestaudio/best[height<=${height}]/bv*+ba/b`;
      }

      try {
        const response = await sendBg({
          action: "download_video",
          url: downloadUrl,
          page_url: pageUrl,
          title: pageTitle,
          cookies,
          user_agent: ua,
          filename: nameInput.value,
          format,
        });
        if (response && response.success) {
          goBtn.textContent = msg("queued", "Queued in Falcon DM");
          goBtn.style.background = TOKENS.success;
          setTimeout(() => host.remove(), 1000);
        } else {
          throw new Error((response && response.error) || msg("errorAppOffline", "Connection failed"));
        }
      } catch (e) {
        errorBox.textContent = e.message || msg("errorAppOffline", "Connection failed");
        errorBox.style.display = "block";
        goBtn.disabled = false;
        goBtn.textContent = msg("startDownload", "Start Download");
      }
    };

    actions.appendChild(cancelBtn);
    actions.appendChild(goBtn);
    panel.appendChild(actions);
    shadow.appendChild(panel);
    document.body.appendChild(host);
    host.addEventListener("click", (e) => { if (e.target === host) host.remove(); });
  }

  async function openDownloadModal() {
    if (!FM) {
      alert(msg("errorMediaUtils", "Falcon eklenti modülü yüklenemedi — eklentiyi yenileyin"));
      return;
    }

    let resp;
    try {
      resp = await sendBg({ action: "get_real_media_url", page_url: location.href });
    } catch (e) {
      alert(e.message);
      return;
    }

    const metaMap = (resp && resp.metaMap) || {};
    let urls = [...((resp && resp.urls) || [])];

    if (location.hostname.includes("youtube.com") || location.hostname.includes("youtu.be")) {
      const ytUrls = await extractYouTubeUrls();
      urls = [...new Set([...urls, ...ytUrls])];
    }

    let sources = FM.groupSources(urls, metaMap);

    if (!sources.length && (location.hostname.includes("youtube.com") || location.hostname.includes("youtu.be"))) {
      await new Promise((r) => setTimeout(r, 1500));
      try {
        resp = await sendBg({ action: "get_real_media_url", page_url: location.href });
        const ytUrls = await extractYouTubeUrls();
        urls = [...new Set([...((resp && resp.urls) || []), ...ytUrls])];
        sources = FM.groupSources(urls, (resp && resp.metaMap) || {});
      } catch (_) {}
    }

    // Prefer progressive muxed, then HLS, then adaptive video
    const muxed = sources.filter((s) => s.muxed && !s.isAudio);
    const hls = sources.filter((s) => s.isHls);
    const video = sources.filter((s) => !s.isAudio && !s.muxed && !s.isHls);
    const audio = sources.filter((s) => s.isAudio);
    if (muxed.length || hls.length || video.length) {
      sources = [...muxed, ...hls, ...video, ...audio];
    }

    createModal(
      (resp && resp.title) || document.title,
      location.href,
      (resp && resp.cookies) || "",
      (resp && resp.userAgent) || navigator.userAgent,
      sources
    );
  }

  function createDownloadButton() {
    const btn = el("button", {
      position: "absolute", zIndex: "999999", top: "10px", right: "10px",
      background: TOKENS.accent, color: "#fff", padding: "8px 14px", borderRadius: "8px",
      fontSize: "12px", fontWeight: "700", cursor: "pointer", border: "none",
      boxShadow: "0 4px 14px rgba(0,0,0,.35)", opacity: "0", transition: "opacity .2s",
      display: "inline-flex", alignItems: "center", gap: "6px",
    });
    btn.innerHTML = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" aria-hidden="true"><path d="M12 3v12m0 0l-4-4m4 4l4-4M5 21h14"/></svg><span>${esc(msg("downloadWithFalcon", "Falcon"))}</span>`;
    btn.setAttribute("aria-label", msg("downloadWithFalcon", "Download with Falcon"));
    btn.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      openDownloadModal().catch((err) => console.error("[Falcon DM]", err));
    });
    return btn;
  }

  function attachToVideo(video) {
    if (ATTACHED_VIDEOS.has(video)) return;
    ATTACHED_VIDEOS.add(video);
    const parent = video.parentElement;
    if (!parent) return;
    if (getComputedStyle(parent).position === "static") parent.style.position = "relative";
    const btn = createDownloadButton();
    btn.style.opacity = "0.85";
    parent.appendChild(btn);
    const show = () => (btn.style.opacity = "1");
    const hide = () => (btn.style.opacity = "0.85");
    parent.addEventListener("mouseenter", show);
    parent.addEventListener("mouseleave", hide);
    video.addEventListener("mouseenter", show);
    btn.addEventListener("focus", show);
  }

  function collectGrabberLinks() {
    const exts = /\.(mp4|mkv|webm|mov|avi|mp3|m4a|flac|zip|rar|7z|pdf|exe|dmg|pkg|iso)(\?|$)/i;
    const seen = new Set();
    const out = [];
    document.querySelectorAll("a[href]").forEach((a) => {
      try {
        const u = new URL(a.href, location.href).href;
        if (seen.has(u)) return;
        if (!/^https?:/i.test(u)) return;
        if (!exts.test(u) && !a.hasAttribute("download")) return;
        seen.add(u);
        out.push({
          url: u,
          filename: (a.getAttribute("download") || u.split("/").pop().split("?")[0] || "download").slice(0, 180),
        });
      } catch (_) {}
    });
    return out.slice(0, 50);
  }

  function openGrabber() {
    const links = collectGrabberLinks();
    const host = document.createElement("div");
    host.style.cssText =
      "position:fixed;inset:0;z-index:2147483646;background:rgba(15,23,42,.72);display:flex;align-items:center;justify-content:center;font-family:Inter,system-ui,sans-serif";
    host.tabIndex = -1;
    const panel = document.createElement("div");
    panel.style.cssText =
      "width:min(480px,92vw);max-height:70vh;overflow:auto;background:#0f172a;color:#f8fafc;border-radius:14px;padding:16px;border:1px solid #334155";
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-modal", "true");
    const title = document.createElement("h2");
    title.textContent = msg("grabberTitle", "Link Grabber");
    title.style.cssText = "margin:0 0 8px;font-size:16px";
    const info = document.createElement("p");
    info.style.cssText = "margin:0 0 12px;font-size:13px;color:#94a3b8";
    info.textContent = links.length
      ? `${links.length} ${msg("grabberFound", "downloadable links found")}`
      : msg("grabberEmpty", "No downloadable links found on this page");
    const list = document.createElement("div");
    const checks = [];
    links.forEach((it, i) => {
      const row = document.createElement("label");
      row.style.cssText = "display:flex;gap:8px;align-items:flex-start;margin:6px 0;font-size:12px;cursor:pointer";
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.checked = true;
      checks.push({ cb, it });
      const span = document.createElement("span");
      span.textContent = it.filename;
      span.title = it.url;
      row.appendChild(cb);
      row.appendChild(span);
      list.appendChild(row);
    });
    const actions = document.createElement("div");
    actions.style.cssText = "display:flex;gap:8px;justify-content:flex-end;margin-top:14px";
    const cancel = document.createElement("button");
    cancel.textContent = msg("cancel", "Cancel");
    cancel.style.cssText = "padding:8px 12px;border-radius:8px;border:1px solid #475569;background:transparent;color:#e2e8f0;cursor:pointer";
    cancel.onclick = () => host.remove();
    const send = document.createElement("button");
    send.textContent = msg("grabberSend", "Send selected");
    send.style.cssText = "padding:8px 12px;border-radius:8px;border:none;background:#D97706;color:#fff;font-weight:600;cursor:pointer";
    send.disabled = !links.length;
    send.onclick = async () => {
      const items = checks.filter((c) => c.cb.checked).map((c) => c.it);
      if (!items.length) return;
      send.disabled = true;
      try {
        const response = await sendBg({
          action: "batch_download",
          items,
          page_url: location.href,
          cookies: "",
        });
        if (!response || !response.success) throw new Error((response && response.error) || "failed");
        host.remove();
      } catch (e) {
        info.textContent = e.message || "failed";
        info.style.color = "#f87171";
        send.disabled = false;
      }
    };
    actions.appendChild(cancel);
    actions.appendChild(send);
    panel.appendChild(title);
    panel.appendChild(info);
    panel.appendChild(list);
    panel.appendChild(actions);
    host.appendChild(panel);
    host.addEventListener("keydown", (e) => {
      if (e.key === "Escape") host.remove();
    });
    host.addEventListener("click", (e) => {
      if (e.target === host) host.remove();
    });
    document.documentElement.appendChild(host);
    cancel.focus();
  }

  document.querySelectorAll("video").forEach(attachToVideo);
  new MutationObserver((mutations) => {
    mutations.forEach((m) => {
      m.addedNodes.forEach((node) => {
        if (node.nodeName === "VIDEO") attachToVideo(node);
        else if (node.querySelectorAll) node.querySelectorAll("video").forEach(attachToVideo);
      });
    });
  }).observe(document.documentElement, { childList: true, subtree: true });

  chrome.runtime.onMessage.addListener((req, _sender, sendResponse) => {
    if (req.action === "open_grabber") {
      openGrabber();
      sendResponse({ ok: true });
      return true;
    }
    return false;
  });
})();
