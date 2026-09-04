(function () {
  if (window.falconDmInjected) return;
  window.falconDmInjected = true;

  const FM = window.FalconMedia;
  const TOKENS = {
    primary: '#2563EB',
    accent: '#D97706',
    success: '#22c55e',
  };

  const ATTACHED_VIDEOS = new WeakSet();

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
      * { box-sizing: border-box; font-family: Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
      .fm-overlay {
        position: fixed; inset: 0; display: flex; align-items: center; justify-content: center;
        background: rgba(10,10,12,.62); padding: 16px;
      }
      .fm-panel {
        width: min(440px, calc(100vw - 32px)); max-height: min(78vh, 640px);
        overflow: auto; background: #161618; color: #f4f4f5;
        border: 1px solid rgba(255,255,255,.1); border-radius: 14px;
        box-shadow: 0 24px 64px rgba(0,0,0,.5); padding: 18px 18px 16px;
        display: flex; flex-direction: column; gap: 14px;
      }
      .fm-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; }
      .fm-title { margin: 0; font-size: 16px; font-weight: 650; letter-spacing: -.02em; color: #fff; }
      .fm-sub { margin: 4px 0 0; font-size: 12px; color: #a1a1aa; line-height: 1.4; }
      .fm-close {
        width: 32px; height: 32px; border-radius: 8px; border: 1px solid rgba(255,255,255,.1);
        background: transparent; color: #e4e4e7; cursor: pointer; font-size: 18px; line-height: 1;
      }
      .fm-label { font-size: 11px; font-weight: 700; color: #a1a1aa; text-transform: uppercase; letter-spacing: .06em; margin-bottom: 8px; display: block; }
      .fm-cards { display: flex; flex-direction: column; gap: 8px; max-height: 220px; overflow: auto; }
      .fm-card {
        display: flex; align-items: center; gap: 12px; padding: 11px 12px;
        border: 1px solid rgba(255,255,255,.1); border-radius: 10px; background: #1e1e22;
        cursor: pointer; transition: border-color .15s, background .15s;
      }
      .fm-card:hover { border-color: #3f3f46; }
      .fm-card.active { border-color: ${TOKENS.primary}; background: rgba(37,99,235,.18); }
      .fm-card input { accent-color: ${TOKENS.primary}; width: 16px; height: 16px; flex-shrink: 0; }
      .fm-card-body { flex: 1; min-width: 0; }
      .fm-card-title { font-size: 13px; font-weight: 600; color: #fff; }
      .fm-card-meta { font-size: 12px; color: #a1a1aa; margin-top: 2px; }
      .fm-badge {
        font-size: 10px; font-weight: 700; padding: 3px 8px; border-radius: 999px;
        background: rgba(217,119,6,.2); color: #fbbf24; text-transform: uppercase; letter-spacing: .04em;
      }
      .fm-badge.hls { background: rgba(37,99,235,.2); color: #93c5fd; }
      .fm-input {
        width: 100%; padding: 10px 12px; border-radius: 8px; border: 1px solid rgba(255,255,255,.12);
        background: #1e1e22; color: #fff; font-size: 13px;
      }
      .fm-info, .fm-error {
        padding: 10px 12px; border-radius: 8px; font-size: 12px; line-height: 1.45;
      }
      .fm-info { background: rgba(37,99,235,.12); border: 1px solid rgba(37,99,235,.3); color: #bfdbfe; }
      .fm-error { background: rgba(239,68,68,.12); border: 1px solid rgba(239,68,68,.35); color: #fecaca; }
      .fm-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 2px; }
      .fm-btn {
        min-height: 36px; padding: 8px 16px; border-radius: 8px; font-size: 13px; font-weight: 600; cursor: pointer; border: none;
      }
      .fm-btn-ghost { background: transparent; color: #fff; border: 1px solid rgba(255,255,255,.14); }
      .fm-btn-primary { background: ${TOKENS.accent}; color: #fff; }
      .fm-btn:disabled { opacity: .6; cursor: default; }
      .fm-btn:focus-visible, .fm-close:focus-visible, .fm-input:focus-visible, .fm-card:focus-within {
        outline: none; box-shadow: 0 0 0 2px #161618, 0 0 0 4px #3b82f6;
      }
      .fm-check {
        display: flex; gap: 8px; align-items: flex-start; margin: 6px 0; font-size: 12px; cursor: pointer; color: #e4e4e7;
      }
      .fm-fab {
        min-height: 44px; padding: 0 14px; border: none; border-radius: 999px; cursor: pointer;
        background: #161618; color: #fff; font-size: 12px; font-weight: 650;
        display: inline-flex; align-items: center; gap: 6px;
        box-shadow: 0 8px 24px rgba(0,0,0,.35); border: 1px solid rgba(255,255,255,.12);
      }
      .fm-fab:hover { border-color: ${TOKENS.accent}; }
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
    titles.innerHTML = `<h2 class="fm-title">${esc(msg('downloadVideo', 'Download with Falcon DM'))}</h2><p class="fm-sub">${esc(pageTitle || pageUrl)}</p>`;
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
            'YouTube: Falcon yt-dlp ile sayfa adresinden indirir (CDN 403 olmaz). En iyi kalite otomatik birleşir.',
          )
        : msg(
            'qualityHint',
            'Choose quality and format. HLS streams are merged automatically by Falcon DM.',
          );
      panel.appendChild(info);

      const label = el('label');
      label.className = 'fm-label';
      label.textContent = msg('labelQuality', 'Quality & format');
      panel.appendChild(label);

      const cards = el('div');
      cards.className = 'fm-cards';

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
          const metaParts = [item.subtitle];
          if (item.sizeLabel) metaParts.push(item.sizeLabel);
          body.innerHTML = `<div class="fm-card-title">${esc(item.title)}</div><div class="fm-card-meta">${esc(metaParts.join(' · '))}</div>`;

          const badge = el('span');
          badge.className = 'fm-badge' + (item.isHls ? ' hls' : '');
          badge.textContent = item.isHls ? 'HLS' : item.format;

          card.appendChild(radio);
          card.appendChild(body);
          card.appendChild(badge);

          card.addEventListener('click', () => {
            selected = item;
            renderCards();
            errorBox.style.display = 'none';
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

      const isYt =
        /youtube\.com|youtu\.be/i.test(pageUrl) ||
        (selected.url || '').includes('googlevideo') ||
        (selected.url || '').includes('videoplayback');
      let downloadUrl = isYt ? pageUrl.split('#')[0] : selected.url;
      let format = null;
      if (isYt) {
        const h = Number(selected.height) || Number(selected.label) || 1080;
        const height = Math.min(Math.max(h, 144), 2160);
        format = `bestvideo[height<=${height}]+bestaudio/best[height<=${height}]/bv*+ba/b`;
      }

      try {
        const response = await sendBg({
          action: 'download_video',
          url: downloadUrl,
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

    if (!sources.length && /youtube\.com|youtu\.be/i.test(location.href)) {
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

    createModal(
      (resp && resp.title) || document.title,
      location.href,
      (resp && resp.cookies) || '',
      (resp && resp.userAgent) || navigator.userAgent,
      sources,
    );
  }

  function createDownloadButton() {
    const host = el('div', {
      position: 'absolute',
      zIndex: '999999',
      top: '10px',
      right: '10px',
    });
    const shadow = host.attachShadow({ mode: 'open' });
    injectStyles(shadow);
    const btn = el('button');
    btn.className = 'fm-fab';
    btn.type = 'button';
    btn.setAttribute('aria-label', msg('downloadWithFalcon', 'Download with Falcon'));
    btn.innerHTML = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" aria-hidden="true"><path d="M12 3v12m0 0l-4-4m4 4l4-4M5 21h14"/></svg><span>${esc(msg('downloadWithFalcon', 'Falcon'))}</span>`;
    btn.addEventListener('click', (e) => {
      e.preventDefault();
      e.stopPropagation();
      openDownloadModal().catch((err) => console.error('[Falcon DM]', err));
    });
    shadow.appendChild(btn);
    return host;
  }

  function attachToVideo(video) {
    if (ATTACHED_VIDEOS.has(video)) return;
    ATTACHED_VIDEOS.add(video);
    const parent = video.parentElement;
    if (!parent) return;
    if (getComputedStyle(parent).position === 'static') parent.style.position = 'relative';
    const btn = createDownloadButton();
    parent.appendChild(btn);
  }

  function collectGrabberLinks() {
    const exts = /\.(mp4|mkv|webm|mov|avi|mp3|m4a|flac|zip|rar|7z|pdf|exe|dmg|pkg|iso)(\?|$)/i;
    const seen = new Set();
    const out = [];
    document.querySelectorAll('a[href]').forEach((a) => {
      try {
        const u = new URL(a.href, location.href).href;
        if (seen.has(u)) return;
        if (!/^https?:/i.test(u)) return;
        if (!exts.test(u) && !a.hasAttribute('download')) return;
        seen.add(u);
        out.push({
          url: u,
          filename: (
            a.getAttribute('download') ||
            u.split('/').pop().split('?')[0] ||
            'download'
          ).slice(0, 180),
        });
      } catch (_) {}
    });
    return out.slice(0, 50);
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

  document.querySelectorAll('video').forEach(attachToVideo);

  let pendingVideos = [];
  let flushScheduled = false;
  const scheduleFlush = self.requestIdleCallback
    ? (fn) => self.requestIdleCallback(fn, { timeout: 300 })
    : (fn) => setTimeout(fn, 300);
  function flushVideos() {
    flushScheduled = false;
    const batch = pendingVideos;
    pendingVideos = [];
    batch.forEach(attachToVideo);
  }
  new MutationObserver((mutations) => {
    for (const m of mutations) {
      for (const node of m.addedNodes) {
        if (node.nodeName === 'VIDEO') pendingVideos.push(node);
        else if (node.querySelectorAll) {
          node.querySelectorAll('video').forEach((v) => pendingVideos.push(v));
        }
      }
    }
    if (pendingVideos.length && !flushScheduled) {
      flushScheduled = true;
      scheduleFlush(flushVideos);
    }
  }).observe(document.documentElement, { childList: true, subtree: true });

  chrome.runtime.onMessage.addListener((req, _sender, sendResponse) => {
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
