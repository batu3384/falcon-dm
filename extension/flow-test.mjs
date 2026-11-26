/**
 * Runnable click-path checks (mock chrome — no browser).
 * Usage: node extension/flow-test.mjs
 */
import { readFileSync, existsSync } from 'fs';
import { homedir } from 'os';
import vm from 'vm';
import path from 'path';
import { fileURLToPath } from 'url';
import { JSDOM } from 'jsdom';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

function assert(cond, msg) {
  if (!cond) throw new Error(msg);
}

const queries = [];
const chrome = {
  storage: {
    session: {
      get: async (defaults) => defaults,
      set: async () => {},
    },
    local: {
      get: async (defaults) => defaults,
      set: async () => {},
    },
  },
  action: {
    setBadgeBackgroundColor() {},
    setBadgeText() {},
  },
  windows: {
    getLastFocused: async ({ windowTypes }) => {
      queries.push({ fn: 'getLastFocused', windowTypes });
      if (windowTypes && windowTypes.includes('normal')) {
        return { id: 1, type: 'normal', focused: true };
      }
      return { id: 99, type: 'popup', focused: true };
    },
  },
  tabs: {
    query: async (q) => {
      queries.push({ fn: 'tabs.query', q });
      if (q.windowId === 99) return [];
      if (q.windowId === 1 || q.windowType === 'normal') {
        return [{ id: 42, url: 'https://www.youtube.com/watch?v=dQw4w9WgXcQ', active: true }];
      }
      return [];
    },
  },
  runtime: { id: 'abcdefghijklmnopqrstuvwxyzabcdef', lastError: null },
  i18n: { getMessage: () => '' },
};

const sandbox = {
  chrome,
  console,
  URL,
  setTimeout,
  clearTimeout,
  fetch,
  AbortController,
  Map,
  Set,
  Promise,
};
sandbox.globalThis = sandbox;
vm.createContext(sandbox);
vm.runInContext(readFileSync(path.join(__dirname, 'media-utils.js'), 'utf8'), sandbox);
vm.runInContext(readFileSync(path.join(__dirname, 'shared.js'), 'utf8'), sandbox);

const tab = await vm.runInContext('getActiveBrowserTab()', sandbox);
assert(tab && tab.id === 42, 'getActiveBrowserTab returns normal window tab, not popup');
assert(
  queries.some((q) => q.fn === 'getLastFocused' && q.windowTypes && q.windowTypes.includes('normal')),
  'tab lookup asks for normal windows',
);

const FM = sandbox.FalconMedia;
const hijack = FM.hijackPayloadForFalcon(
  'https://rr1---sn.googlevideo.com/videoplayback?itag=18&id=abc',
  'https://www.youtube.com/watch?v=dQw4w9WgXcQ',
);
assert(hijack.url.includes('watch?v='), 'click-path hijack uses watch URL');

const watchJob = FM.overlayEnqueuePayload('https://www.youtube.com/watch?v=dQw4w9WgXcQ', {
  url: 'https://rr1---sn.googlevideo.com/videoplayback?itag=18',
  height: 720,
});
assert(watchJob.url.includes('watch?v='), 'watch overlay enqueues page URL');
assert(watchJob.format && watchJob.format.includes('height<=720'), 'watch overlay sends yt-dlp format');

const homeJob = FM.overlayEnqueuePayload('https://www.youtube.com/', {
  url: 'https://cdn.example.com/preview.mp4',
  height: 720,
});
assert(homeJob.url === 'https://cdn.example.com/preview.mp4', 'home overlay keeps media URL');
assert(homeJob.format === null, 'home overlay does not force yt-dlp format');

const shortsJob = FM.overlayEnqueuePayload('https://www.youtube.com/shorts/abc123', {
  url: 'https://example.com/x.mp4',
  height: 1080,
});
assert(shortsJob.url.includes('/shorts/abc123'), 'shorts overlay uses shorts URL');
assert(shortsJob.format.includes('height<=1080'), 'shorts height bound');

