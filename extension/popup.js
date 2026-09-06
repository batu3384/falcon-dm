function t(key, fallback) {
  try {
    return chrome.i18n.getMessage(key) || fallback;
  } catch (_) {
    return fallback;
  }
}

document.documentElement.lang = (() => {
  try {
    return (chrome.i18n.getUILanguage() || 'en').slice(0, 2);
  } catch (_) {
    return 'en';
  }
})();

const $ = (id) => document.getElementById(id);
const statusEl = $('status');
const queueEl = $('queue');
const pageUrlEl = $('page-url');
const grabBtn = $('grab');
const dlUrlBtn = $('dl-url');
const reconnectBtn = $('reconnect');
const pauseBtn = $('pause');
const failClosedEl = $('fail-closed');
const clipboardMonitorEl = $('clipboard-monitor');
const bannerEl = $('banner');
const bannerTextEl = $('banner-text');
const bannerActionEl = $('banner-action');
const pendingBoxEl = $('pending-box');
const pendingTextEl = $('pending-text');
const blockedBoxEl = $('blocked-box');
const blockedTextEl = $('blocked-text');

let paused = false;
let failClosed = false;
let currentTabUrl = '';
let currentTabId = 0;

function isYoutubeUrl(url) {
  try {
    const h = new URL(url).hostname.toLowerCase();
    return (
      h === 'youtu.be' ||
      h === 'youtube.com' ||
      h.endsWith('.youtube.com') ||
      h.endsWith('.youtube-nocookie.com')
    );
  } catch {
    return /youtube\.com|youtu\.be/i.test(url || '');
  }
}

function applyStaticI18n() {
  $('app-name').textContent = t('appName', 'Falcon DM');
  grabBtn.textContent = t('popupDownloadPage', 'Download this page media');
  $('grab-hint').textContent = t(
    'popupGrabHint',
    'Play the video first, then pick quality in the overlay.',
  );
  dlUrlBtn.textContent = t('popupDownloadUrl', 'Download URL');
  reconnectBtn.textContent = t('popupReconnect', 'Reconnect');
  pauseBtn.textContent = t('popupPause', 'Pause');
  $('tab-label').textContent = t('popupCurrentTab', 'Current tab');
  $('queue-label').textContent = t('popupQueueTitle', 'Recent downloads');
  $('queue-empty').textContent = t('popupQueueEmpty', 'No recent downloads');
  $('fail-closed-label').textContent = t('popupFailClosed', 'Block when offline');
  $('fail-closed-hint').textContent = t(
    'popupFailClosedHint',
    'Cancel browser download if Falcon DM cannot receive it.',
  );
  $('clipboard-monitor-label').textContent = t(
    'popupClipboardMonitor',
    'Queue copied http(s) URLs',
  );
  $('clipboard-monitor-hint').textContent = t(
    'popupClipboardMonitorHint',
    'Checks about once per minute while connected.',
  );
  $('more-label').textContent = t('popupAdvanced', 'Advanced');
  $('settings').textContent = t('popupSettings', 'Settings');
  bannerTextEl.textContent = t(
    'popupOfflineHelp',
    'Falcon DM is offline or not paired. Open the app, then reconnect.',
  );
  bannerActionEl.textContent = t('popupReconnect', 'Reconnect');
  pendingTextEl.textContent = t(
    'popupPendingHelp',
    'Falcon DM → Settings → approve the extension.',
  );
  blockedTextEl.textContent = t(
    'popupBlockedHelp',
    'Extension not approved. Open Settings in extension options.',
  );
  $('open-options').textContent = t('popupOpenOptions', 'Open extension settings');
}

