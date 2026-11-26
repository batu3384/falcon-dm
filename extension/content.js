(function () {
  const CS_KEY = '__falconDmContentScript';
  const CS_READY = '__falconDmReady';
  const extId = chrome?.runtime?.id;
  if (!extId) return;
  // ponytail: extId alone is not enough — after MV3 reload executeScript must re-bind listeners.
  if (window[CS_KEY] === extId && window[CS_READY] === extId) return;
  window[CS_KEY] = extId;
  document.querySelectorAll('[data-falcon-fab]').forEach((node) => node.remove());

  function mediaApi() {
    return window.FalconMedia;
  }
  let FM = mediaApi();
  const TOKENS = {
    success: '#34c759',
  };

  let fabVideo = null;
  let fabHost = null;
  let fabListeners = null;
  // ponytail: in-memory only — cleared on full page reload (F5). Same as IDM hide-for-this-page.
  let fabDismissedUrl = null;
  let fabManualPos = null;
  let fabPageKey = '';

  function isYoutubePageUrl(url) {
    FM = mediaApi() || FM;
    return !!(FM && FM.isYoutubePageUrl && FM.isYoutubePageUrl(url));
  }

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
    if (tag === 'button' && !node.getAttribute('type')) node.type = 'button';
    return node;
  }

  function showFabError(text) {
    const toast = el('div', {
      position: 'fixed',
      bottom: '24px',
      left: '50%',
      transform: 'translateX(-50%)',
      zIndex: '2147483647',
      maxWidth: 'min(92vw, 420px)',
      padding: '12px 16px',
      borderRadius: '12px',
      background: 'rgba(30,36,48,.96)',
      color: '#fecaca',
      border: '1px solid rgba(239,68,68,.4)',
      fontSize: '13px',
      lineHeight: '1.45',
      boxShadow: '0 12px 32px rgba(0,0,0,.35)',
      pointerEvents: 'none',
    });
    toast.textContent = text;
    toast.setAttribute('role', 'alert');
    document.documentElement.appendChild(toast);
    setTimeout(() => toast.remove(), 5000);
  }

  function sendBg(payload) {
    return new Promise((resolve, reject) => {
      if (!chrome?.runtime?.id) {
        reject(new Error(msg('errorExtensionReload', 'Eklenti yenilendi — sayfayı yenileyin')));
        return;
      }
      chrome.runtime.sendMessage(payload, (response) => {
        const err = chrome.runtime.lastError;
        if (err) {
          const m = err.message || '';
          if (/invalidated|context/i.test(m)) {
            reject(new Error(msg('errorExtensionReload', 'Eklenti yenilendi — sayfayı yenileyin')));
            return;
          }
          reject(new Error(m));
        } else resolve(response);
      });
    });
  }

  function overlayCss() {
    // macOS sheet look, matching the desktop app. Light by default, dark with the system.
    return `
      :host { all: initial; }
      * { box-sizing: border-box; font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", "Segoe UI", sans-serif; }
      .fm-overlay {
        --accent: #007aff; --accent-hover: #0066d6; --accent-soft: rgba(0,122,255,.12);
        --bg: #ffffff; --bg-subtle: #f5f5f7; --field: rgba(0,0,0,.045); --text: #1d1d1f; --text-2: #6e6e73;
        --border: rgba(0,0,0,.14); --sep: rgba(0,0,0,.09);
        --red: #d70015; --red-soft: rgba(255,59,48,.1); --orange: #c25e00; --orange-soft: rgba(255,149,0,.12);
        position: fixed; inset: 0; display: flex; align-items: flex-start; justify-content: center;
        background: rgba(0,0,0,.22);
        padding: 56px 16px 20px; overflow-y: auto; overscroll-behavior: contain;
        animation: fmFadeIn .15s ease-out; color: var(--text); font-size: 13px; line-height: 1.38;
        -webkit-font-smoothing: antialiased;
      }
      @keyframes fmFadeIn { from { opacity: 0; } }
      @keyframes fmSheetIn { from { opacity: 0; transform: translateY(-12px) scale(.98); } }
      .fm-panel {
        width: min(440px, calc(100vw - 32px)); max-height: min(86dvh, 680px); overflow-y: auto;
        background: var(--bg); border-radius: 12px; padding: 18px 20px 16px;
        box-shadow: 0 22px 70px rgba(0,0,0,.25), 0 0 0 .5px rgba(0,0,0,.18);
        display: flex; flex-direction: column; gap: 12px;
        animation: fmSheetIn .2s cubic-bezier(.2,.9,.3,1);
      }
      .fm-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; }
      .fm-title { margin: 0; font-size: 15px; font-weight: 600; color: var(--text); }
      .fm-sub { margin: 3px 0 0; font-size: 12px; color: var(--text-2); }
      .fm-steps { margin: 6px 0 0; font-size: 11.5px; color: var(--text-2); }
      .fm-close {
        width: 26px; height: 26px; flex: none; border-radius: 50%; border: 0; padding: 0;
        background: var(--field); color: var(--text-2); cursor: default; font-size: 15px; line-height: 1;
        display: inline-grid; place-items: center;
      }
      .fm-close:hover { background: rgba(0,0,0,.09); color: var(--text); }
      .fm-label { display: block; margin: 0 0 6px; font-size: 12px; font-weight: 500; color: var(--text-2); }
      .fm-cards, .fm-grab-list {
        display: flex; flex-direction: column; max-height: min(44dvh, 330px); overflow: auto;
        border: .5px solid var(--border); border-radius: 8px; background: var(--bg);
      }
      .fm-card { display: flex; align-items: center; gap: 10px; padding: 8px 12px; cursor: default; }
      .fm-card + .fm-card { border-top: 1px solid var(--sep); }
      .fm-card:hover { background: var(--bg-subtle); }
      .fm-card.active { background: var(--accent-soft); }
      .fm-card input { accent-color: var(--accent); width: 14px; height: 14px; margin: 0; flex: none; }
      .fm-card-body { flex: 1; min-width: 0; }
      .fm-card-title { font-size: 13px; font-weight: 500; color: var(--text); }
      .fm-card-meta { font-size: 11.5px; color: var(--text-2); margin-top: 1px; font-variant-numeric: tabular-nums; }
      .fm-badge {
        flex: none; font-size: 10.5px; font-weight: 600; padding: 1px 6px; border-radius: 4px;
        background: var(--field); color: var(--text-2);
      }
      .fm-badge.hls, .fm-badge.dash { background: var(--accent-soft); color: var(--accent); }
      .fm-badge.blob { background: rgba(52,199,89,.14); color: #1f8a3b; }
      .fm-input {
        width: 100%; height: 28px; padding: 0 9px; border-radius: 6px; border: .5px solid var(--border);
        background: var(--bg); color: var(--text); font-size: 13px; outline: none;
      }
      .fm-input:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
      .fm-info, .fm-error, .fm-warn { padding: 8px 11px; border-radius: 8px; font-size: 12px; line-height: 1.45; }
      .fm-info { background: var(--bg-subtle); color: var(--text-2); }
      .fm-error { background: var(--red-soft); color: var(--red); }
      .fm-warn { background: var(--orange-soft); color: var(--orange); display: none; }
      .fm-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 2px; }
      .fm-grab-bar { display: flex; gap: 6px; }
      .fm-btn {
        height: 28px; min-width: 72px; padding: 0 12px; border-radius: 7px; font-size: 13px; font-weight: 400;
        cursor: default; border: .5px solid var(--border); background: var(--bg); color: var(--text);
        box-shadow: 0 .5px 1px rgba(0,0,0,.12); white-space: nowrap;
      }
      .fm-btn-sm { height: 24px; min-width: 0; padding: 0 9px; font-size: 12px; }
      .fm-btn-ghost:hover { background: var(--bg-subtle); }
      .fm-btn-primary { background: var(--accent); border-color: transparent; color: #fff; }
      .fm-btn-primary:hover { background: var(--accent-hover); }
      .fm-btn:disabled { opacity: .45; }
      .fm-btn:focus-visible, .fm-close:focus-visible, .fm-input:focus-visible, .fm-card:focus-within,
      .fm-fab:focus-visible, .fm-fab-dismiss:focus-visible {
        outline: 3px solid rgba(0,122,255,.45); outline-offset: 1px;
      }
      .fm-check { display: flex; gap: 8px; align-items: center; margin: 0; padding: 6px 12px; font-size: 12.5px; color: var(--text); }
      .fm-check + .fm-check { border-top: 1px solid var(--sep); }
      .fm-check input { accent-color: var(--accent); margin: 0; flex: none; }
      .fm-check span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .fm-fab {
        height: 32px; padding: 0 14px 0 12px; border-radius: 999px; cursor: default;
        background: rgba(255,255,255,.94); color: #1d1d1f; font-size: 12.5px; font-weight: 500;
        display: inline-flex; align-items: center; gap: 6px;
        border: .5px solid rgba(0,0,0,.16); box-shadow: 0 6px 20px rgba(0,0,0,.18);
        -webkit-backdrop-filter: blur(16px); backdrop-filter: blur(16px);
      }
      .fm-fab:hover { background: #fff; color: #007aff; }
      .fm-fab-wrap {
        display: inline-flex; align-items: center; gap: 4px; pointer-events: auto;
        cursor: grab; user-select: none; touch-action: none;
      }
      .fm-fab-wrap.fm-dragging { cursor: grabbing; }
      .fm-fab-dismiss {
        width: 24px; height: 24px; padding: 0; border-radius: 50%; cursor: default;
        border: .5px solid rgba(0,0,0,.16); background: rgba(255,255,255,.94); color: #6e6e73;
        font-size: 13px; line-height: 1; display: inline-grid; place-items: center; flex: none;
        box-shadow: 0 4px 12px rgba(0,0,0,.14);
      }
      .fm-fab-dismiss:hover { color: #1d1d1f; }
      @media (prefers-color-scheme: dark) {
        .fm-overlay {
          --accent: #0a84ff; --accent-hover: #3b9bff; --accent-soft: rgba(10,132,255,.22);
          --bg: #2b2b2d; --bg-subtle: rgba(255,255,255,.05); --field: rgba(255,255,255,.08);
          --text: rgba(255,255,255,.9); --text-2: rgba(235,235,245,.62);
          --border: rgba(255,255,255,.14); --sep: rgba(255,255,255,.09);
          --red: #ff6961; --red-soft: rgba(255,69,58,.16); --orange: #ffb340; --orange-soft: rgba(255,159,10,.16);
          background: rgba(0,0,0,.45);
        }
        .fm-panel { box-shadow: 0 22px 70px rgba(0,0,0,.55), 0 0 0 .5px rgba(255,255,255,.12); }
        .fm-close:hover { background: rgba(255,255,255,.14); }
        .fm-btn { background: rgba(255,255,255,.1); }
        .fm-btn-primary { background: var(--accent); }
        .fm-badge.blob { color: #32d74b; }
        .fm-fab { background: rgba(44,44,47,.92); color: rgba(255,255,255,.9); border-color: rgba(255,255,255,.14); }
        .fm-fab:hover { background: rgba(58,58,62,.96); color: #fff; }
        .fm-fab-dismiss { background: rgba(44,44,47,.92); color: rgba(235,235,245,.62); border-color: rgba(255,255,255,.14); }
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
      (node) => !node.disabled && !node.hidden && node.getClientRects().length > 0,
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
    const prev = document.activeElement;
    document.querySelectorAll('[data-falcon-overlay]').forEach((node) => node.remove());
    const host = el('div', { position: 'fixed', inset: '0', zIndex: '2147483647' });
    host.setAttribute('data-falcon-overlay', '1');
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
      const restore = prev && prev.isConnected ? prev : null;
      host.remove();
      try {
        restore?.focus?.();
      } catch (_) {}
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
    FM = mediaApi() || FM;
    if (!FM) {
      showFabError(msg('errorMediaUtils', 'Falcon module failed to load — reload extension'));
      throw new Error(msg('errorMediaUtils', 'Falcon module failed to load — reload extension'));
    }

    let selected = FM.pickBest(sources) || sources[0] || null;
    const sheet = openSheet(msg('downloadVideo', 'Download with Falcon DM'));
    const { panel, close } = sheet;

    const head = el('div');
    head.className = 'fm-head';
    const titles = el('div');
    titles.innerHTML = `<h2 class="fm-title" id="fm-dialog-title">${esc(msg('downloadVideo', 'Download with Falcon DM'))}</h2><p class="fm-sub">${esc(pageTitle || pageUrl)}</p><p class="fm-steps">${esc(msg('overlaySteps', 'Pick quality → Start download'))}</p>`;
    const closeBtn = el('button');
    closeBtn.className = 'fm-close';
    closeBtn.type = 'button';
    closeBtn.setAttribute('aria-label', msg('cancel', 'Cancel'));
    closeBtn.textContent = '×';
    closeBtn.onclick = close;
    head.appendChild(titles);
    head.appendChild(closeBtn);
    panel.appendChild(head);
    panel.setAttribute('aria-labelledby', 'fm-dialog-title');
    panel.removeAttribute('aria-label');

    const errorBox = el('div', { display: 'none' });
    errorBox.className = 'fm-error';
    errorBox.setAttribute('role', 'alert');

    const nameWrap = el('div');
    const nameInput = el('input');
    nameInput.id = 'fm-filename';
    nameInput.className = 'fm-input';
    nameInput.setAttribute('aria-label', msg('labelFilename', 'File name'));
    nameInput.value = FM.defaultFilename(pageTitle, selected);
    nameWrap.innerHTML = `<label class="fm-label" for="fm-filename">${esc(msg('labelFilename', 'File name'))}</label>`;
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
      info.textContent = isYoutubePageUrl(pageUrl)
        ? msg(
            'youtubeHint',
            'YouTube: Falcon captures the signed CDN link from this tab. Pick Video+Audio for sound.',
          )
        : msg(
            'qualityHint',
            'Choose quality and format. HLS/DASH streams merge to MP4. Blob media saves from this tab (max 32 MB).',
          );
      panel.appendChild(info);

      const label = el('div');
      label.className = 'fm-label';
      label.id = 'fm-quality-label';
      label.textContent = msg('labelQuality', 'Quality & format');
      panel.appendChild(label);

      const cards = el('div');
      cards.className = 'fm-cards';
      cards.setAttribute('role', 'radiogroup');
      cards.setAttribute('aria-labelledby', 'fm-quality-label');

      const warn = el('div');
      warn.className = 'fm-warn';
      warn.setAttribute('role', 'status');
      warn.setAttribute('aria-live', 'polite');
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
          // YouTube fallback cards share the watch URL — identity, not url, is the selection key.
          const on = selected === item;
          card.className = 'fm-card' + (on ? ' active' : '');

          const radio = el('input');
          radio.type = 'radio';
          radio.name = 'falcon-quality';
          radio.checked = on;

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

      const payload =
        typeof FM.overlayEnqueuePayload === 'function'
          ? FM.overlayEnqueuePayload(pageUrl, selected)
          : { url: selected.url, format: null };
      const { url: downloadUrl, format } = payload;

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
    nameInput.focus();
  }

  async function openDownloadModal() {
    FM = mediaApi() || FM;
    if (!FM) {
      const err = msg('errorMediaUtils', 'Falcon module failed to load — reload extension');
      showFabError(err);
      throw new Error(err);
    }

    const isYtWatch = !!(FM.isYoutubeWatchUrl && FM.isYoutubeWatchUrl(location.href));

    // YouTube watch: overlay from yt-dlp cards immediately — sniff wait was thrown away anyway.
    if (isYtWatch) {
      createModal(
        document.title,
        location.href,
        '',
        navigator.userAgent,
        FM.youtubeFallbackSources(location.href),
      );
      return;
    }

    const loading = openSheet(msg('downloadVideo', 'Download with Falcon DM'));
    const status = el('div');
    status.className = 'fm-info';
    status.textContent = msg('sending', 'Loading...');
    loading.panel.appendChild(status);

    let resp;
    try {
      resp = await sendBg({ action: 'get_real_media_url', page_url: location.href });
    } catch (e) {
      status.className = 'fm-error';
      status.textContent = e.message;
      const closeBtn = el('button');
      closeBtn.className = 'fm-btn fm-btn-primary';
      closeBtn.textContent = msg('cancel', 'Close');
      closeBtn.onclick = loading.close;
      loading.panel.appendChild(closeBtn);
      closeBtn.focus();
      return;
    }

    let sources = FM.groupSources([...((resp && resp.urls) || [])], (resp && resp.metaMap) || {});
    const blobSources = FM.collectPageBlobSources ? FM.collectPageBlobSources() : [];
    if (blobSources.length) sources = [...blobSources, ...sources];

    if (!sources.length && isYoutubePageUrl(location.href)) {
      sources = FM.youtubeFallbackSources(location.href);
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

    loading.close();
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
    });

    wrap.addEventListener('pointermove', (e) => {
      if (!drag || e.pointerId !== drag.pid) return;
      const dx = e.clientX - drag.sx;
      const dy = e.clientY - drag.sy;
      if (!drag.moved && Math.hypot(dx, dy) < 5) return;
      if (!drag.moved) {
        drag.moved = true;
        wrap.classList.add('fm-dragging');
        try {
          wrap.setPointerCapture(e.pointerId);
        } catch (_) {}
      }
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
      openDownloadModal().catch((err) => {
        console.error('[Falcon DM]', err);
        showFabError(err?.message || msg('errorAppOffline', 'Connection failed'));
      });
    });
  }

  function createDownloadButton() {
    const host = el('div', {
      position: 'fixed',
      zIndex: '2147483646',
      pointerEvents: 'none',
    });
    host.setAttribute('data-falcon-fab', '1');
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

  function pickEligibleVideo() {
    FM = mediaApi() || FM;
    let best = null;
    let bestArea = 0;
    const pageUrl = location.href;
    document.querySelectorAll('video').forEach((v) => {
      if (!FM || !FM.isEligibleVideoElement(v, pageUrl)) return;
      const r = v.getBoundingClientRect();
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
    const minPx = (FM && FM.MIN_FAB_PX) || 120;
    const r = fabVideo.getBoundingClientRect();
    if (r.width < minPx || r.height < minPx) {
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
    const video = pickEligibleVideo();
    if (!video) {
      fabVideo = null;
      teardownFabListeners();
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
        if (!FM || !FM.isPublicHttpUrl || !FM.isPublicHttpUrl(u)) return;
        if (FM.isJunkUrl && FM.isJunkUrl(u)) return;
        seen.add(u);
        out.push({
          url: u,
          filename: (filenameHint || (FM && FM.urlBasename && FM.urlBasename(u)) || 'download').slice(
            0,
            180,
          ),
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
    title.id = 'fm-dialog-title';
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
    panel.setAttribute('aria-labelledby', 'fm-dialog-title');
    panel.removeAttribute('aria-label');

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
        info.setAttribute('role', 'alert');
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
    const firstCb = list.querySelector('input');
    (firstCb || send || closeBtn).focus();
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
    if (req.action === 'media_updated') {
      scheduleFabSync();
      sendResponse({ ok: true });
      return true;
    }
    if (req.action === 'open_grabber') {
      openGrabber();
      sendResponse({ ok: true });
      return true;
    }
    if (req.action === 'open_download_modal') {
      openDownloadModal()
        .then(() => sendResponse({ ok: true }))
        .catch((e) => {
          showFabError(e?.message || msg('errorAppOffline', 'Failed'));
          sendResponse({ ok: false, error: e?.message || 'Failed' });
        });
      return true;
    }
    return false;
  });

  window.__falconDmReady = extId;
})();