const spoofShorts = FM.overlayEnqueuePayload('https://evil.com/shorts/abc', {
  url: 'https://cdn.example.com/a.mp4',
  height: 1080,
});
assert(spoofShorts.url === 'https://cdn.example.com/a.mp4', 'non-youtube /shorts/ keeps media URL');
assert(spoofShorts.format === null, 'non-youtube /shorts/ does not force yt-dlp');

const blobJob = FM.overlayEnqueuePayload('https://www.youtube.com/watch?v=abc', {
  url: 'blob:https://www.youtube.com/uuid',
  isBlob: true,
});
assert(blobJob.url.startsWith('blob:'), 'blob on watch stays blob');
assert(blobJob.format === null, 'blob skip yt-dlp format');
assert(!FM.isJunkUrl('https://www.youtube.com/watch?v=abc'), 'watch URL is not junk');

const cards = FM.youtubeFallbackSources('https://www.youtube.com/watch?v=abc');
const best = FM.pickBest(cards);
assert(best && best.height === 1080, 'pickBest selects highest muxed fallback');
assert(
  cards.filter((c) => c.url === best.url).length === cards.length,
  'watch fallback cards share page URL — UI must not key selection on url',
);

function tick(ms = 30) {
  return new Promise((r) => setTimeout(r, ms));
}

function mockRect(width, height, top = 40, left = 40) {
  return {
    width,
    height,
    top,
    left,
    right: left + width,
    bottom: top + height,
    x: left,
    y: top,
    toJSON() {},
  };
}

function later(win, fn) {
  win.setTimeout(fn, 0);
}

function installChrome(win, { onSend } = {}) {
  const sent = [];
  win.chrome = {
    runtime: {
      id: 'abcdefghijklmnopqrstuvwxyzabcdef',
      lastError: null,
      sendMessage(payload, cb) {
        sent.push(payload);
        const result = onSend ? onSend(payload) : { ok: true, success: true };
        if (typeof cb === 'function') later(win, () => cb(result));
      },
      onMessage: { addListener() {} },
    },
    i18n: { getMessage: () => '', getUILanguage: () => 'en' },
    tabs: {
      query: () =>
        Promise.resolve([
          { id: 42, url: 'https://www.youtube.com/watch?v=dQw4w9WgXcQ', active: true },
        ]),
    },
    storage: {
      session: { get: async (d) => d, set: async () => {} },
      local: { get: async (d) => d, set: async () => {} },
    },
  };
  return sent;
}

function withDomGlobals(win, fn) {
  const prev = {
    window: globalThis.window,
    document: globalThis.document,
    chrome: globalThis.chrome,
  };
  globalThis.window = win;
  globalThis.document = win.document;
  globalThis.chrome = win.chrome;
  return Promise.resolve()
    .then(fn)
    .finally(() => {
      globalThis.window = prev.window;
      globalThis.document = prev.document;
      globalThis.chrome = prev.chrome;
    });
}

function patchVisual(win) {
  win.innerWidth = 1280;
  win.innerHeight = 800;
  win.ResizeObserver = class {
    observe() {}
    disconnect() {}
  };
  win.requestAnimationFrame = (cb) => setTimeout(() => cb(Date.now()), 0);
  win.cancelAnimationFrame = (id) => clearTimeout(id);
  win.HTMLElement.prototype.getBoundingClientRect = function () {
    if (this.tagName === 'VIDEO') return mockRect(640, 360, 80, 80);
    return mockRect(196, 44, 88, 520);
  };
}

function loadScripts(win, files) {
  for (const file of files) {
    const script = win.document.createElement('script');
    script.textContent = readFileSync(path.join(__dirname, file), 'utf8');
    win.document.documentElement.appendChild(script);
  }
}