function setState(state) {
  if (!statusEl) return;
  statusEl.dataset.state = state;
  statusEl.textContent = t(
    state === 'connected'
      ? 'popupStateConnected'
      : state === 'pending'
        ? 'popupStatePending'
        : state === 'blocked'
          ? 'popupStateBlocked'
          : 'popupStateOffline',
    state === 'connected'
      ? 'Connected'
      : state === 'pending'
        ? 'Pending'
        : state === 'blocked'
          ? 'Blocked'
          : 'Offline',
  );

  // Exactly one alert surface — never "Bağlı" + offline banner together.
  if (bannerEl) bannerEl.hidden = true;
  if (pendingBoxEl) pendingBoxEl.hidden = true;
  if (blockedBoxEl) blockedBoxEl.hidden = true;
  if (state === 'offline' && bannerEl) bannerEl.hidden = false;
  else if (state === 'pending' && pendingBoxEl) pendingBoxEl.hidden = false;
  else if (state === 'blocked' && blockedBoxEl) blockedBoxEl.hidden = false;

  if (bannerActionEl) bannerActionEl.hidden = state !== 'offline';
  const connected = state === 'connected';
  // Overlay still opens offline — never mark the CTA disabled (HTML/ARIA both swallow or lie).
  grabBtn.removeAttribute('aria-disabled');
  grabBtn.classList.remove('is-blocked');
  if (reconnectBtn) reconnectBtn.hidden = connected;
  const grabHint = $('grab-hint');
  if (grabHint) {
    grabHint.textContent = connected
      ? t('popupConnectedHint', 'Connected — play the video, pick quality, download.')
      : t('popupGrabHint', 'Play the video first, then pick quality in the overlay.');
  }
}

function timeAgo(ts) {
  const s = Math.max(1, Math.round((Date.now() - ts) / 1000));
  if (s < 60) return t('popupTimeNow', 'just now');
  return t('popupTimeMin', '{n}m ago').replace('{n}', String(Math.floor(s / 60)));
}

function renderQueue(recent) {
  queueEl.innerHTML = '';
  if (!recent || !recent.length) {
    const li = document.createElement('li');
    li.className = 'empty';
    li.textContent = t('popupQueueEmpty', 'No recent downloads');
    queueEl.appendChild(li);
    return;
  }
  recent.slice(0, 3).forEach((it) => {
    const li = document.createElement('li');
    const main = document.createElement('div');
    main.className = 'qmain';
    const name = document.createElement('span');
    name.className = 'qname';
    name.textContent = it.filename || 'download';
    name.title = it.filename || '';
    const meta = document.createElement('span');
    meta.className = 'qmeta';
    meta.textContent = (it.host ? it.host + ' · ' : '') + timeAgo(it.ts);
    main.appendChild(name);
    main.appendChild(meta);
    const kind = document.createElement('span');
    kind.className = 'qkind';
    kind.textContent = it.kind === 'media' ? t('kindVideoAudio', 'Video+Audio') : 'FILE';
    li.appendChild(main);
    li.appendChild(kind);
    queueEl.appendChild(li);
  });
}

function showNotice(text) {
  const notice = $('notice');
  if (!notice) return;
  notice.hidden = false;
  notice.textContent = text;
  notice.focus();
  clearTimeout(showNotice._t);
  showNotice._t = setTimeout(() => {
    notice.hidden = true;
    notice.textContent = '';
  }, 6000);
}

function applyStatus(resp) {
  if (!resp) return;
  setState(resp.state || 'offline');
  paused = !!resp.paused;
  failClosed = !!resp.failClosed;
  pauseBtn.textContent = paused ? t('popupResume', 'Resume') : t('popupPause', 'Pause');
  if (failClosedEl) failClosedEl.checked = failClosed;
  if (clipboardMonitorEl) clipboardMonitorEl.checked = !!resp.clipboardMonitor;
  renderQueue(resp.recent || []);
}

function send(action, payload) {
  return new Promise((resolve) => {
    let done = false;
    const finish = (value) => {
      if (done) return;
      done = true;
      resolve(value);
    };
    const timer = setTimeout(() => finish(null), 12000);
    try {
      chrome.runtime.sendMessage({ action, ...payload }, (resp) => {
        clearTimeout(timer);
        finish(chrome.runtime.lastError ? null : resp);
      });
    } catch (_) {
      clearTimeout(timer);
      finish(null);
    }
  });
}

async function refresh() {
  if (statusEl) statusEl.textContent = '…';
  let resp = await send('check_status');
  if (!resp) {
    setState('offline');
    showNotice(t('errorAppOffline', 'Falcon DM is not running — open the desktop app'));
    return;
  }
  applyStatus(resp);
  if (resp.state === 'offline' || resp.state === 'pending') {
    send('auto_pair').then(async (pair) => {
      if (pair && pair.ok === false && pair.error) showNotice(pair.error);
      const next = await send('check_status');
      if (next) applyStatus(next);
    });
  }
}

