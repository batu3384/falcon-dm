/**
 * Smoke checks for extension media helpers.
 * Usage: node extension/smoke-test.mjs
 */
import { readFileSync } from 'fs';
import vm from 'vm';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const src = readFileSync(path.join(__dirname, 'media-utils.js'), 'utf8');
const background = readFileSync(path.join(__dirname, 'background.js'), 'utf8');
const shared = readFileSync(path.join(__dirname, 'shared.js'), 'utf8');
const pairing = readFileSync(path.join(__dirname, 'pairing.js'), 'utf8');
const api = readFileSync(path.join(__dirname, 'api.js'), 'utf8');
const content = readFileSync(path.join(__dirname, 'content.js'), 'utf8');
const manifest = JSON.parse(readFileSync(path.join(__dirname, 'manifest.json'), 'utf8'));
const sandbox = { console, globalThis: {} };
sandbox.globalThis = sandbox;
vm.createContext(sandbox);
vm.runInContext(src, sandbox);
const FM = sandbox.FalconMedia || sandbox.globalThis.FalconMedia;
if (!FM) throw new Error('FalconMedia not exported');

function assert(cond, msg) {
  if (!cond) throw new Error(msg);
}

assert(FM.isJunkUrl('https://x.com/no_input.mp3'), 'junk no_input');
assert(!FM.isJunkUrl('https://cdn.example.com/video.mp4'), 'real mp4');
assert(FM.isCapturableMedia('https://x.com/a.m3u8', 'application/vnd.apple.mpegurl'), 'hls');
const norm = FM.normalizeMediaUrl(
  'https://googlevideo.com/videoplayback?id=abc&range=0-100&other=1',
);
assert(!norm.includes('range='), 'strip range');
assert(norm.includes('id=abc'), 'keep id');
assert(manifest.permissions.includes('nativeMessaging'), 'native messaging permission');
assert(manifest.host_permissions.includes('http://127.0.0.1:14201/*'), 'localhost host permission');
assert(
  background.includes('sendNativeMessage') || pairing.includes('sendNativeMessage'),
  'native pairing proof',
);
assert(
  background.includes('withTimeout') || shared.includes('withTimeout'),
  'bounded request timeout',
);
assert(background.includes('suggest({ cancel: false })'), 'download fallback');
assert(background.includes('Promise.allSettled'), 'batch partial results');
assert(shared.includes('getCookiesHeader'), 'target cookie lookup');
assert(
  !`${background}${shared}${api}`.includes('getCookiesHeader(it.url, request.page_url)'),
  'no page cookie fallback',
);
assert(background.includes('/api/intercept'), 'media intercept endpoint');
assert(background.includes('suggest({ cancel: true })'), 'download intercept cancels browser save');
assert(background.includes('cookie_url: item.url'), 'download intercept cookie origin');
const optionsHtml = readFileSync(path.join(__dirname, 'options.html'), 'utf8');
assert(optionsHtml.includes('aria-live="polite"'), 'options status live region');
assert(background.includes('results'), 'batch result contract');
assert(background.includes('format'), 'YouTube format field');
assert(background.includes('chrome.tabs.onUpdated'), 'tab navigation cleanup');
assert(
  /importScripts\(['"]media-utils\.js['"],\s*['"]shared\.js['"],\s*['"]pairing\.js['"],\s*['"]api\.js['"]\)/.test(
    background,
  ),
  'module split',
);
const popup = readFileSync(path.join(__dirname, 'popup.js'), 'utf8');
assert(content.includes('pageUrl.split'), 'YouTube watch URL');
assert(/setAttribute\(['"]role['"],\s*['"]dialog['"]\)/.test(content), 'overlay dialog role');
assert(content.includes('aria-modal'), 'overlay aria-modal');
assert(content.includes('Escape'), 'overlay escape close');
assert(content.includes('fm-fab'), 'isolated video chip');
assert(popup.includes('resp.ok === false'), 'pause fail-closed on missing ok');
assert(popup.includes('refresh();'), 'pause refreshes connection state');
const popupHtml = readFileSync(path.join(__dirname, 'popup.html'), 'utf8');
assert(popupHtml.includes('aria-live="polite"'), 'popup status live region');
assert(content.includes('Math.min(Math.max(h, 144), 2160)'), 'bounded YouTube height');
assert(content.includes('googlevideo'), 'YouTube CDN source guard');

console.log('extension smoke ok');