{
  const sentBox = { sent: [] };
  const dom = new JSDOM(
    `<!doctype html><html><body>
      <div id="movie_player">
        <video class="html5-main-video" src="https://example.com/x.mp4"></video>
      </div>
    </body></html>`,
    {
      url: 'https://www.youtube.com/watch?v=dQw4w9WgXcQ',
      pretendToBeVisual: true,
      runScripts: 'dangerously',
      beforeParse(win) {
        patchVisual(win);
        sentBox.sent = installChrome(win, {
          onSend(payload) {
            if (payload.action === 'download_video') return { success: true };
            if (payload.action === 'get_real_media_url') {
              return { urls: [], metaMap: {}, title: 'Rick', cookies: '', userAgent: 'ua' };
            }
            return { ok: true };
          },
        });
      },
    },
  );
  const win = dom.window;
  await withDomGlobals(win, async () => {
    loadScripts(win, ['media-utils.js', 'content.js']);

    const fabHost = win.document.querySelector('[data-falcon-fab]');
    assert(fabHost, 'FAB mounts on YouTube watch player');
    const fabBtn = fabHost.shadowRoot && fabHost.shadowRoot.querySelector('.fm-fab');
    assert(fabBtn, 'FAB button is in shadow root');
    fabBtn.click();
    await tick();

    const overlayHost = [...win.document.querySelectorAll('div')].find(
      (node) => node.shadowRoot && node.shadowRoot.querySelector('[role="dialog"]'),
    );
    assert(overlayHost, 'FAB click opens overlay dialog');
    const panel = overlayHost.shadowRoot.querySelector('[role="dialog"]');
    const activeCards = [...panel.querySelectorAll('.fm-card.active')];
    assert(activeCards.length === 1, `exactly one quality card active, got ${activeCards.length}`);
    assert(activeCards[0].textContent.includes('1080'), 'default selected card is 1080p');

    const radios = [...panel.querySelectorAll('input[name="falcon-quality"]')];
    assert(radios.filter((r) => r.checked).length === 1, 'exactly one quality radio checked');

    const lowCard = [...panel.querySelectorAll('.fm-card')].find((c) => c.textContent.includes('360'));
    assert(lowCard, '360p card exists');
    lowCard.click();
    await tick();
    const activeAfter = [...panel.querySelectorAll('.fm-card.active')];
    assert(activeAfter.length === 1, 'clicking 360 leaves one active card');
    assert(activeAfter[0].textContent.includes('360'), '360p stays selected after click');

    const goBtn = [...panel.querySelectorAll('button')].find((b) => /start download/i.test(b.textContent));
    assert(goBtn, 'Start Download button exists');
    goBtn.click();
    await tick(80);

    const job = sentBox.sent.find((p) => p.action === 'download_video');
    assert(job, 'Start Download posts download_video');
    assert(String(job.url).includes('watch?v='), 'enqueue uses watch URL not CDN');
    assert(
      String(job.format || '').includes('height<=360'),
      `format follows selected 360, got ${job.format}`,
    );
  });
}

{
  const dom = new JSDOM(
    `<!doctype html><html><body>
      <div id="movie_player">
        <video class="html5-main-video" src="https://example.com/x.mp4"></video>
      </div>
    </body></html>`,
    {
      url: 'https://www.youtube.com/',
      pretendToBeVisual: true,
      runScripts: 'dangerously',
      beforeParse(win) {
        patchVisual(win);
        installChrome(win);
      },
    },
  );
  const win = dom.window;
  await withDomGlobals(win, () => {
    loadScripts(win, ['media-utils.js', 'content.js']);
    assert(!win.document.querySelector('[data-falcon-fab]'), 'FAB hidden on YouTube home');
  });
}

