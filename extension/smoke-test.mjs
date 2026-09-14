/**
 * Smoke checks for extension media helpers.
 * Usage: node extension/smoke-test.mjs
 */
import { readFileSync } from 'fs';
import { spawnSync } from 'child_process';
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
assert(!FM.isJunkUrl('https://cdn.example.com/doubleclick-audit.pdf'), 'doubleclick substring is not junk');
assert(!FM.isJunkUrl('https://cdn.example.com/static/video.mp4'), 'non-youtube /static/ is not junk');
assert(!FM.isJunkUrl('https://cdn.example.com/file.mp3?ref=youtube.com'), 'youtube.com in query is not junk');
assert(!FM.isJunkUrl('https://docs.example.com/s/search/guide.pdf'), 'non-youtube /s/search/ is not junk');
assert(!FM.isJunkUrl('https://cdn.example.com/api/log_event/export.csv'), 'non-youtube log_event is not junk');
assert(FM.isJunkUrl('https://ad.doubleclick.net/track'), 'doubleclick host is junk');
assert(FM.isJunkUrl('https://www.youtube.com/youtubei/v1/player'), 'youtubei is junk');
assert(FM.isJunkUrl('https://www.youtube.com/ptracking?event=1'), 'youtube ptracking is junk');
assert(!FM.isCapturableMedia('https://evil.com/videoplayback?id=1', 'application/octet-stream'), 'videoplayback path spoof is not sniffable');
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
const fallback = FM.youtubeFallbackSources('https://www.youtube.com/watch?v=dQw4w9WgXcQ');
assert(fallback.length >= 4, 'watch page yt-dlp quality cards');
assert(fallback.every((s) => s.muxed && s.url.includes('watch?v=')), 'fallback is muxed watch URL');
assert(FM.youtubeFallbackSources('https://www.youtube.com/').length === 0, 'home has no fallback cards');
assert(FM.youtubeFallbackSources('https://evil.com/shorts/x').length === 0, 'fallback rejects shorts spoof');
assert(FM.isYoutubeWatchPage('https://www.youtube.com/watch?v=abc'), 'watch page detect');
assert(!FM.isYoutubeWatchPage('https://www.youtube.com/'), 'home is not watch');
assert(FM.isYoutubeWatchPage('https://www.youtube.com/shorts/abc'), 'youtube shorts is watch');
assert(!FM.isYoutubeWatchPage('https://evil.com/shorts/abc'), 'shorts path spoof rejected');
assert(
  FM.overlayEnqueuePayload('https://evil.com/shorts/abc', { url: 'https://cdn.example.com/a.mp4' })
    .url === 'https://cdn.example.com/a.mp4',
  'evil shorts overlay keeps media URL',
);
assert(content.includes('nameInput.focus()'), 'overlay focuses filename not close');
assert(content.includes("role', 'alert'") || content.includes('role="alert"'), 'overlay error is alert');
assert(content.includes("role', 'radiogroup'") || content.includes('role="radiogroup"'), 'overlay quality is radiogroup');
assert(content.includes("info.setAttribute('role', 'alert')"), 'grabber failure is announced');
assert(background.includes('byExtensionId'), 'skip re-hijack on fail-open fallback');
assert(background.includes('hijackPayloadForFalcon'), 'hijack payload normalization');
assert(!FM.isJunkUrl('https://cdn.example.com/video.mp4'), 'real mp4');
const norm = FM.normalizeMediaUrl(
  'https://googlevideo.com/videoplayback?id=abc&range=0-100&other=1',
);
assert(FM.isJunkHijackFilename('videoplayback', 'https://googlevideo.com/videoplayback?id=1'), 'cdn placeholder junk');
assert(!FM.isJunkHijackFilename('report.pdf', 'https://x.com/report.pdf'), 'real filename kept');
assert(!FM.isJunkHijackFilename('My Setup (2024).exe', 'https://x.com/a'), 'parens kept not junk');
assert(FM.urlBasename('https://x.com/a/b.zip?x=1') === 'b.zip', 'urlBasename strips query');
assert(FM.urlBasename('not a url') === 'not a url', 'urlBasename never throws on junk');
assert(!/\.pop\(\)\.split\(/.test(background + shared + content), 'no unguarded pop().split on download URLs');
assert(background.includes('stashPendingChromeFilename'), 'sync chrome filename before async hijack');
assert(background.includes('pendingChromeNames'), 'preserve determining filename');
assert(!FM.isDirectGooglevideoUrl('https://rr1---sn.googlevideo.com/videoplayback?sabr=1&itag=18&id=1'), 'reject sabr cdn');
assert(!FM.isGooglevideoUrl('https://cdn.example.com/videoplayback?id=1'), 'reject videoplayback spoof');
assert(FM.isYoutubePageUrl('https://www.youtube.com/watch?v=1'), 'youtube page url helper');
assert(!FM.isYoutubePageUrl('https://evil.com/youtube.com'), 'youtube path spoof rejected');
assert(FM.isBlockedLiteralHost('127.0.0.1'), 'block loopback');
assert(FM.isBlockedLiteralHost('100.64.0.1'), 'block CGNAT');
assert(FM.isBlockedLiteralHost('fc00::1'), 'block IPv6 ULA');
assert(!FM.isBlockedLiteralHost('8.8.8.8'), 'allow public ipv4');
assert(!FM.isPublicHttpUrl('http://192.168.0.5/a.zip'), 'reject RFC1918 url');
assert(FM.isBlockedLiteralHost('0.0.0.1'), 'block 0.0.0.0/8');
assert(FM.isBlockedLiteralHost('224.0.0.1'), 'block multicast v4');
assert(FM.isBlockedLiteralHost('foo.localhost'), 'block localhost subdomain');
assert(FM.isBlockedLiteralHost('ff02::1'), 'block ipv6 multicast');
assert(FM.isBlockedLiteralHost('::ffff:7f00:1'), 'block hex-mapped loopback');
assert(!FM.isBlockedLiteralHost('::ffff:808:808'), 'allow hex-mapped public v4');
assert(src.includes('o[0] === 100') && readFileSync(path.join(__dirname, '../src/lib/publicHttpUrl.ts'), 'utf8').includes('o[0] === 100'), 'CGNAT literal block stays in sync JS↔TS');
assert(FM.isYoutubeHost('www.youtube.com'), 'youtube host');
assert(!FM.isYoutubeHost('evil-youtube.com'), 'reject youtube suffix spoof');
assert(FM.hostLabel('https://www.youtube.com/watch?v=1') === 'YouTube', 'youtube host label');
assert(FM.hostLabel('https://evil-youtube.com/a') !== 'YouTube', 'host label rejects youtube suffix spoof');
assert(FM.hostLabel('https://cdn.example.com/videoplayback') !== 'YouTube CDN', 'host label rejects videoplayback spoof');
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
assert(background.includes('suggest();'), 'download passthrough uses bare suggest()');
const hijackListen = background.slice(
  background.indexOf('chrome.downloads.onDeterminingFilename.addListener'),
  background.indexOf('chrome.downloads.onCreated.addListener'),
);
assert(
  hijackListen.indexOf('markHijackStarted(item);') <
    hijackListen.lastIndexOf('runHijack(item, { skipDedup: true })'),
  'primary hijack marks dedup before runHijack',
);
assert(background.includes('fallbackBrowserDownload'), 'fail-open restarts Chrome download');
assert(!background.includes('pauseBrowserDownload'), 'IDM model does not pause/resume race');
assert(!background.includes('resumeBrowserDownload'), 'IDM model does not resume cancelled rows');
assert(
  /suggest\(\);[\s\S]*?abortAndEraseBrowserDownload\(item\.id\);[\s\S]*?runHijack\(item, \{ skipDedup: true \}\)/.test(
    hijackListen,
  ),
  'IDM: cancel Chrome first then enqueue Falcon',
);
assert(
  background.includes('opts.skipDedup || opts.skipDedupCheck') ||
    background.includes('skipDedup = !!(opts.skipDedup'),
  'runHijack honors skipDedup from listeners',
);
assert(!background.includes('blockBrowser'), 'removed falconReachable kill switch on POST errors');
assert(background.includes('hijackInFlight'), 'onCreated dedupes against active hijack');
assert(
  !hijackListen.includes('hijackRecentlyHandled(item) || hijackInFlight'),
  'same-URL retry still enqueues after prior hijack',
);
assert(
  shared.includes('falconInterceptFailClosed: true'),
  'SW restore defaults fail-closed like getInterceptFailClosed',
);
const hijackBlock = background.slice(
  background.indexOf('chrome.downloads.onDeterminingFilename.addListener'),
  background.indexOf('chrome.contextMenus.onClicked.addListener'),
);
assert(
  !/suggest\(\{\s*cancel:/.test(hijackBlock),
  'hijack must not use invalid suggest({ cancel }) API',
);
assert(
  shared.includes("windowTypes: ['normal']") || shared.includes('windowType: \'normal\''),
  'active tab lookup ignores extension popup window',
);
assert(background.includes('request.tabId'), 'grab_tab_media uses popup-provided tabId');
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
assert(popup.includes('currentWindow: true'), 'popup queries opener window not lastFocused');
assert(popup.includes('tabId:'), 'popup sends tabId with grab_tab_media');
assert(!/grabBtn\.disabled = state === 'offline'/.test(popup), 'offline must not disable primary CTA clicks');
assert(!/setAttribute\(['"]aria-disabled['"],\s*connected \? ['"]false['"] : ['"]true['"]\)/.test(popup), 'CTA must not lie aria-disabled when clickable');
assert(popup.includes('12000'), 'popup sendMessage is bounded');
assert(
  popup.includes('failClosedEl.checked = !failClosed') &&
    popup.includes("failClosed: !failClosedEl.checked"),
  'popup checkbox is allow-fallback (inverted fail-closed)',
);
const grabClick = popup.slice(
  popup.indexOf('grabBtn.addEventListener'),
  popup.indexOf('dlUrlBtn.addEventListener'),
);
assert(grabClick.includes('openMediaPicker()'), 'primary CTA opens overlay when connected');
assert(/return;\n\s*}\n\s*openMediaPicker/.test(grabClick), 'overlay skipped until Falcon is connected');
assert(content.includes('selected === item'), 'quality selection uses object identity');
assert(!content.includes('hasCdn'), 'YouTube skips raw CDN download URL');
assert(/setAttribute\(['"]role['"],\s*['"]dialog['"]\)/.test(content), 'overlay dialog role');
assert(content.includes('aria-modal'), 'overlay aria-modal');
assert(content.includes('Escape'), 'overlay escape close');
assert(content.includes('fm-fab'), 'isolated video chip');
assert(background.includes('runHijack'), 'hijack logic centralized');
assert(background.includes('hijackInFlight'), 'in-flight hijack guard');
assert(
  background.includes('scheduleCreatedBackup') && background.includes('awaitingFilename'),
  'onCreated waits for Chrome filename before Falcon enqueue',
);
assert(
  !/onCreated\.addListener[\s\S]*?markHijackStarted\(item\);\s*\n\s*stashPendingChromeFilename\(item\);\s*\n\s*abortAndEraseBrowserDownload\(item\.id\);\s*\n\s*await runHijack/.test(
    background,
  ),
  'onCreated must not enqueue immediately with empty filename',
);
assert(api.includes("connectionState === 'connected'"), 'connected path skips health RTT');
assert(background.includes('removeFile'), 'complete hijacks delete file from disk');
assert(!background.includes('queueMicrotask(() => {\n    chrome.downloads'), 'abort does not defer cancel');
assert(background.includes('downloads.onChanged'), 'watch hijacked ids until purged');
assert(background.includes('eraseBrowserDownload'), 'hijack clears ghost Chrome download row');
assert(background.includes('interceptQueued'), 'hijack success explains Chrome cancel');
assert(background.includes('set_fail_closed'), 'fail-closed toggle handler');
assert(background.includes('GRAB_BATCH_LIMIT'), 'grabber batch limit constant');
assert(
  content.includes('!FM || !FM.isPublicHttpUrl || !FM.isPublicHttpUrl(u)'),
  'grabber fail-closed without FalconMedia public URL helper',
);
assert(
  background.includes('isPublicHttpUrl') && background.includes("action === 'batch_download'"),
  'batch enqueue requires public URL',
);
assert(background.includes('isPublicEnqueueUrl'), 'shared public URL gate for hijack/add/context');
assert(
  background.includes("action === 'download_video'") && background.includes('!isPublicEnqueueUrl(rawUrl)'),
  'overlay enqueue fail-closed without FalconMedia',
);
assert(
  background.includes("action === 'batch_download'") && background.includes('!isPublicEnqueueUrl(it.url)'),
  'batch enqueue fail-closed without FalconMedia',
);
assert(
  /isPublicHttpUrl\(url\);[\s\S]{0,80}return false;/.test(background),
  'isPublicEnqueueUrl fail-closed if FalconMedia helper missing',
);
assert(background.includes("action === 'add_url'") && background.includes('isPublicEnqueueUrl(rawUrl)'), 'add_url rejects private URLs');
assert(
  background.includes("clipboardMonitor: await getClipboardMonitorEnabled()") &&
    background.includes("action === 'check_status'"),
  'popup check_status reports clipboard monitor',
);
assert(background.includes('abortedBrowserIds.has(item.id)'), 'backup/onCreated skip already-aborted Chrome rows');
assert(background.includes('id:${item.id}') || background.includes('`id:${item.id}`'), 'hijack in-flight keyed by Chrome download id');
assert(
  Number(background.match(/GRAB_BATCH_LIMIT = (\d+)/)?.[1]) >= 100,
  'grabber batch limit raised',
);
assert(popup.includes('refresh();'), 'pause refreshes connection state');
const popupHtml = readFileSync(path.join(__dirname, 'popup.html'), 'utf8');
assert(popupHtml.includes('aria-live="polite"'), 'popup status live region');
assert(popupHtml.includes('role="alert"'), 'popup error is announced');
assert(
  popupHtml.includes('id="pending-box" class="pending-box" hidden role="status"'),
  'pending pairing is a status region',
);
assert(
  popupHtml.includes('id="blocked-box" class="blocked-box" hidden role="alert"'),
  'blocked pairing is announced',
);
assert(src.includes('Math.min(Math.max(h, 144), 2160)'), 'bounded YouTube height');
assert(src.includes('isDirectGooglevideoUrl'), 'YouTube CDN direct guard');
assert(content.includes('videoOnlyHint'), 'video-only audio warning');
assert(content.includes('grabberSelectAll'), 'grabber select-all');
assert(content.includes('fm-warn'), 'overlay warning style');
assert(content.includes('prefers-color-scheme: light'), 'overlay light theme');
assert(
  content.includes("tag === 'button'") && content.includes("node.type = 'button'"),
  'content buttons default to type=button',
);
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
{
  const bind = content.slice(
    content.indexOf('function bindFabDrag'),
    content.indexOf('function createDownloadButton'),
  );
  const down = bind.slice(bind.indexOf('pointerdown'), bind.indexOf('pointermove'));
  assert(!down.includes('preventDefault'), 'FAB pointerdown must not swallow click');
  assert(!down.includes('setPointerCapture'), 'FAB capture only after drag, not on click');
  assert(bind.includes('openDownloadModal'), 'FAB click opens overlay');
}
{
  const open = content.slice(
    content.indexOf('async function openDownloadModal'),
    content.indexOf('function clampFabPos'),
  );
  assert(
    open.indexOf('youtubeFallbackSources') < open.indexOf('get_real_media_url'),
    'YouTube overlay opens before media sniff wait',
  );
}
assert(content.includes('fabManualPos'), 'fab manual position state');
assert(content.includes('syncFabPageContext'), 'fab manual pos reset on navigation');
assert(content.includes('for="fm-filename"') || content.includes("for='fm-filename'"), 'overlay filename label is associated');
assert(!content.includes('/youtube\\.com|youtu\\.be/i.test(pageUrl)'), 'no youtube path spoof on pageUrl');
assert(popup.includes('FalconMedia.isYoutubePageUrl'), 'popup youtube uses FalconMedia host parse');
assert(popupHtml.includes('media-utils.js'), 'popup loads media-utils for youtube host parse');
assert(!/return \/youtube\\.com/.test(popup), 'popup must not regex-fallback youtube');
assert(!/return \/youtube\\.com/.test(background), 'background must not regex-fallback youtube');
assert(background.includes('isYoutubePage(pageUrl)'), 'player_response inject uses host parse');
assert(shared.includes('prefsHydrated'), 'SW waits for intercept prefs hydrate');
assert(shared.includes("falconInterceptPaused: false"), 'pause preference hydrates from local');
assert(background.includes("falconInterceptPaused: next"), 'pause persists to local storage');
assert(shared.includes('hasLocalPause'), 'pause hydrate distinguishes missing local from Resume');
assert(shared.includes("session.remove('falconInterceptPaused')"), 'pause migrate clears session so Resume cannot resurrect');
assert(background.includes("session.remove('falconInterceptPaused')"), 'set_paused clears session pause');
assert(
  /action === 'check_status'[\s\S]{0,120}await prefsReady/.test(background),
  'check_status waits for prefs hydrate',
);
assert(shared.includes('markPauseWrite'), 'pause write wins over stale hydrate');
assert(shared.includes('failClosedDirty'), 'fail-closed hydrate is independent of pause');
assert(hijackListen.includes('prefsHydrated') || hijackListen.includes('prefsReady'), 'hijack waits prefs');
assert(!shared.includes('function isBlockedLiteralHost'), 'clipboard host block lives in FalconMedia');
assert(shared.includes('!fm || !fm.isPublicHttpUrl || !fm.isPublicHttpUrl(url)'), 'clipboard fail-closed without FalconMedia');
assert(!shared.includes('100 * 1024 * 1024'), 'blob fetch cap matches 32MB backend');
assert(shared.includes('rememberClipboardUrl(text)'), 'clipboard remembers URL before POST');
assert(shared.includes("chrome.storage.local.remove('falconLastClipboardUrl')"), 'clipboard POST fail allows retry');
assert(shared.includes('resetContentScriptGuard'), 'clear stale content guard before inject');
assert(shared.includes('reinitContentScriptsAfterReload'), 'rebind content scripts after extension reload');
assert(background.includes('reinitContentScriptsAfterReload'), 'reload youtube tabs on install');
assert(content.includes('isEligibleVideoElement'), 'fab eligibility guard');
assert(content.includes('media_updated'), 'fab resync on sniff');
assert(content.includes('pickLargestVideo') === false, 'removed naive largest-video fab');
assert(content.includes("'ping'"), 'content script ping');
assert(popupHtml.includes('id="notice"'), 'popup error notice');
assert(popupHtml.includes('id="dl-url"') && /id="dl-url"[^>]*disabled/.test(popupHtml), 'dl-url starts disabled until tab url ready');
assert(optionsHtml.includes('options-head'), 'options header');
assert(optionsHtml.includes('aria-describedby="token-hint"'), 'options token hint is describedby');
assert(
  optionsHtml.includes('aria-describedby="fail-closed-hint"'),
  'options fail-closed hint is describedby',
);
assert(optionsHtml.includes('aria-describedby="clipboard-monitor-hint"'),
  'options clipboard hint is describedby',
);
assert(
  optionsHtml.includes('aria-describedby="id-hint"'),
  'options extension id hint is describedby',
);
assert(
  optionsHtml.includes('aria-describedby="hint"'),
  'options pair/test hint is describedby',
);
assert(
  popupHtml.includes('aria-describedby="grab-hint"'),
  'popup grab hint is describedby',
);
assert(!popupHtml.includes('fonts.googleapis.com'), 'popup does not load Google Fonts');
assert(!optionsHtml.includes('fonts.googleapis.com'), 'options does not load Google Fonts');
assert(
  !readFileSync(path.join(__dirname, '../index.html'), 'utf8').includes('fonts.googleapis.com'),
  'desktop CSP font-src self — no Google Fonts CDN',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/settings/ProfilesTab.tsx'), 'utf8').includes(
    'aria-describedby="settings-profile-host-hint"',
  ),
  'profile host hint is describedby',
);
assert(content.includes('getClientRects()'), 'overlay tab trap uses layout boxes not offsetParent');
assert(!content.includes('node.offsetParent !== null'), 'offsetParent is null under position:fixed overlay');
const appTsx = readFileSync(path.join(__dirname, '../src/App.tsx'), 'utf8');
assert(
  appTsx.includes('if (isForegroundDialogOpen()) return false'),
  'clipboard overlay skip does not consume the URL',
);
assert(
  readFileSync(path.join(__dirname, '../src/hooks/useClipboardMonitor.ts'), 'utf8').includes(
    'clipboardUrlConsumed',
  ),
  'clipboard lastRef waits for overlay accept',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/SettingsModal.tsx'), 'utf8').includes(
    "e.key === 'ArrowUp'",
  ),
  'settings tabs use Up/Down as well as Left/Right',
);
const paletteTsx = readFileSync(path.join(__dirname, '../src/components/CommandPalette.tsx'), 'utf8');
assert(paletteTsx.includes('aria-haspopup="listbox"'), 'command palette combobox has listbox popup');
assert(paletteTsx.includes('role="status"'), 'empty palette announces no results');
assert(
  readFileSync(path.join(__dirname, '../src-tauri/src/settings.rs'), 'utf8').includes(
    'port_or_known_default()',
  ),
  'profile origin match uses default ports',
);
assert(
  readFileSync(path.join(__dirname, '../src-tauri/src/lib.rs'), 'utf8').includes(
    'fn emit_download_status',
  ),
  'pause/resume emit download-progress',
);
assert(
  readFileSync(path.join(__dirname, '../src-tauri/src/util/path.rs'), 'utf8').includes(
    'Save path is outside allowed directories',
  ),
  'outside-root save path is a hard error',
);
assert(
  readFileSync(path.join(__dirname, '../src-tauri/src/lib.rs'), 'utf8').includes(
    'fn clear_failed_retry_artifacts',
  ),
  'Failed retry clears leftover part/HLS/error dest',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/SchedulerModal.tsx'), 'utf8').includes(
    'id="sched-scope-hint"',
  ),
  'scheduler scope hint is describedby',
);
assert(
  readFileSync(path.join(__dirname, '../src/App.tsx'), 'utf8').includes('speedLimitFromSettings') &&
    readFileSync(path.join(__dirname, '../src/App.tsx'), 'utf8').includes('[settingsOpen]'),
  'toolbar speed limit refetches when Settings closes',
);
assert(
  readFileSync(path.join(__dirname, '../src/App.tsx'), 'utf8').includes('schedulerActive') &&
    readFileSync(path.join(__dirname, '../src/App.tsx'), 'utf8').includes('[schedulerOpen]'),
  'toolbar scheduler refetches when Scheduler closes',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/Toolbar.tsx'), 'utf8').includes(
    'aria-pressed={schedulerActive}',
  ),
  'toolbar scheduler exposes pressed state',
);
assert(
  readFileSync(path.join(__dirname, '../src-tauri/src/settings.rs'), 'utf8').includes(
    'fn preserve_command_owned_fields',
  ) &&
    readFileSync(path.join(__dirname, '../src-tauri/src/settings.rs'), 'utf8').includes(
      'fn apply_live_schedule',
    ) &&
    readFileSync(path.join(__dirname, '../src-tauri/src/commands.rs'), 'utf8').includes(
      'apply_live_schedule',
    ),
  'save_settings preserves allowlist and live queue schedule',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/SettingsModal.tsx'), 'utf8').includes(
    '!hydrated',
  ),
  'settings save waits for first snapshot',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/SchedulerModal.tsx'), 'utf8').includes(
    '!hydrated',
  ) &&
    readFileSync(path.join(__dirname, '../src/components/SettingsModal.tsx'), 'utf8').includes(
      '{ inert: true }',
    ) &&
    readFileSync(path.join(__dirname, '../src/components/SchedulerModal.tsx'), 'utf8').includes(
      '{ inert: true }',
    ),
  'scheduler/settings save waits for first snapshot and inert until then',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/SettingsModal.tsx'), 'utf8').includes(
    'aria-describedby="settings-profiles-hint"',
  ),
  'profiles tabpanel is described by the intro hint',
);
assert(
  /onClose\(\);[\s\S]{0,240}await action\.run\(\)/.test(paletteTsx) &&
    !/await action\.run\(\);[\s\S]{0,80}onClose\(\)/.test(paletteTsx),
  'command palette closes before running so focus restore cannot steal the next dialog',
);

