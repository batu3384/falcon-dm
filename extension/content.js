(function () {
  const CS_KEY = '__falconDmContentScript';
  const extId = chrome?.runtime?.id;
  if (extId && window[CS_KEY] === extId) return;
  if (extId) window[CS_KEY] = extId;

  const FM = window.FalconMedia;
  const TOKENS = {
    primary: '#2563EB',
    accent: '#D97706',
    success: '#22c55e',
  };

  let fabVideo = null;
  let fabHost = null;
  let fabListeners = null;
  // ponytail: in-memory only — cleared on full page reload (F5). Same as IDM hide-for-this-page.
  let fabDismissedUrl = null;
  let fabManualPos = null;
  let fabPageKey = '';

  function pageUrlKey() {
    try {
      return location.href.split('#')[0];
    } catch (_) {
      return String(location.href || '');
    }
  }

  fabPageKey = pageUrlKey();

  function syncFabPageContext() {
    const key = pageUrlKey();
    if (fabPageKey !== key) {
      fabPageKey = key;
      fabManualPos = null;
    }
  }

  function isFabDismissedHere() {
    return fabDismissedUrl === pageUrlKey();
  }

  function dismissFabForPage() {
    fabDismissedUrl = pageUrlKey();
    fabManualPos = null;
    if (fabHost) fabHost.style.display = 'none';
    teardownFabListeners();
    fabVideo = null;
  }

  function msg(key, fallback) {
    return chrome.i18n.getMessage(key) || fallback;
  }

  function esc(s) {
    return String(s || '')
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
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
        reject(new Error(msg('errorExtensionReload', 'Eklenti yenilendi — sayfayı yenileyin')));
        return;
      }
      chrome.runtime.sendMessage(payload, (response) => {
        const err = chrome.runtime.lastError;
        if (err) reject(new Error(err.message));
        else resolve(response);
      });
    });
  }

  function overlayCss() {
    return `
      :host { all: initial; }
      * { box-sizing: border-box; font-family: 'Plus Jakarta Sans', -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
      .fm-overlay {
        position: fixed; inset: 0; display: flex; align-items: flex-start; justify-content: center;
        background: rgba(12,16,24,.58);
        backdrop-filter: blur(10px); -webkit-backdrop-filter: blur(10px);
        padding: max(20px, env(safe-area-inset-top, 0px)) max(16px, env(safe-area-inset-right, 0px))
          max(20px, env(safe-area-inset-bottom, 0px)) max(16px, env(safe-area-inset-left, 0px));
        overflow-y: auto; overscroll-behavior: contain;
        animation: fmFadeIn .2s ease;
      }
      @keyframes fmFadeIn { from { opacity: 0; } to { opacity: 1; } }
      @keyframes fmSlideUp {
        from { opacity: 0; transform: translateY(12px) scale(.985); }
        to { opacity: 1; transform: translateY(0) scale(1); }
      }
      .fm-panel {
        width: min(460px, calc(100vw - 32px)); max-height: min(90dvh, 720px);
        margin: auto 0; overflow-y: auto; overscroll-behavior: contain;
        background: linear-gradient(180deg, #1e2430 0%, #171b24 100%);
        color: #f4f4f5;
        border: 1px solid rgba(255,255,255,.12);
        border-radius: 18px;
        box-shadow: 0 28px 72px rgba(8,12,24,.58), inset 0 1px 0 rgba(255,255,255,.06);
        padding: 18px 18px 16px;
        display: flex; flex-direction: column; gap: 12px;
        animation: fmSlideUp .28s cubic-bezier(.16,1,.3,1);
      }
      .fm-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; }
      .fm-title { margin: 0; font-size: 17px; font-weight: 650; letter-spacing: -.025em; color: #fff; text-wrap: pretty; }
      .fm-sub { margin: 4px 0 0; font-size: 12px; color: #9aa3b2; line-height: 1.45; }
      .fm-steps { margin: 6px 0 0; font-size: 11px; font-weight: 600; color: #93c5fd; letter-spacing: .01em; }
      .fm-close {
        width: 36px; height: 36px; border-radius: 10px; border: 1px solid rgba(255,255,255,.1);
        background: transparent; color: #e4e4e7; cursor: pointer; font-size: 20px; line-height: 1;
        transition: background .15s, transform .12s, border-color .15s;
      }
      .fm-close:hover { background: rgba(255,255,255,.08); border-color: rgba(255,255,255,.18); }
      .fm-close:active { transform: scale(.94); }
      .fm-label { font-size: 12px; font-weight: 600; color: #9aa3b2; margin-bottom: 8px; display: block; letter-spacing: .01em; }
      .fm-cards, .fm-grab-list { display: flex; flex-direction: column; gap: 8px; max-height: min(42dvh, 320px); overflow: auto; }
      .fm-card {
        display: flex; align-items: center; gap: 12px; padding: 11px 12px;
        border: 1px solid rgba(255,255,255,.1); border-radius: 12px; background: rgba(30,36,48,.85);
        cursor: pointer; transition: border-color .15s, background .15s, transform .15s;
      }
      .fm-card:hover { border-color: #3f4b63; transform: translateY(-1px); }
      .fm-card.active { border-color: ${TOKENS.primary}; background: rgba(37,99,235,.2); box-shadow: inset 0 0 0 1px rgba(37,99,235,.35); }
      .fm-card input { accent-color: ${TOKENS.primary}; width: 16px; height: 16px; flex-shrink: 0; }
      .fm-card-body { flex: 1; min-width: 0; }
      .fm-card-title { font-size: 13px; font-weight: 600; color: #fff; }
      .fm-card-meta { font-size: 12px; color: #9aa3b2; margin-top: 2px; font-variant-numeric: tabular-nums; }
      .fm-badge {
        font-size: 10px; font-weight: 700; padding: 3px 8px; border-radius: 6px;
        background: rgba(217,119,6,.2); color: #fbbf24; letter-spacing: .02em;
      }
      .fm-badge.hls { background: rgba(37,99,235,.2); color: #93c5fd; }
      .fm-badge.dash { background: rgba(168,85,247,.18); color: #d8b4fe; }
      .fm-badge.blob { background: rgba(34,197,94,.16); color: #86efac; }
      .fm-input {
        width: 100%; padding: 10px 12px; border-radius: 10px; border: 1px solid rgba(255,255,255,.12);
        background: #1e2430; color: #fff; font-size: 13px;
        transition: border-color .15s, box-shadow .15s;
      }
      .fm-input:focus { border-color: rgba(59,130,246,.55); }
      .fm-info, .fm-error, .fm-warn {
        padding: 10px 12px; border-radius: 10px; font-size: 12px; line-height: 1.45;
      }
      .fm-info { background: rgba(37,99,235,.12); border: 1px solid rgba(37,99,235,.3); color: #bfdbfe; }
      .fm-error { background: rgba(239,68,68,.12); border: 1px solid rgba(239,68,68,.35); color: #fecaca; }
      .fm-warn { background: rgba(217,119,6,.12); border: 1px solid rgba(217,119,6,.35); color: #fde68a; display: none; }
      .fm-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 2px; }
      .fm-grab-bar { display: flex; gap: 8px; }
      .fm-btn {
        min-height: 40px; padding: 8px 16px; border-radius: 10px; font-size: 13px; font-weight: 600; cursor: pointer; border: none;
        transition: filter .15s, transform .12s, box-shadow .15s;
      }
      .fm-btn-sm { min-height: 32px; padding: 6px 10px; font-size: 12px; }
      .fm-btn-ghost { background: transparent; color: #fff; border: 1px solid rgba(255,255,255,.14); }
      .fm-btn-primary {
        background: linear-gradient(180deg, #e08a12, ${TOKENS.accent});
        color: #fff;
        box-shadow: 0 4px 14px rgba(217,119,6,.35);
      }
      .fm-btn:hover { filter: brightness(1.05); }
      .fm-btn:active { transform: scale(.98); }
      .fm-btn:disabled { opacity: .6; cursor: default; filter: none; transform: none; box-shadow: none; }
      .fm-btn:focus-visible, .fm-close:focus-visible, .fm-input:focus-visible, .fm-card:focus-within {
        outline: none; box-shadow: 0 0 0 2px #171b24, 0 0 0 4px #3b82f6;
      }
      .fm-check {
        display: flex; gap: 8px; align-items: flex-start; margin: 0; font-size: 12px; cursor: pointer; color: #e4e4e7;
      }
      .fm-check span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .fm-fab {
        min-height: 44px; padding: 0 16px; border: none; border-radius: 999px; cursor: pointer;
        background: linear-gradient(180deg, #1e2430, #171b24);
        color: #fff; font-size: 12px; font-weight: 650;
        display: inline-flex; align-items: center; gap: 6px;
        box-shadow: 0 10px 28px rgba(0,0,0,.4), 0 0 0 1px rgba(255,255,255,.1);
        border: 1px solid rgba(255,255,255,.12);
        transition: border-color .15s, transform .12s, box-shadow .15s;
      }
      .fm-fab:hover {
        border-color: ${TOKENS.accent};
        box-shadow: 0 12px 32px rgba(217,119,6,.28), 0 0 0 1px rgba(217,119,6,.5);
        transform: translateY(-1px);
      }
      .fm-fab:active { transform: scale(.98); }
      .fm-fab-wrap {
        display: inline-flex; align-items: center; gap: 4px; pointer-events: auto;
        cursor: grab; user-select: none; touch-action: none;
      }
      .fm-fab-wrap.fm-dragging { cursor: grabbing; }
      .fm-fab-wrap.fm-dragging .fm-fab:hover { transform: none; box-shadow: 0 10px 28px rgba(0,0,0,.4), 0 0 0 1px rgba(255,255,255,.1); }
      .fm-fab-dismiss {
        width: 28px; height: 28px; padding: 0; border-radius: 999px; cursor: pointer;
        border: 1px solid rgba(255,255,255,.14); background: rgba(30,36,48,.92); color: #9aa3b2;
        font-size: 17px; line-height: 1; display: inline-flex; align-items: center; justify-content: center;
        flex-shrink: 0; transition: background .15s, color .15s, border-color .15s, transform .12s;
      }
      .fm-fab-dismiss:hover { background: rgba(255,255,255,.1); color: #fff; border-color: rgba(255,255,255,.22); }
      .fm-fab-dismiss:active { transform: scale(.94); }
      .fm-fab-dismiss:focus-visible {
        outline: none; box-shadow: 0 0 0 2px #171b24, 0 0 0 4px #3b82f6;
      }
      @media (prefers-color-scheme: light) {
        .fm-overlay { background: rgba(15,23,42,.42); }
        .fm-panel {
          background: linear-gradient(180deg, #fff 0%, #f5f7fb 100%);
          color: #111827; border-color: #e2e6ee;
          box-shadow: 0 24px 48px rgba(15,23,42,.16), inset 0 1px 0 rgba(255,255,255,.9);
        }
        .fm-title { color: #111827; }
        .fm-sub, .fm-label, .fm-card-meta { color: #5b6575; }
        .fm-close { color: #111827; border-color: #e2e6ee; }
        .fm-card, .fm-input { background: #f5f7fb; border-color: #e2e6ee; }
        .fm-card.active { background: rgba(37,99,235,.1); }
        .fm-card-title { color: #111827; }
        .fm-btn-ghost { color: #111827; border-color: #cfd5e0; }
        .fm-check { color: #111827; }
        .fm-fab {
          background: linear-gradient(180deg, #fff, #f5f7fb);
          color: #111827; border-color: #e2e6ee;
          box-shadow: 0 10px 24px rgba(15,23,42,.12);
        }
        .fm-fab-dismiss {
          background: rgba(255,255,255,.95); color: #5b6575; border-color: #e2e6ee;
        }
        .fm-fab-dismiss:hover { background: #fff; color: #111827; border-color: #cfd5e0; }
        .fm-fab-dismiss:focus-visible { box-shadow: 0 0 0 2px #fff, 0 0 0 4px #3b82f6; }
        .fm-btn:focus-visible, .fm-close:focus-visible, .fm-input:focus-visible, .fm-card:focus-within {
          box-shadow: 0 0 0 2px #fff, 0 0 0 4px #3b82f6;
        }
      }
      @media (prefers-reduced-motion: reduce) {
        * { transition: none !important; animation: none !important; }
      }
    `;
  }

  function injectStyles(shadow) {
    const style = document.createElement('style');
    style.textContent = overlayCss();
    shadow.appendChild(style);
  }

  function trapTab(shadow, panel, e) {
    const nodes = [...panel.querySelectorAll('button, input, textarea, select, a[href]')].filter(
      (node) => !node.disabled && node.offsetParent !== null,
    );
    if (!nodes.length) return;
    const first = nodes[0];
    const last = nodes[nodes.length - 1];
    const active = shadow.activeElement;
    if (e.shiftKey && active === first) {
      last.focus();
      e.preventDefault();
    } else if (!e.shiftKey && active === last) {
      first.focus();
      e.preventDefault();
    }
  }

  function openSheet(ariaLabel) {
    const host = el('div', { position: 'fixed', inset: '0', zIndex: '2147483647' });
    const shadow = host.attachShadow({ mode: 'open' });
    injectStyles(shadow);
    const wrap = el('div');
    wrap.className = 'fm-overlay';
    const panel = el('div');
    panel.className = 'fm-panel';
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-modal', 'true');
    panel.setAttribute('aria-label', ariaLabel);
    wrap.appendChild(panel);
    shadow.appendChild(wrap);
    function close() {
      host.remove();
    }
    wrap.addEventListener('click', (e) => {
      if (e.target === wrap) close();
    });
    shadow.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        close();
      }
      if (e.key === 'Tab') trapTab(shadow, panel, e);
    });
    document.documentElement.appendChild(host);
    return { host, shadow, panel, close };
  }

  function createModal(pageTitle, pageUrl, cookies, ua, sources) {
    if (!FM) {
      return;
    }

    let selected = FM.pickBest(sources) || sources[0] || null;
    const sheet = openSheet(msg('downloadVideo', 'Download with Falcon DM'));
    const { panel, close } = sheet;

    const head = el('div');
    head.className = 'fm-head';
    const titles = el('div');
    titles.innerHTML = `<h2 class="fm-title">${esc(msg('downloadVideo', 'Download with Falcon DM'))}</h2><p class="fm-sub">${esc(pageTitle || pageUrl)}</p><p class="fm-steps">${esc(msg('overlaySteps', 'Pick quality → Start download'))}</p>`;
    const closeBtn = el('button');
    closeBtn.className = 'fm-close';
    closeBtn.type = 'button';
    closeBtn.setAttribute('aria-label', msg('cancel', 'Cancel'));
    closeBtn.textContent = '×';
    closeBtn.onclick = close;
    head.appendChild(titles);
    head.appendChild(closeBtn);
    panel.appendChild(head);

    const errorBox = el('div', { display: 'none' });
    errorBox.className = 'fm-error';

    const nameWrap = el('div');
    nameWrap.innerHTML = `<label class="fm-label">${esc(msg('labelFilename', 'File name'))}</label>`;
    const nameInput = el('input');
    nameInput.className = 'fm-input';
    nameInput.value = FM.defaultFilename(pageTitle, selected);
    nameWrap.appendChild(nameInput);

    if (!sources.length) {
      const err = el('div');
      err.className = 'fm-info';
      err.textContent = msg(
        'errorNoValidSource',
        'No source found — play the video first, then try again',
      );
      panel.appendChild(err);
    } else {
      const info = el('div');
      info.className = 'fm-info';
      info.textContent = /youtube\.com|youtu\.be/i.test(pageUrl)
        ? msg(
            'youtubeHint',
            'YouTube: Falcon captures the signed CDN link from this tab. Pick Video+Audio for sound.',
          )
        : msg(
            'qualityHint',
            'Choose quality and format. HLS/DASH streams merge to MP4. Blob media saves from this tab (max 32 MB).',
          );
      panel.appendChild(info);

      const label = el('label');
      label.className = 'fm-label';
      label.textContent = msg('labelQuality', 'Quality & format');
      panel.appendChild(label);

      const cards = el('div');
      cards.className = 'fm-cards';

      const warn = el('div');
      warn.className = 'fm-warn';
      warn.textContent = msg(
        'videoOnlyHint',
        'This source has no audio. Pick a Video+Audio card if you want sound.',
      );

      function syncWarn() {
        const videoOnly =
          selected && !selected.muxed && !selected.isAudio && !selected.isHls && !selected.isDash && !selected.isBlob;
        warn.style.display = videoOnly ? 'block' : 'none';
      }

      function renderCards() {
        cards.innerHTML = '';
        sources.forEach((item) => {
          const card = el('label');
          card.className = 'fm-card' + (selected && selected.url === item.url ? ' active' : '');

          const radio = el('input');
          radio.type = 'radio';
          radio.name = 'falcon-quality';
          radio.checked = !!(selected && selected.url === item.url);

          const body = el('div');
          body.className = 'fm-card-body';
          body.innerHTML = `<div class="fm-card-title">${esc(item.title)}</div><div class="fm-card-meta">${esc(item.subtitle || '')}</div>`;

          const badge = el('span');
          const badgeKind = item.isBlob
            ? 'blob'
            : item.isDash
              ? 'dash'
              : item.isHls
                ? 'hls'
                : '';
          badge.className = 'fm-badge' + (badgeKind ? ` ${badgeKind}` : '');
          badge.textContent = item.isBlob
            ? msg('blobBadge', 'Blob')
            : item.isDash
              ? msg('kindDash', 'DASH stream')
              : item.isHls
                ? msg('kindStream', 'Stream')
                : item.muxed
                  ? msg('kindVideoAudio', 'Video+Audio')
                  : item.isAudio
                    ? msg('kindAudio', 'Audio')
                    : msg('kindVideoOnly', 'Video only');

          card.appendChild(radio);
          card.appendChild(body);
          card.appendChild(badge);

          card.addEventListener('click', () => {
            selected = item;
            renderCards();
            errorBox.style.display = 'none';
            nameInput.value = FM.defaultFilename(pageTitle, item);
            syncWarn();
          });

          cards.appendChild(card);
        });
      }
      renderCards();
      syncWarn();
      panel.appendChild(cards);
      panel.appendChild(warn);
    }

    panel.appendChild(nameWrap);
    panel.appendChild(errorBox);

    const actions = el('div');
    actions.className = 'fm-actions';
    const cancelBtn = el('button');
    cancelBtn.className = 'fm-btn fm-btn-ghost';
    cancelBtn.textContent = msg('cancel', 'Cancel');
    cancelBtn.onclick = close;

    const goBtn = el('button');
    goBtn.className = 'fm-btn fm-btn-primary';
    goBtn.textContent = msg('startDownload', 'Start Download');
    goBtn.onclick = async () => {
      errorBox.style.display = 'none';
      if (!selected) {
        errorBox.textContent = msg('errorNoValidSource', 'No valid source');
        errorBox.style.display = 'block';
        return;
      }
      goBtn.disabled = true;
      goBtn.textContent = msg('sending', 'Sending...');

      const isYtPage = (() => {
        try {
          return FM.isYoutubeHost(new URL(pageUrl).hostname);
        } catch {
          return false;
        }
      })();
      let downloadUrl = selected.url;
      let format = null;
      if (isYtPage) {
        downloadUrl = pageUrl.split('#')[0];
        const h = Number(selected.height) || Number(selected.label) || 720;
        const height = Math.min(Math.max(h, 144), 2160);
        format = `bestvideo[height<=${height}]+bestaudio/best[height<=${height}]/bv*+ba/b`;
      } else if (FM.isDirectGooglevideoUrl(selected.url || '')) {
        downloadUrl = FM.normalizeMediaUrl(selected.url);
      }

      try {
        const action = selected.isBlob || FM.isBlobUrl(selected.url) ? 'download_blob' : 'download_video';
        const response = await sendBg({
          action,
          url: downloadUrl,
          blob_url: selected.isBlob || FM.isBlobUrl(selected.url) ? selected.url : undefined,
          page_url: pageUrl,
          title: pageTitle,
          cookies,
          user_agent: ua,
          filename: nameInput.value,
          format,
        });
        if (response && response.success) {
          goBtn.textContent = msg('queued', 'Queued in Falcon DM');
          goBtn.style.background = TOKENS.success;
          setTimeout(close, 900);
        } else {
          throw new Error(
            (response && response.error) || msg('errorAppOffline', 'Connection failed'),
          );
        }
      } catch (e) {
        errorBox.textContent = e.message || msg('errorAppOffline', 'Connection failed');
        errorBox.style.display = 'block';
        goBtn.disabled = false;
        goBtn.textContent = msg('startDownload', 'Start Download');
      }
    };

    actions.appendChild(cancelBtn);
    actions.appendChild(goBtn);
    panel.appendChild(actions);
    closeBtn.focus();
  }

  async function openDownloadModal() {
    if (!FM) return;

    let resp;
    try {
      resp = await sendBg({ action: 'get_real_media_url', page_url: location.href });
    } catch (e) {
      const sheet = openSheet(msg('errorAppOffline', 'Connection failed'));
      const err = el('div');
      err.className = 'fm-error';
      err.textContent = e.message;
      const closeBtn = el('button');
      closeBtn.className = 'fm-btn fm-btn-primary';
      closeBtn.textContent = msg('cancel', 'Close');
      closeBtn.onclick = sheet.close;
      sheet.panel.appendChild(err);
      sheet.panel.appendChild(closeBtn);
      closeBtn.focus();
      return;
    }

    let sources = FM.groupSources([...((resp && resp.urls) || [])], (resp && resp.metaMap) || {});
    const blobSources = FM.collectPageBlobSources ? FM.collectPageBlobSources() : [];
    if (blobSources.length) sources = [...blobSources, ...sources];

    let isYtWatch = false;
    try {
      const u = new URL(location.href);
      isYtWatch =
        FM.isYoutubeHost(u.hostname) &&
        (FM.isYoutubeWatchUrl(location.href) ||
          u.hostname === 'youtu.be' ||
          u.pathname.startsWith('/shorts/'));
    } catch (_) {}

    if (isYtWatch) {
      sources = FM.youtubeFallbackSources(location.href);
    } else if (!sources.length && /youtube\.com|youtu\.be/i.test(location.href)) {
      await new Promise((r) => setTimeout(r, 1500));
      try {
        resp = await sendBg({ action: 'get_real_media_url', page_url: location.href });
        sources = FM.groupSources([...((resp && resp.urls) || [])], (resp && resp.metaMap) || {});
      } catch (_) {}
    }

    const muxed = sources.filter((s) => s.muxed && !s.isAudio);
    const hls = sources.filter((s) => s.isHls);
    const video = sources.filter((s) => !s.isAudio && !s.muxed && !s.isHls);
    const audio = sources.filter((s) => s.isAudio);
    if (muxed.length || hls.length || video.length) {
      sources = [...muxed, ...hls, ...video, ...audio];
    }

    if (!sources.length) {
      sources = FM.youtubeFallbackSources(location.href);
    }

    createModal(
      (resp && resp.title) || document.title,
      location.href,
      (resp && resp.cookies) || '',
      (resp && resp.userAgent) || navigator.userAgent,
      sources,
    );
  }

  function clampFabPos(top, left) {
    const w = fabHost?.offsetWidth || 196;
    const h = fabHost?.offsetHeight || 44;
    const pad = 8;
    return {
      top: Math.max(pad, Math.min(window.innerHeight - h - pad, top)),
      left: Math.max(pad, Math.min(window.innerWidth - w - pad, left)),
    };
  }

  function applyFabPos(pos) {
    if (!fabHost || !pos) return;
    fabHost.style.display = 'block';
    fabHost.style.top = `${pos.top}px`;
    fabHost.style.left = `${pos.left}px`;
  }

  function bindFabDrag(wrap, btn) {
    let drag = null;
    let blockClick = false;

    wrap.addEventListener('pointerdown', (e) => {
      if (e.button !== 0 || e.target.closest('.fm-fab-dismiss')) return;
      const rect = fabHost.getBoundingClientRect();
      if (!fabManualPos) fabManualPos = { top: rect.top, left: rect.left };
      drag = {
        pid: e.pointerId,
        sx: e.clientX,
        sy: e.clientY,
        ot: fabManualPos.top,
        ol: fabManualPos.left,
        moved: false,
      };
      wrap.classList.add('fm-dragging');
      wrap.setPointerCapture(e.pointerId);
      e.preventDefault();
    });

    wrap.addEventListener('pointermove', (e) => {
      if (!drag || e.pointerId !== drag.pid) return;
      const dx = e.clientX - drag.sx;
      const dy = e.clientY - drag.sy;
      if (!drag.moved && Math.hypot(dx, dy) < 5) return;
      drag.moved = true;
      fabManualPos = clampFabPos(drag.ot + dy, drag.ol + dx);
      applyFabPos(fabManualPos);
    });

    const endDrag = (e) => {
      if (!drag || e.pointerId !== drag.pid) return;
      if (drag.moved) blockClick = true;
      drag = null;
      wrap.classList.remove('fm-dragging');
      try {
        wrap.releasePointerCapture(e.pointerId);
      } catch (_) {
        /* ponytail: capture already released */
      }
    };
    wrap.addEventListener('pointerup', endDrag);
    wrap.addEventListener('pointercancel', endDrag);

    btn.addEventListener('click', (e) => {
      if (blockClick) {
        blockClick = false;
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      openDownloadModal().catch((err) => console.error('[Falcon DM]', err));
    });
  }

  function createDownloadButton() {
    const host = el('div', {
      position: 'fixed',
      zIndex: '2147483646',
      pointerEvents: 'none',
    });
    const shadow = host.attachShadow({ mode: 'open' });
    injectStyles(shadow);
    const wrap = el('div');
    wrap.className = 'fm-fab-wrap';
    wrap.setAttribute('title', msg('fabDragHint', 'Drag to move'));
    const btn = el('button');
    btn.className = 'fm-fab';
    btn.type = 'button';
    btn.setAttribute('aria-label', msg('downloadWithFalcon', 'Download with Falcon'));
    btn.setAttribute('title', msg('fabDragHint', 'Drag to move'));
    btn.innerHTML = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" aria-hidden="true"><path d="M12 3v12m0 0l-4-4m4 4l4-4M5 21h14"/></svg><span>${esc(msg('downloadWithFalcon', 'Falcon'))}</span>`;
    const dismissBtn = el('button');
    dismissBtn.className = 'fm-fab-dismiss';
    dismissBtn.type = 'button';
    dismissBtn.setAttribute('aria-label', msg('fabDismiss', 'Hide download button for this page'));
    dismissBtn.setAttribute('title', msg('fabDismissHint', 'Hidden until you refresh or open another video'));
    dismissBtn.textContent = '×';
    dismissBtn.addEventListener('click', (e) => {
      e.preventDefault();
      e.stopPropagation();
      dismissFabForPage();
    });
    wrap.appendChild(btn);
    wrap.appendChild(dismissBtn);
    bindFabDrag(wrap, btn);
    shadow.appendChild(wrap);
    return host;
  }

  function pickLargestVideo() {
    let best = null;
    let bestArea = 0;
    document.querySelectorAll('video').forEach((v) => {
      const r = v.getBoundingClientRect();
      if (r.width < 48 || r.height < 48 || r.bottom < 0 || r.top > window.innerHeight) return;
      const area = r.width * r.height;
      if (area > bestArea) {
        bestArea = area;
        best = v;
      }
    });
    return best;
  }

  function placeFabOnVideo() {
    if (!fabHost || isFabDismissedHere()) return;
    if (fabManualPos) {
      applyFabPos(clampFabPos(fabManualPos.top, fabManualPos.left));
      return;
    }
    if (!fabVideo) return;
    const r = fabVideo.getBoundingClientRect();
    if (r.width < 48 || r.height < 48) {
      fabHost.style.display = 'none';
      return;
    }
    fabHost.style.display = 'block';
    fabHost.style.top = `${Math.max(8, r.top + 8)}px`;
    fabHost.style.left = `${Math.max(8, Math.min(window.innerWidth - 196, r.right - 188))}px`;
  }

  function teardownFabListeners() {
    if (!fabListeners) return;
    window.removeEventListener('scroll', fabListeners.onMove, true);
    window.removeEventListener('resize', fabListeners.onMove);
    fabListeners.ro?.disconnect();
    fabListeners = null;
  }

  function syncVideoFab() {
    syncFabPageContext();
    if (isFabDismissedHere()) {
      if (fabHost) fabHost.style.display = 'none';
      return;
    }
    const video = pickLargestVideo();
    if (!video) {
      if (fabHost) fabHost.style.display = 'none';
      return;
    }
    if (fabVideo !== video) {
      fabManualPos = null;
      fabVideo = video;
      teardownFabListeners();
      if (!fabHost) {
        fabHost = createDownloadButton();
        document.documentElement.appendChild(fabHost);
      }
      const onMove = () => placeFabOnVideo();
      window.addEventListener('scroll', onMove, true);
      window.addEventListener('resize', onMove);
      let ro = null;
      if (typeof ResizeObserver !== 'undefined') {
        ro = new ResizeObserver(onMove);
        ro.observe(video);
      }
      fabListeners = { onMove, ro };
    }
    placeFabOnVideo();
  }

  let fabScheduled = false;
  function scheduleFabSync() {
    if (fabScheduled) return;
    fabScheduled = true;
    const run = () => {
      fabScheduled = false;
      syncVideoFab();
    };
    if (typeof requestAnimationFrame === 'function') requestAnimationFrame(run);
    else setTimeout(run, 16);
  }

  function collectGrabberLinks() {
    const seen = new Set();
    const out = [];

    function pushLink(rawUrl, filenameHint) {
      try {
        const u = new URL(rawUrl, location.href).href;
        if (seen.has(u) || !/^https?:/i.test(u)) return;
        if (FM && FM.isJunkUrl && FM.isJunkUrl(u)) return;
        seen.add(u);
        out.push({
          url: u,
          filename: (filenameHint || u.split('/').pop().split('?')[0] || 'download').slice(0, 180),
        });
      } catch (_) {}
    }

    document.querySelectorAll('a[href]').forEach((a) => {
      const hasDownload = a.hasAttribute('download');
      const u = a.href;
      if (FM && FM.isGrabberLink ? FM.isGrabberLink(u, hasDownload) : /\.(mp4|pdf|zip)(\?|#|$)/i.test(u)) {
        pushLink(u, a.getAttribute('download'));
      }
    });

    document.querySelectorAll('video source[src], audio source[src], video[src], audio[src]').forEach((el) => {
      const src = el.currentSrc || el.src || el.getAttribute('src') || '';
      if (!src || src.startsWith('blob:') || src.startsWith('data:')) return;
      pushLink(src);
    });

    document.querySelectorAll('link[rel="enclosure"][href]').forEach((link) => {
      pushLink(link.href, link.getAttribute('title'));
    });

    return out.slice(0, 100);
  }

  function openGrabber() {
    const links = collectGrabberLinks();
    const sheet = openSheet(msg('grabberTitle', 'Link Grabber'));
    const { panel, close } = sheet;

    const head = el('div');
    head.className = 'fm-head';
    const titles = el('div');
    const title = el('h2');
    title.className = 'fm-title';
    title.textContent = msg('grabberTitle', 'Link Grabber');
    titles.appendChild(title);
    const closeBtn = el('button');
    closeBtn.className = 'fm-close';
    closeBtn.type = 'button';
    closeBtn.setAttribute('aria-label', msg('cancel', 'Cancel'));
    closeBtn.textContent = '×';
    closeBtn.onclick = close;
    head.appendChild(titles);
    head.appendChild(closeBtn);
    panel.appendChild(head);

    const info = el('p');
    info.className = 'fm-sub';
    info.textContent = links.length
      ? `${links.length} ${msg('grabberFound', 'downloadable links found')}`
      : msg('grabberEmpty', 'No downloadable links found on this page');
    panel.appendChild(info);

    const list = el('div');
    list.className = 'fm-grab-list';
    const checks = [];
    links.forEach((it) => {
      const row = el('label');
      row.className = 'fm-check';
      const cb = el('input');
      cb.type = 'checkbox';
      cb.checked = true;
      checks.push({ cb, it, row });
      const span = el('span');
      span.textContent = it.filename;
      span.title = it.url;
      row.appendChild(cb);
      row.appendChild(span);
      list.appendChild(row);
    });
    if (links.length) {
      const bar = el('div');
      bar.className = 'fm-grab-bar';
      const all = el('button');
      all.className = 'fm-btn fm-btn-ghost fm-btn-sm';
      all.type = 'button';
      all.textContent = msg('grabberSelectAll', 'Select all');
      all.onclick = () =>
        checks.forEach((c) => {
          if (!c.cb.disabled) c.cb.checked = true;
        });
      const none = el('button');
      none.className = 'fm-btn fm-btn-ghost fm-btn-sm';
      none.type = 'button';
      none.textContent = msg('grabberSelectNone', 'Select none');
      none.onclick = () =>
        checks.forEach((c) => {
          if (!c.cb.disabled) c.cb.checked = false;
        });
      bar.appendChild(all);
      bar.appendChild(none);
      panel.appendChild(bar);
    }
    panel.appendChild(list);

    const actions = el('div');
    actions.className = 'fm-actions';
    const cancel = el('button');
    cancel.className = 'fm-btn fm-btn-ghost';
    cancel.textContent = msg('cancel', 'Cancel');
    cancel.onclick = close;
    const send = el('button');
    send.className = 'fm-btn fm-btn-primary';
    send.textContent = msg('grabberSend', 'Send selected');
    send.disabled = !links.length;
    send.onclick = async () => {
      const items = checks.filter((c) => c.cb.checked).map((c) => c.it);
      if (!items.length) return;
      send.disabled = true;
      try {
        const response = await sendBg({
          action: 'batch_download',
          items,
          page_url: location.href,
          cookies: '',
        });
        const results = response && Array.isArray(response.results) ? response.results : [];
        const failed = results.filter((result) => !result.ok);
        if (!response || (!response.success && !results.length)) {
          throw new Error((response && response.error) || 'failed');
        }
        checks.forEach((check) => {
          const result = results.find((item) => item.url === check.it.url);
          if (result?.ok) {
            check.cb.checked = false;
            check.cb.disabled = true;
            check.row.style.opacity = '0.5';
          }
        });
        if (!failed.length) {
          close();
          return;
        }
        info.textContent = `${failed.length} ${msg('grabberFailed', 'downloads failed')} — ${msg('grabberRetry', 'retry failed items')}`;
        info.style.color = '#f87171';
        send.disabled = false;
      } catch (e) {
        info.textContent = e.message || 'failed';
        info.style.color = '#f87171';
        send.disabled = false;
      }
    };
    actions.appendChild(cancel);
    actions.appendChild(send);
    panel.appendChild(actions);
    closeBtn.focus();
  }

  syncVideoFab();
  new MutationObserver(() => scheduleFabSync()).observe(document.documentElement, {
    childList: true,
    subtree: true,
  });
  window.addEventListener('scroll', scheduleFabSync, true);
  window.addEventListener('resize', scheduleFabSync);

  chrome.runtime.onMessage.addListener((req, _sender, sendResponse) => {
    if (req.action === 'ping') {
      sendResponse({ ok: true });
      return true;
    }
    if (req.action === 'open_grabber') {
      openGrabber();
      sendResponse({ ok: true });
      return true;
    }
    if (req.action === 'open_download_modal') {
      openDownloadModal().catch(() => {});
      sendResponse({ ok: true });
      return true;
    }
    return false;
  });
})();
