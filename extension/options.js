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

function send(action, payload) {
  return new Promise((resolve) => {
    try {
      chrome.runtime.sendMessage({ action, ...payload }, (resp) =>
        resolve(chrome.runtime.lastError ? null : resp),
      );
    } catch (_) {
      resolve(null);
    }
  });
}

function notice(text) {
  $('notice').textContent = text;
}

const STATES = {
  connected: ['optionsConnected', 'Connected to Falcon DM', 'optionsConnectedDetail', 'Downloads go to the desktop app.'],
  pending: ['popupStatePending', 'Waiting for approval', 'optionsPendingDetail', 'Approve code {code} in Falcon DM.'],
  blocked: ['popupStateBlocked', 'Not approved', 'popupBlockedText', 'Falcon DM declined this extension or disconnected it.'],
  offline: ['popupStateOffline', 'Falcon DM is not running', 'popupOfflineText', 'Downloads go to the desktop app. Start it to continue.'],
};

async function refresh() {
  const resp = await send('check_status');
  const state = (resp && resp.state) || 'offline';
  const [key, fallback, detailKey, detailFallback] = STATES[state] || STATES.offline;
  $('dot').dataset.state = state;
  $('conn-state').textContent = t(key, fallback);
  $('conn-detail').textContent = t(detailKey, detailFallback).replace('{code}', (resp && resp.pairCode) || '');
  $('pair').hidden = state === 'connected';
  $('pair').textContent =
    state === 'blocked' ? t('popupNewCode', 'Request a New Code') : t('optionsPair', 'Connect');
  $('capture').checked = !(resp && resp.paused);
  // Checked = Chrome may download when Falcon DM is closed.
  $('fallback').checked = !(resp && resp.failClosed);
}

$('title').textContent = t('optionsTitle', 'Falcon DM');
$('desc').textContent = t('optionsDesc', 'Send downloads from your browser to Falcon DM.');
$('conn-title').textContent = t('optionsConnection', 'Connection');
$('id-label').textContent = t('optionsExtensionId', 'Extension ID');
$('extension-id').textContent = chrome.runtime.id;
$('copy-id').textContent = t('optionsCopyId', 'Copy');
$('capture-title').textContent = t('optionsCapture', 'Downloads');
$('capture-label').textContent = t('popupCapture', 'Send browser downloads to Falcon DM');
$('capture-hint').textContent = t('popupCaptureHint', 'When off, Chrome downloads files itself.');
$('fallback-label').textContent = t('optionsFallback', 'Let Chrome download when Falcon DM is closed');
$('fallback-hint').textContent = t(
  'optionsFallbackHint',
  'When off, a captured download waits until Falcon DM can take it.',
);

$('pair').addEventListener('click', async () => {
  $('pair').disabled = true;
  notice(t('optionsPairing', 'Look for the code in the popup and approve it in Falcon DM.'));
  const resp = await send('auto_pair');
  $('pair').disabled = false;
  notice(resp && resp.ok ? t('optionsConnected', 'Connected to Falcon DM') : (resp && resp.error) || '');
  void refresh();
});

$('copy-id').addEventListener('click', async () => {
  try {
    await navigator.clipboard.writeText(chrome.runtime.id);
    notice(t('optionsIdCopied', 'Extension ID copied'));
  } catch (_) {
    notice(t('optionsCopyFailed', 'Could not copy'));
  }
});

$('capture').addEventListener('change', async (e) => {
  const resp = await send('set_paused', { paused: !e.target.checked });
  if (!resp || resp.ok === false) e.target.checked = !e.target.checked;
});

$('fallback').addEventListener('change', async (e) => {
  const resp = await send('set_fail_closed', { failClosed: !e.target.checked });
  if (!resp || resp.ok === false) e.target.checked = !e.target.checked;
});

void refresh();
setInterval(() => void refresh(), 3000);
