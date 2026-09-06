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
const sandbox = { console, globalThis: {}, URL };
sandbox.globalThis = sandbox;
vm.createContext(sandbox);
vm.runInContext(src, sandbox);
const FM = sandbox.FalconMedia || sandbox.globalThis.FalconMedia;
if (!FM) throw new Error('FalconMedia not exported');

function assert(cond, msg) {
  if (!cond) throw new Error(msg);
}

assert(FM.isGrabberLink('https://x.com/file.docx', false), 'grabber docx');
assert(FM.isGrabberLink('https://x.com/file.bin', false) === false, 'grabber skips unknown');
assert(FM.isBlobUrl('blob:https://example.com/u'), 'blob url detect');
assert(FM.isJunkUrl('https://x.com/no_input.mp3'), 'junk no_input');
assert(!FM.isJunkUrl('blob:https://example.com/u'), 'blob not junk');
assert(FM.isCapturableMedia('https://x.com/stream.mpd', 'application/dash+xml'), 'dash capturable');
assert(!FM.isCapturableMedia('https://x.com/app/manifest/config.json', 'application/json'), 'reject generic manifest path');
assert(!FM.isCapturableMedia('https://x.com/manual.pdf', 'application/pdf'), 'reject pdf sniff');
assert(FM.shouldSniffInject('https://x.com/beacon.mp4', 'video/mp4', 512) === false, 'reject tiny progressive beacon');
assert(FM.shouldSniffInject('https://x.com/a.m3u8', 'application/vnd.apple.mpegurl', 0), 'hls inject');
assert(FM.shouldSniffInject('https://x.com/stream', 'video/mp4', 0) === false, 'reject CT-only without content-length');
const hijack = FM.hijackPayloadForFalcon(
  'https://rr1---sn.googlevideo.com/videoplayback?itag=18&id=abc',
  'https://www.youtube.com/watch?v=dQw4w9WgXcQ',
);
assert(hijack.url.includes('watch?v='), 'hijack maps googlevideo to watch page');
assert(hijack.format === null, 'hijack defers yt-dlp format to backend');
assert(background.includes('byExtensionId'), 'skip re-hijack on fail-open fallback');
assert(background.includes('hijackPayloadForFalcon'), 'hijack payload normalization');
assert(!FM.isJunkUrl('https://cdn.example.com/video.mp4'), 'real mp4');
const norm = FM.normalizeMediaUrl(
  'https://googlevideo.com/videoplayback?id=abc&range=0-100&other=1',
);
assert(FM.isDirectGooglevideoUrl('https://rr1---sn.googlevideo.com/videoplayback?itag=18&id=1'), 'direct cdn');
assert(!FM.isDirectGooglevideoUrl('https://rr1---sn.googlevideo.com/videoplayback?sabr=1&itag=18&id=1'), 'reject sabr cdn');
assert(!FM.isGooglevideoUrl('https://cdn.example.com/videoplayback?id=1'), 'reject videoplayback spoof');
assert(FM.isYoutubeHost('www.youtube.com'), 'youtube host');
assert(!FM.isYoutubeHost('evil-youtube.com'), 'reject youtube suffix spoof');
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
assert(background.includes('fallbackBrowserDownload'), 'fail-open browser re-download');
assert(
  background.indexOf('suggest({ cancel: true })') < background.indexOf("sendToFalcon('/api/add'"),
  'hijack cancels browser before falcon post',
);
assert(background.includes('shouldSniffInject'), 'strict overlay sniff gate');
assert(background.includes('media_updated'), 'overlay refresh on sniff');
assert(background.includes('Promise.allSettled'), 'batch partial results');
assert(shared.includes('getCookiesHeader'), 'target cookie lookup');
assert(shared.includes('cookieLookupUrl'), 'YouTube CDN cookie lookup');
assert(
  shared.includes('isYoutubeHost') || shared.includes('FalconMedia.isYoutubeHost'),
  'CDN cookie lookup requires YouTube page host check',
);
assert(
  background.includes('cookieLookupUrl(url, pageUrl)'),
  'download uses watch-page cookies for googlevideo',
);
assert(background.includes('/api/intercept'), 'media intercept endpoint');
assert(background.includes('suggest({ cancel: true })'), 'download intercept cancels browser save');
assert(background.includes('cookie_url: cookieLookup'), 'download intercept cookie origin');
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
assert(content.includes('pageUrl.split'), 'YouTube watch URL for yt-dlp');
assert(!content.includes('hasCdn'), 'YouTube skips raw CDN download URL');
assert(content.includes('normalizeMediaUrl(selected.url)'), 'CDN URL normalized');
assert(/setAttribute\(['"]role['"],\s*['"]dialog['"]\)/.test(content), 'overlay dialog role');
assert(content.includes('aria-modal'), 'overlay aria-modal');
assert(content.includes('Escape'), 'overlay escape close');
assert(content.includes('fm-fab'), 'isolated video chip');
assert(background.includes('getInterceptFailClosed'), 'fail-closed preference lookup');
assert(background.includes('set_fail_closed'), 'fail-closed toggle handler');
assert(background.includes('GRAB_BATCH_LIMIT'), 'grabber batch limit constant');
assert(
  Number(background.match(/GRAB_BATCH_LIMIT = (\d+)/)?.[1]) >= 100,
  'grabber batch limit raised',
);
assert(popup.includes('refresh();'), 'pause refreshes connection state');
const popupHtml = readFileSync(path.join(__dirname, 'popup.html'), 'utf8');
assert(popupHtml.includes('aria-live="polite"'), 'popup status live region');
assert(content.includes('Math.min(Math.max(h, 144), 2160)'), 'bounded YouTube height');
assert(content.includes('isDirectGooglevideoUrl'), 'YouTube CDN direct guard');
assert(content.includes('videoOnlyHint'), 'video-only audio warning');
assert(content.includes('grabberSelectAll'), 'grabber select-all');
assert(content.includes('fm-warn'), 'overlay warning style');
assert(content.includes('prefers-color-scheme: light'), 'overlay light theme');
const chromeCss = readFileSync(path.join(__dirname, 'chrome.css'), 'utf8');
assert(chromeCss.includes('.check-row'), 'popup checkbox row');
assert(manifest.permissions.includes('tabs'), 'tabs permission');
assert(manifest.permissions.includes('clipboardRead'), 'clipboard permission');
assert(manifest.permissions.includes('alarms'), 'alarms permission');
assert(src.includes('GRABBER_EXT_RE'), 'grabber extension list');
assert(src.includes('isGrabberLink'), 'grabber link helper');
assert(src.includes('collectPageBlobSources'), 'blob source collector');
assert(readFileSync(path.join(__dirname, '../src-tauri/src/util/net.rs'), 'utf8').includes('ytdlp_source_url_for_download'), 'dash ytdlp url resolver');
assert(content.includes('link[rel="enclosure"]'), 'grabber enclosure scan');
assert(readFileSync(path.join(__dirname, '../src-tauri/src/util/net.rs'), 'utf8').includes('ERR_UNSUPPORTED_MAGNET'), 'magnet error code');
assert(background.includes('download_blob'), 'blob download action');
assert(background.includes('/api/upload'), 'blob upload api');
assert(shared.includes('validate_upload_b64_len') || readFileSync(path.join(__dirname, '../src-tauri/src/local_api.rs'), 'utf8').includes('validate_upload_b64_len'), 'upload b64 guard');
assert(src.includes('BLOB_MAX_BYTES = 32'), 'blob size cap');
assert(popupHtml.includes('clipboard-monitor'), 'popup clipboard toggle');
assert(Array.isArray(manifest.content_scripts) && manifest.content_scripts.length, 'youtube content_scripts');
assert(content.includes('youtubeFallbackSources'), 'youtube fallback tiers');
assert(content.includes('let fabHost = null'), 'fab host state declared');
assert(content.includes('dismissFabForPage'), 'page-scoped fab dismiss');
assert(content.includes('fm-fab-dismiss'), 'fab dismiss control');
assert(content.includes('syncVideoFab'), 'single video fab sync');
assert(content.includes('bindFabDrag'), 'fab drag reposition');
assert(content.includes('fabManualPos'), 'fab manual position state');
assert(content.includes('syncFabPageContext'), 'fab manual pos reset on navigation');
assert(content.includes('pickEligibleVideo'), 'eligible video only');
assert(content.includes('isEligibleVideoElement'), 'fab eligibility guard');
assert(content.includes('media_updated'), 'fab resync on sniff');
assert(content.includes('pickLargestVideo') === false, 'removed naive largest-video fab');
assert(content.includes("'ping'"), 'content script ping');
assert(popupHtml.includes('id="notice"'), 'popup error notice');
assert(optionsHtml.includes('options-head'), 'options header');

console.log('extension smoke ok');
