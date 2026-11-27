/**
 * Static contract checks for the browser extension (no browser needed).
 * Usage: node extension/smoke-test.mjs
 */
import { readFileSync } from 'fs';
import path from 'path';
import vm from 'vm';
import { fileURLToPath } from 'url';

const dir = path.dirname(fileURLToPath(import.meta.url));
const read = (name) => readFileSync(path.join(dir, name), 'utf8');
function assert(cond, message) {
  if (!cond) throw new Error(message);
}

const manifest = JSON.parse(read('manifest.json'));
const background = read('background.js');
const shared = read('shared.js');
const pairing = read('pairing.js');
const api = read('api.js');
const content = read('content.js');
const popup = read('popup.js');
const popupHtml = read('popup.html');
const options = read('options.js');
const optionsHtml = read('options.html');
const css = read('chrome.css');
const en = JSON.parse(read('_locales/en/messages.json'));
const tr = JSON.parse(read('_locales/tr/messages.json'));

// Manifest: only what the features use.
assert(manifest.manifest_version === 3, 'MV3');
assert(manifest.background?.service_worker === 'background.js', 'service worker');
for (const removed of ['nativeMessaging', 'clipboardRead', 'alarms']) {
  assert(!manifest.permissions.includes(removed), `${removed} permission is not requested`);
}
for (const needed of ['downloads', 'webRequest', 'cookies', 'storage', 'scripting', 'contextMenus']) {
  assert(manifest.permissions.includes(needed), `${needed} permission`);
}
assert(manifest.host_permissions.includes('http://127.0.0.1:14201/*'), 'local API host permission');
assert(background.startsWith("importScripts('media-utils.js', 'shared.js', 'pairing.js', 'api.js');"), 'worker script order');

// Pairing: code shown to the user, no native host, token only after approval.
assert(!pairing.includes('sendNativeMessage') && !background.includes('sendNativeMessage'), 'no native messaging');
assert(pairing.includes('crypto.getRandomValues'), 'pair code comes from the CSPRNG');
assert(pairing.includes("JSON.stringify({ extension_id: chrome.runtime.id, code })"), 'pair request carries id + code');
assert(pairing.includes('resp.status === 403') && pairing.includes('forgetPairCode'), 'declined code is discarded');
assert(!background.includes("action === 'save_token'"), 'no manual token pasting');
assert(background.includes('pairCode: formatPairCode(visiblePairCode())'), 'popup receives the visible code');

// Removed features stay removed.
assert(!/clipboard/i.test(shared) && !/clipboard/i.test(background), 'clipboard monitor removed');
assert(!background.includes('chrome.alarms'), 'no alarms');

// Network safety.
assert(background.includes('isPublicEnqueueUrl'), 'private URLs are never enqueued');
assert(background.includes("action === 'add_url'") && background.includes('isPublicEnqueueUrl(rawUrl)'), 'add_url checks the URL');
assert(api.includes("'X-Falcon-Token': token"), 'API calls are authenticated');
assert(api.includes('/api/downloads') && background.includes("action === 'get_downloads'"), 'popup activity endpoint');

// Popup.
for (const id of ['card-offline', 'card-pending', 'card-blocked', 'pair-code', 'grab', 'grab-links', 'downloads', 'capture', 'notice']) {
  assert(popupHtml.includes(`id="${id}"`), `popup #${id}`);
}
assert(popupHtml.includes('aria-live="polite"') && popupHtml.includes('role="alert"'), 'popup announces state and errors');
assert(popupHtml.includes('media-utils.js'), 'popup uses FalconMedia for YouTube detection');
assert(popup.includes('FalconMedia.isYoutubePageUrl'), 'popup YouTube check');
assert(popup.includes('currentWindow: true'), 'popup reads the tab of its own window');
assert(popup.includes('tabId: currentTab.id'), 'popup passes the tab id');
assert(popup.includes('12000'), 'popup messages are bounded');
assert(popup.includes("state !== 'connected'"), 'overlay opens only when connected');
assert(!popup.includes('innerHTML'), 'popup builds DOM nodes, never HTML strings');

// Options.
for (const id of ['pair', 'copy-id', 'extension-id', 'capture', 'fallback', 'capture-types', 'blocked-sites', 'notice']) {
  assert(optionsHtml.includes(`id="${id}"`), `options #${id}`);
}
assert(options.includes("send('set_fail_closed'"), 'options toggles the offline fallback');
assert(optionsHtml.includes('shared.js'), 'options reuses the shared capture-rule parser');
assert(options.includes('falconCaptureExtensions') && options.includes('falconCaptureBlockedHosts'), 'options persists capture rules');
assert(background.includes('shouldCaptureBrowserDownload'), 'automatic capture honors editable rules');
assert(shared.includes('captureHostMatches') && shared.includes('normalizeCaptureExtensions'), 'shared capture-rule helpers');

// Look and feel: system font, no remote assets.
for (const [name, html] of [['popup', popupHtml], ['options', optionsHtml]]) {
  assert(!html.includes('fonts.googleapis.com') && !/https?:\/\//.test(html.replace(/xmlns="[^"]+"/g, '').replace('http://www.w3.org/2000/svg', '')), `${name} loads no remote assets`);
}
assert(css.includes('-apple-system') && css.includes('prefers-color-scheme: dark'), 'system font + dark mode');
for (const cls of ['fm-overlay', 'fm-panel', 'fm-card', 'fm-btn-primary', 'fm-fab', 'fm-fab-dismiss', 'fm-warn', 'fm-grab-list']) {
  assert(content.includes(`.${cls}`), `overlay style .${cls}`);
}
assert(content.includes('prefers-color-scheme: dark'), 'overlay follows dark mode');
assert(!content.includes('Plus Jakarta Sans'), 'overlay uses the system font');

// Locales: parity, every used key exists, no positional placeholders.
const enKeys = Object.keys(en).sort();
assert(JSON.stringify(enKeys) === JSON.stringify(Object.keys(tr).sort()), 'en/tr key parity');
const used = new Set();
for (const source of [background, shared, pairing, api, content, popup, options, read('media-utils.js')]) {
  for (const m of source.matchAll(/\b(?:msg|t)\(\s*['"]([A-Za-z0-9_]+)['"]/g)) used.add(m[1]);
}
for (const m of JSON.stringify(manifest).matchAll(/__MSG_([A-Za-z0-9_]+)__/g)) used.add(m[1]);
for (const key of used) assert(en[key] && tr[key], `locale key ${key}`);
for (const [key, value] of Object.entries({ ...en, ...tr })) {
  assert(!/\$\d/.test(value.message), `${key} uses named placeholders`);
}
assert(/[ğışçöü]/i.test(tr.popupPendingTitle.message + tr.popupCapture.message), 'Turkish text uses Turkish letters');

// Shared code still parses as scripts (the worker would fail to start otherwise).
for (const file of ['media-utils.js', 'shared.js', 'pairing.js', 'api.js', 'background.js', 'content.js', 'popup.js', 'options.js']) {
  new vm.Script(read(file), { filename: file });
}

console.log('extension smoke ok');
