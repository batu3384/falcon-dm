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

const statusEl = document.getElementById('status');
const tokenEl = document.getElementById('api-token');
const extIdEl = document.getElementById('extension-id');

function setStatus(text, kind) {
  statusEl.textContent = text;
  statusEl.className = 'status-card ' + (kind || '');
}

document.getElementById('title').textContent = t('optionsTitle', 'Falcon DM Connection');
document.getElementById('desc').textContent = t(
  'optionsDesc',
  'Connects automatically when Falcon DM is open.',
);
document.getElementById('id-label').textContent = t('optionsExtensionId', 'Extension ID');
document.getElementById('id-hint').textContent = t(
  'optionsExtensionIdHint',
  'Copy to Falcon DM Settings → approve extension + install native host.',
);
document.getElementById('copy-id').textContent = t('optionsCopyId', 'Copy');
document.getElementById('token-label').textContent = t('optionsApiToken', 'API token');
document.getElementById('token-hint').textContent = t(
  'optionsApiTokenHint',
  'Copy from Falcon DM Settings → Extension API token, paste here, Save.',
);
document.getElementById('save-token').textContent = t('optionsSaveToken', 'Save token');
document.getElementById('pair').textContent = t('optionsPair', 'Connect now');
document.getElementById('test').textContent = t('optionsTest', 'Test connection');
document.getElementById('hint').textContent = t(
  'optionsHint',
  'If blocked: approve extension ID in Falcon DM Settings, then save token here.',
);
document.getElementById('fail-closed-label').textContent = t(
  'optionsFailClosed',
  'Block browser download when Falcon DM is offline',
);
document.getElementById('fail-closed-hint').textContent = t(
  'optionsFailClosedHint',
  'When enabled, intercepted downloads are cancelled instead of falling back.',
);

extIdEl.textContent = chrome.runtime.id;

document.getElementById('copy-id').addEventListener('click', async () => {
  try {
    await navigator.clipboard.writeText(chrome.runtime.id);
    setStatus(t('optionsIdCopied', 'Extension ID copied'), 'ok');
  } catch {
    setStatus(t('optionsCopyFailed', 'Copy failed'), 'err');
  }
});

const failClosedEl = document.getElementById('fail-closed');

function refresh() {
  setStatus(t('sending', 'Checking...'), '');
  chrome.runtime.sendMessage({ action: 'check_status' }, (resp) => {
    if (!resp) {
      setStatus(t('errorAppOffline', 'Falcon DM is not running'), 'err');
      return;
    }
    const labels = {
      connected: ['optionsConnected', 'Connected — downloads go to Falcon DM', 'ok'],
      pending: ['popupStatePending', 'Pending approval in Falcon DM Settings', ''],
      blocked: ['popupStateBlocked', 'Extension not approved — open Settings', 'err'],
      offline: ['errorAppOffline', 'Falcon DM is not running', 'err'],
    };
    const [key, fallback, kind] = labels[resp.state] || labels.offline;
    setStatus(t(key, fallback), kind);
  });
  chrome.runtime.sendMessage({ action: 'get_status' }, (resp) => {
    if (resp && failClosedEl) failClosedEl.checked = !!resp.failClosed;
  });
}

chrome.storage.local.get({ apiToken: '' }, (data) => {
  if (data.apiToken) tokenEl.value = data.apiToken;
});

document.getElementById('save-token').addEventListener('click', () => {
  const token = tokenEl.value.trim();
  setStatus(t('sending', 'Saving...'), '');
  chrome.runtime.sendMessage({ action: 'save_token', token }, (resp) => {
    if (resp && resp.ok) {
      setStatus(t('optionsConnected', 'Connected — downloads go to Falcon DM'), 'ok');
      return;
    }
    setStatus((resp && resp.error) || t('errorAppOffline', 'Failed'), 'err');
  });
});

document.getElementById('pair').addEventListener('click', () => {
  setStatus(t('sending', 'Connecting...'), '');
  chrome.runtime.sendMessage({ action: 'auto_pair' }, (resp) => {
    if (resp && resp.ok) {
      setStatus(t('optionsConnected', 'Connected — downloads go to Falcon DM'), 'ok');
    } else {
      setStatus(resp?.error || t('errorAppOffline', 'Connection failed'), 'err');
    }
  });
});

document.getElementById('test').addEventListener('click', refresh);

if (failClosedEl) {
  failClosedEl.addEventListener('change', () => {
    chrome.runtime.sendMessage(
      { action: 'set_fail_closed', failClosed: failClosedEl.checked },
      (resp) => {
        if (!resp || resp.ok === false) {
          failClosedEl.checked = !failClosedEl.checked;
          setStatus(resp?.error || t('errorAppOffline', 'Failed'), 'err');
        }
      },
    );
  });
}

refresh();