async function initPage() {
  try {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    currentTabUrl = tab && tab.url ? tab.url : '';
    currentTabId = tab && tab.id ? tab.id : 0;
    pageUrlEl.textContent = currentTabUrl || '—';
    pageUrlEl.title = currentTabUrl || '';
    const okUrl = /^https?:/i.test(currentTabUrl);
    dlUrlBtn.disabled = !okUrl;
    dlUrlBtn.textContent = isYoutubeUrl(currentTabUrl)
      ? t('popupOpenPicker', 'Pick quality')
      : t('popupDownloadUrl', 'Download URL');
  } catch (_) {
    dlUrlBtn.disabled = true;
  }
}

async function openMediaPicker() {
  grabBtn.disabled = true;
  try {
    const resp = await send('grab_tab_media', { tabId: currentTabId || undefined });
    if (resp && resp.ok) {
      window.close();
      return true;
    }
    showNotice((resp && resp.error) || t('errorAppOffline', 'Failed'));
    return false;
  } finally {
    grabBtn.disabled = false;
  }
}

grabBtn.addEventListener('click', () => {
  const state = statusEl?.dataset?.state || 'offline';
  if (state === 'offline' || state === 'pending' || state === 'blocked') {
    showNotice(
      state === 'blocked'
        ? t('popupBlockedHelp', 'Extension not approved. Open Settings in extension options.')
        : state === 'pending'
          ? t('popupPendingHelp', 'Falcon DM → Settings → approve the extension.')
          : t('popupOfflineHelp', 'Falcon DM is offline or not paired. Open the app, then reconnect.'),
    );
    if (state === 'offline' || state === 'pending') {
      send('auto_pair').then(() => refresh());
    }
  }
  openMediaPicker();
});

dlUrlBtn.addEventListener('click', async () => {
  if (!currentTabUrl) return;
  if (isYoutubeUrl(currentTabUrl)) {
    await openMediaPicker();
    return;
  }
  dlUrlBtn.disabled = true;
  const resp = await send('add_url', { url: currentTabUrl });
  dlUrlBtn.disabled = false;
  if (resp && resp.success) {
    showNotice(t('sentToApp', 'Sent to Falcon DM'));
    refresh();
  } else {
    showNotice((resp && resp.error) || t('errorAppOffline', 'Failed'));
  }
});

reconnectBtn.addEventListener('click', async () => {
  reconnectBtn.disabled = true;
  await send('auto_pair');
  reconnectBtn.disabled = false;
  refresh();
});

bannerActionEl.addEventListener('click', () => {
  reconnectBtn.click();
});

pauseBtn.addEventListener('click', async () => {
  pauseBtn.disabled = true;
  const resp = await send('set_paused', { paused: !paused });
  pauseBtn.disabled = false;
  if (!resp || resp.ok === false) {
    showNotice(t('errorAppOffline', 'Failed'));
    refresh();
    return;
  }
  refresh();
});

if (failClosedEl) {
  failClosedEl.addEventListener('change', async () => {
    failClosedEl.disabled = true;
    const resp = await send('set_fail_closed', { failClosed: failClosedEl.checked });
    failClosedEl.disabled = false;
    if (!resp || resp.ok === false) {
      failClosedEl.checked = failClosed;
      showNotice(t('errorAppOffline', 'Failed'));
      refresh();
    }
  });
}

if (clipboardMonitorEl) {
  clipboardMonitorEl.addEventListener('change', async () => {
    clipboardMonitorEl.disabled = true;
    const resp = await send('set_clipboard_monitor', { enabled: clipboardMonitorEl.checked });
    clipboardMonitorEl.disabled = false;
    if (!resp || resp.ok === false) {
      clipboardMonitorEl.checked = !clipboardMonitorEl.checked;
      showNotice(t('errorAppOffline', 'Failed'));
      refresh();
    }
  });
}

$('settings').addEventListener('click', (e) => {
  e.preventDefault();
  chrome.runtime.openOptionsPage();
});

$('open-options').addEventListener('click', (e) => {
  e.preventDefault();
  chrome.runtime.openOptionsPage();
});

applyStaticI18n();
if (!statusEl || !grabBtn) {
  document.body.innerHTML =
    '<p style="padding:16px;margin:0">Falcon DM popup yüklenemedi. chrome://extensions → Reload.</p>';
} else {
  initPage();
  refresh();
}