async function runPopupGrab(state) {
  const html = readFileSync(path.join(__dirname, 'popup.html'), 'utf8').replace(
    /<script src="popup.js"><\/script>/,
    '',
  );
  const grabs = [];
  const sentBox = { sent: [] };
  const dom = new JSDOM(html, {
    url: 'https://www.youtube.com/watch?v=abc',
    pretendToBeVisual: true,
    runScripts: 'dangerously',
    beforeParse(win) {
      sentBox.sent = installChrome(win, {
        onSend(payload) {
          if (payload.action === 'grab_tab_media') {
            grabs.push(payload);
            return { ok: true };
          }
          if (payload.action === 'check_status') {
            return { state, paused: false, failClosed: false, pairCode: '482 913' };
          }
          if (payload.action === 'get_downloads') {
            return {
              downloads: [
                {
                  id: 1,
                  filename: 'clip.mp4',
                  status: 'Downloading',
                  downloaded_size: 50,
                  total_size: 100,
                  speed: 10,
                  error_message: null,
                },
              ],
              active: 1,
              queued: 0,
              speed: 10,
            };
          }
          if (payload.action === 'auto_pair') return { ok: false, error: 'offline' };
          return { ok: true };
        },
      });
    },
  });
  const win = dom.window;
  await withDomGlobals(win, async () => {
    loadScripts(win, ['popup.js']);
    await tick(60);
    const grab = win.document.getElementById('grab');
    assert(grab, 'popup grab CTA exists');
    assert(!grab.disabled, `${state} popup CTA is clickable`);
    assert(grab.getAttribute('aria-disabled') !== 'true', 'offline CTA does not lie aria-disabled');
    const doc = win.document;
    if (state === 'pending') {
      assert(!doc.getElementById('card-pending').hidden, 'pending popup shows the pairing card');
      assert(doc.getElementById('pair-code').textContent === '482 913', 'pending popup shows the code');
    }
    if (state === 'connected') {
      assert(doc.getElementById('card-offline').hidden, 'connected popup hides the offline card');
      assert(
        doc.querySelector('#downloads .dl-name')?.textContent === 'clip.mp4',
        'connected popup lists live downloads',
      );
    }
    if (state === 'offline') {
      assert(!doc.getElementById('card-offline').hidden, 'offline popup offers to open the app');
    }
    grab.click();
    await tick(60);
  });
  // popup.js polls while open; closing the window stops its timers.
  win.close();
  return { grabs, sent: sentBox.sent };
}

{
  const pending = await runPopupGrab('pending');
  assert(pending.grabs.length === 0, 'pending popup does not open the overlay');
}

{
  const offline = await runPopupGrab('offline');
  assert(offline.grabs.length === 0, 'popup grab does not send grab_tab_media while offline');
  assert(
    offline.sent.some((p) => p.action === 'auto_pair'),
    'offline grab tries auto_pair instead of overlay',
  );
}

{
  const connected = await runPopupGrab('connected');
  assert(connected.grabs.length === 1, 'popup grab sends grab_tab_media when connected');
  assert(connected.grabs[0].tabId === 42, `popup passes tabId 42, got ${connected.grabs[0].tabId}`);
}

