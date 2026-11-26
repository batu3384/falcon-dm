function t(key, fallback) {
  try {
    return chrome.i18n.getMessage(key) || fallback;
  } catch (_) {
    return fallback;
  }
}

const lang = (() => {
  try {
    return (chrome.i18n.getUILanguage() || 'en').slice(0, 2);
  } catch (_) {
    return 'en';
  }
})();
document.documentElement.lang = lang;

const $ = (id) => document.getElementById(id);
const numberFormat = new Intl.NumberFormat(lang === 'tr' ? 'tr-TR' : 'en-US', {
  maximumFractionDigits: 1,
});

function formatBytes(bytes) {
  if (!bytes || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** i;
  return `${i === 0 ? Math.round(value) : numberFormat.format(value >= 100 ? Math.round(value) : value)} ${units[i]}`;
}

function formatSpeed(bytes) {
  return `${formatBytes(bytes)}/${lang === 'tr' ? 'sn' : 's'}`;
}

let state = 'checking';
let currentTab = { id: 0, url: '' };
let pollTimer = 0;

function isYoutubeUrl(url) {
  return !!(globalThis.FalconMedia && FalconMedia.isYoutubePageUrl && FalconMedia.isYoutubePageUrl(url));
}

function send(action, payload) {
  return new Promise((resolve) => {
    let done = false;
    const finish = (value) => {
      if (!done) {
        done = true;
        resolve(value);
      }
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

function showNotice(text, ok = false) {
  const notice = $('notice');
  notice.textContent = text;
  notice.classList.toggle('ok', ok);
  notice.hidden = false;
  clearTimeout(showNotice.timer);
  showNotice.timer = setTimeout(() => (notice.hidden = true), 6000);
}

const STATE_TEXT = {
  connected: ['popupStateConnected', 'Connected'],
  pending: ['popupStatePending', 'Waiting for approval'],
  blocked: ['popupStateBlocked', 'Not approved'],
  offline: ['popupStateOffline', 'Falcon DM is not running'],
  checking: ['popupStateChecking', 'Checking…'],
};

function applyStatus(resp) {
  state = (resp && resp.state) || 'offline';
  const [key, fallback] = STATE_TEXT[state] || STATE_TEXT.offline;
  $('dot').dataset.state = state;
  $('state-text').textContent = t(key, fallback);
  $('card-offline').hidden = state !== 'offline';
  $('card-pending').hidden = state !== 'pending';
  $('card-blocked').hidden = state !== 'blocked';
  if (state === 'pending') $('pair-code').textContent = (resp && resp.pairCode) || '— — —';
  $('capture').checked = !(resp && resp.paused);
  $('activity').hidden = state !== 'connected';
}

function statusLine(row) {
  const done = formatBytes(row.downloaded_size);
  const size = row.total_size > 0 ? `${done} / ${formatBytes(row.total_size)}` : done;
  switch (row.status) {
    case 'Downloading':
      return row.speed > 0 ? `${size} · ${formatSpeed(row.speed)}` : size;
    case 'Merging':
      return t('dlMerging', 'Finishing…');
    case 'Queued':
      return t('dlQueued', 'Waiting');
    case 'Paused':
      return `${t('dlPaused', 'Paused')} · ${size}`;
    case 'Completed':
      return formatBytes(row.total_size || row.downloaded_size);
    case 'Failed':
      return t('dlFailed', 'Failed');
    default:
      return size;
  }
}

function renderDownloads(data) {
  const list = $('downloads');
  list.replaceChildren();
  const rows = (data && data.downloads) || [];
  if (!rows.length) {
    const li = document.createElement('li');
    li.className = 'empty';
    li.textContent = t('popupNoDownloads', 'No downloads yet');
    list.appendChild(li);
    return;
  }
  for (const row of rows.slice(0, 5)) {
    const li = document.createElement('li');
    const name = document.createElement('span');
    name.className = 'dl-name';
    name.textContent = row.filename;
    name.title = row.filename;
    li.appendChild(name);
    if (row.status !== 'Completed' && row.status !== 'Failed') {
      const bar = document.createElement('div');
      bar.className = row.status === 'Downloading' || row.status === 'Merging' ? 'bar' : 'bar paused';
      const fill = document.createElement('span');
      const pct = row.total_size > 0 ? Math.min(100, (row.downloaded_size / row.total_size) * 100) : 0;
      fill.style.width = `${row.status === 'Merging' ? 100 : pct}%`;
      bar.appendChild(fill);
      li.appendChild(bar);
    }
    const meta = document.createElement('span');
    meta.className = row.status === 'Failed' ? 'dl-meta bad' : 'dl-meta';
    meta.textContent = statusLine(row);
    li.appendChild(meta);
    list.appendChild(li);
  }
  const summary = $('activity-title');
  summary.textContent =
    data.active > 0
      ? t('popupActiveSummary', '{count} downloading · {speed}')
          .replace('{count}', String(data.active))
          .replace('{speed}', formatSpeed(data.speed || 0))
      : t('popupRecent', 'Recent downloads');
}

async function refresh() {
  const resp = await send('check_status');
  applyStatus(resp);
  if (state === 'connected') renderDownloads(await send('get_downloads'));
}

function startPolling() {
  clearInterval(pollTimer);
  // Live progress while the popup is open; pairing status while waiting for approval.
  pollTimer = setInterval(() => void refresh(), 1500);
}

async function initTab() {
  try {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    currentTab = { id: (tab && tab.id) || 0, url: (tab && tab.url) || '' };
  } catch (_) {
    currentTab = { id: 0, url: '' };
  }
  const web = /^https?:/i.test(currentTab.url);
  const grab = $('grab');
  grab.disabled = !web;
  grab.textContent = isYoutubeUrl(currentTab.url)
    ? t('popupPickQuality', 'Download This Video…')
    : t('popupDownloadPage', 'Download Media on This Page…');
  $('grab-links').disabled = !web;
  let host = '';
  try {
    host = web ? new URL(currentTab.url).hostname.replace(/^www\./, '') : '';
  } catch (_) {}
  $('page-url').textContent = host || t('popupNoPage', 'Open a web page to download from it.');
  $('page-url').title = currentTab.url;
}

async function openOverlay(mode) {
  if (state !== 'connected') {
    // The overlay can only send to a connected app: connect first.
    showNotice(
      state === 'pending'
        ? t('popupPendingTitle', 'Approve this code in Falcon DM')
        : state === 'blocked'
          ? t('popupBlockedText', 'Falcon DM declined this extension or disconnected it.')
          : t('popupOfflineText', 'Downloads go to the desktop app. Start it to continue.'),
    );
    if (state === 'offline') void send('auto_pair').then(() => refresh());
    return;
  }
  const resp = await send('grab_tab_media', { tabId: currentTab.id || undefined, mode });
  if (resp && resp.ok) window.close();
  else showNotice((resp && resp.error) || t('errorMediaUtils', 'Cannot run on this page'));
}

function applyStaticText() {
  $('app-name').textContent = t('appName', 'Falcon DM');
  $('open-app').title = t('popupOpenApp', 'Open Falcon DM');
  $('open-app').setAttribute('aria-label', t('popupOpenApp', 'Open Falcon DM'));
  $('offline-title').textContent = t('popupOfflineTitle', 'Falcon DM is not running');
  $('offline-text').textContent = t(
    'popupOfflineText',
    'Downloads go to the desktop app. Start it to continue.',
  );
  $('start-app').textContent = t('popupStartApp', 'Open Falcon DM');
  $('pending-title').textContent = t('popupPendingTitle', 'Approve this code in Falcon DM');
  $('pending-text').textContent = t(
    'popupPendingText',
    'Falcon DM shows the same code. Approve it there to connect.',
  );
  $('blocked-title').textContent = t('popupBlockedTitle', 'Connection declined');
  $('blocked-text').textContent = t(
    'popupBlockedText',
    'Falcon DM declined this extension or disconnected it.',
  );
  $('new-code').textContent = t('popupNewCode', 'Request a New Code');
  $('activity-title').textContent = t('popupRecent', 'Recent downloads');
  $('capture-label').textContent = t('popupCapture', 'Send browser downloads to Falcon DM');
  $('capture-hint').textContent = t(
    'popupCaptureHint',
    'When off, Chrome downloads files itself.',
  );
  $('grab-links').textContent = t('popupGrabLinks', 'Grab Links on This Page…');
  $('settings').textContent = t('popupSettings', 'Settings');
}

$('grab').addEventListener('click', () => void openOverlay('media'));
$('grab-links').addEventListener('click', () => void openOverlay('links'));

$('start-app').addEventListener('click', async () => {
  $('start-app').disabled = true;
  const pair = await send('auto_pair');
  $('start-app').disabled = false;
  if (pair && pair.ok === false && pair.error) showNotice(pair.error);
  void refresh();
});

$('new-code').addEventListener('click', async () => {
  $('new-code').disabled = true;
  // The request runs while the popup polls and shows the new code.
  void send('auto_pair').then(() => ($('new-code').disabled = false));
  setTimeout(() => void refresh(), 500);
});

$('open-app').addEventListener('click', () => {
  void send('auto_pair');
  window.close();
});

$('capture').addEventListener('change', async (e) => {
  const enabled = e.target.checked;
  const resp = await send('set_paused', { paused: !enabled });
  if (!resp || resp.ok === false) {
    e.target.checked = !enabled;
    showNotice(t('errorAppOffline', 'Failed'));
  }
});

$('settings').addEventListener('click', () => chrome.runtime.openOptionsPage());

applyStaticText();
void initTab();
void refresh().then(startPolling);
window.addEventListener('unload', () => clearInterval(pollTimer));