assert(
  readFileSync(path.join(__dirname, '../src-tauri/src/settings.rs'), 'utf8').includes(
    'fn restore_blank_profile_cookies',
  ),
  'blank profile cookies survive rename via index fallback',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/SettingsModal.tsx'), 'utf8').includes(
    '/^[a-p]{32}$/',
  ) &&
    readFileSync(path.join(__dirname, '../src/components/OnboardingWizard.tsx'), 'utf8').includes(
      '/^[a-p]{32}$/',
    ),
  'native host install UI matches Chrome ID charset',
);
assert(
  readFileSync(path.join(__dirname, '../src-tauri/src/download/queue.rs'), 'utf8').includes(
    'fn demote_active_to_queued',
  ) &&
    readFileSync(path.join(__dirname, '../src-tauri/src/download/queue.rs'), 'utf8').includes(
      'emit_download_status',
    ),
  'scheduler/concurrency demotion emits Queued like pause/resume',
);
assert(
  /download_blob[\s\S]{0,800}sendToFalcon\(/.test(background),
  'blob upload uses sendToFalcon wake/retry',
);
assert(shared.includes("delete window.__falconDmContentScript"), 'reload re-inject clears content guard');
assert(
  shared.indexOf("sendMessage(tabId, { action: 'ping' })") <
    shared.indexOf("files: ['media-utils.js', 'content.js']"),
  'ensureContentScript pings before injecting so YouTube manifest CS is not doubled',
);
assert(
  popup.includes('failClosed = !!resp.failClosed') &&
    popup.includes('failClosedEl.checked = !failClosedEl.checked'),
  'popup fail-closed revert uses the checkbox not a stale flag',
);
assert(
  pairing.includes('function throwIfExtensionBlocked') &&
    pairing.includes("connectionState !== 'blocked'"),
  'pair 403 sets blocked and does not clobber it to offline',
);
assert(api.includes("if (response.status === 403) setState('blocked')"), 'postFalcon 403 marks blocked');
assert(
  background.includes("e.message || msg('interceptBlocked'"),
  'fail-closed hijack notify keeps the real error',
);
assert(
  /grab_page_links[\s\S]{0,350}if \(!ok\)/.test(background),
  'context-menu grabber reports inject failure',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/LogPanel.tsx'), 'utf8').includes(
    'errorToastSent',
  ),
  'log auto-refresh does not toast every tick',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/DownloadItem.tsx'), 'utf8').includes(
    'already_queued',
  ),
  'redownload of an active URL is announced',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/InspectorPanel.tsx'), 'utf8').includes(
    'isLive ? `${formatBytes(download.speed)}/s`',
  ),
  'inspector shows speed while merging',
);
assert(
  /fallbackBrowserDownload[\s\S]{0,250}msg\('appClosedFallback'/.test(background) &&
    !background.includes("e.message || msg('appClosedFallback'"),
  'fail-open success notify is the fallback copy not the Falcon error',
);
assert(
  readFileSync(path.join(__dirname, '../src/store/downloads.ts'), 'utf8').includes(
    "existing.status === 'Paused'",
  ),
  'poll merge keeps Paused against a stale Downloading snapshot',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/SchedulerModal.tsx'), 'utf8').includes(
    "t('scheduler.retry')",
  ),
  'scheduler load failure offers retry',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/SettingsModal.tsx'), 'utf8').includes(
    "t('settings.retry')",
  ),
  'settings load failure offers retry',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/settings/ProfilesTab.tsx'), 'utf8').includes(
    'allocProfileUiKey',
  ),
  'profile cards use stable keys across delete',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/CommandPalette.tsx'), 'utf8').includes(
    'if (filtered.length === 0) return;',
  ),
  'palette Enter on empty results is a no-op',
);
assert(
  popup.includes("if (state === 'offline' || state === 'pending')") &&
    /return;\n\s*}\n\s*openMediaPicker/.test(popup),
  'popup grab does not open the overlay while disconnected',
);
assert(
  /open_grabber[\s\S]{0,250}chrome\.runtime\.lastError/.test(background),
  'context-menu grabber reports sendMessage failure',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/SettingsModal.tsx'), 'utf8').includes(
    'pair_load_failed',
  ),
  'pending pair fetch failure is announced',
);
assert(
  readFileSync(path.join(__dirname, '../src/store/downloads.ts'), 'utf8').includes(
    "existing.status === 'Merging' && fresh.status === 'Downloading'",
  ),
  'poll merge keeps Merging against a stale Downloading snapshot',
);
assert(
  readFileSync(path.join(__dirname, '../src/components/NewDownloadModal.tsx'), 'utf8').includes(
    'Promise.allSettled',
  ),
  'batch add continues after a single URL fails',
);
const flow = spawnSync(process.execPath, [path.join(__dirname, 'flow-test.mjs')], { encoding: 'utf8' });
assert(flow.status === 0, (flow.stderr || flow.stdout || 'flow-test failed').trim());

console.log('extension smoke ok');