// Code pairing: 202 (waiting) → 200 (approved) stores the token; 403 forgets the code.
async function runPairing(responses) {
  const store = {};
  const notes = [];
  const calls = [];
  const sandbox2 = {
    console,
    URL,
    setTimeout: (fn) => setTimeout(fn, 0),
    clearTimeout,
    AbortController,
    Map,
    Set,
    Promise,
    crypto: globalThis.crypto,
    chrome: {
      runtime: { id: 'abcdefghijklmnopabcdefghijklmnop' },
      i18n: { getMessage: () => '' },
      action: { setBadgeBackgroundColor() {}, setBadgeText() {} },
      notifications: { create: (_id, opts) => notes.push(opts) },
      storage: {
        local: {
          get: async (key) => (typeof key === 'string' ? { [key]: store[key] } : { ...key, ...store }),
          set: async (values) => Object.assign(store, values),
          remove: async (key) => delete store[key],
        },
        session: { get: async (d) => d, set: async () => {}, remove: async () => {} },
      },
      tabs: { create() {} },
    },
    fetch: async (url, opts = {}) => {
      if (url.endsWith('/api/health')) return { ok: true, status: 200, json: async () => ({ ok: true }) };
      if (url.endsWith('/api/pair')) {
        calls.push(JSON.parse(opts.body));
        const next = responses.shift() || { status: 202, body: { pending: true } };
        return { ok: next.status === 200, status: next.status, json: async () => next.body };
      }
      return { ok: false, status: 404, json: async () => ({}) };
    },
  };
  sandbox2.globalThis = sandbox2;
  vm.createContext(sandbox2);
  for (const file of ['media-utils.js', 'shared.js', 'pairing.js']) {
    vm.runInContext(readFileSync(path.join(__dirname, file), 'utf8'), sandbox2);
  }
  let result;
  let error;
  try {
    result = await vm.runInContext('ensurePaired(true)', sandbox2);
  } catch (e) {
    error = e;
  }
  return { store, calls, notes, result, error, state: vm.runInContext('connectionState', sandbox2) };
}

{
  const approved = await runPairing([
    { status: 202, body: { pending: true } },
    { status: 200, body: { ok: true, token: 'tok-1' } },
  ]);
  assert(approved.result === 'tok-1', 'approved pairing returns the token');
  assert(approved.store.apiToken === 'tok-1', 'token is stored');
  assert(!approved.store.falconPairCode, 'code is forgotten after approval');
  assert(/^\d{6}$/.test(approved.calls[0].code), 'pair request sends a six-digit code');
  assert(approved.calls[0].code === approved.calls[1].code, 'polling keeps the same code');
  assert(!('challenge' in approved.calls[0]) && !('proof' in approved.calls[0]), 'no native proof');
  assert(approved.notes.length === 1, 'user is told to approve the code once');
  assert(approved.state === 'connected', 'state is connected after approval');
}

{
  const declined = await runPairing([{ status: 403, body: null }]);
  assert(declined.error, 'declined pairing throws');
  assert(declined.state === 'blocked', 'declined pairing is shown as blocked');
  assert(!declined.store.falconPairCode, 'declined code is discarded for a fresh one');
}

let live = false;
try {
  const health = await fetch('http://127.0.0.1:14201/api/health', {
    signal: AbortSignal.timeout(2000),
  });
  if (health.ok) {
    const body = await health.json();
    assert(body.ok === true && body.service === 'falcon-dm' && body.protocol === 2, 'health payload');
    live = true;
  }
} catch (_) {}
if (!live) {
  console.log('live Falcon API skipped (not listening)');
} else {
  const noToken = await fetch('http://127.0.0.1:14201/api/intercept', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ url: 'https://example.com/file.bin' }),
    signal: AbortSignal.timeout(3000),
  });
  assert(noToken.status === 401, `intercept without token expected 401, got ${noToken.status}`);
  const settingsPath = path.join(
    homedir(),
    'Library/Application Support/com.falcondm.app/settings.json',
  );
  if (existsSync(settingsPath)) {
    const settings = JSON.parse(readFileSync(settingsPath, 'utf8'));
    const token = settings.api_token;
    const ext = (settings.allowed_extension_ids || [])[0];
    if (token && ext) {
      const intercept = await fetch('http://127.0.0.1:14201/api/intercept', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'X-Falcon-Token': token,
          Origin: `chrome-extension://${ext}`,
        },
        body: JSON.stringify({ url: 'not-a-url' }),
        signal: AbortSignal.timeout(3000),
      });
      assert(
        intercept.status !== 401 && intercept.status !== 403,
        `intercept auth failed HTTP ${intercept.status}`,
      );
      assert(
        intercept.status === 400,
        `intercept rejects junk url with 400, got ${intercept.status}`,
      );
    }
  }
}

console.log('extension flow ok');
