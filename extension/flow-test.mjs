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
      remove: async () => {},
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

const normalizedExts = vm.runInContext(
  "normalizeCaptureExtensions(' ZIP, pdf\\n7z zip .EXE bad_ext ')",
  sandbox,
);
assert(normalizedExts.text === 'zip pdf 7z exe', 'capture extensions normalize to lowercase alnum');
assert(normalizedExts.dropped === 1, 'invalid capture extension is dropped');
assert(
  vm.runInContext("normalizeCaptureExtensions('').extensions.length", sandbox) === 0,
  'empty capture extension input is safe',
);
const manyExts = Array.from({ length: 300 }, (_, i) => `e${i}`).join(' ');
const cappedExts = vm.runInContext(
  `normalizeCaptureExtensions(${JSON.stringify(manyExts)})`,
  sandbox,
);
assert(cappedExts.extensions.length === 256, 'capture extension list is count-capped');
assert(
  vm.runInContext(
    `normalizeCaptureExtensions(${JSON.stringify('x'.repeat(20000))}).dropped`,
    sandbox,
  ) === 1,
  'extreme capture extension input is bounded',
);
assert(
  vm.runInContext('defaultCaptureExtensionsText()', sandbox).includes('zip'),
  'capture default comes from the existing extension list',
);
const normalizedHosts = vm.runInContext(
  "normalizeCaptureBlockedHosts('blocked.example\\n*.Example.COM\\n.alt.org\\nnot_a_host\\nblocked.example')",
  sandbox,
);
assert(
  normalizedHosts.text === 'blocked.example\n*.example.com\n*.alt.org',
  'blocked hosts normalize exact and wildcard rules',
);
assert(normalizedHosts.dropped === 1, 'invalid blocked host is dropped');
assert(
  vm.runInContext("captureHostMatches('cdn.example.com', '*.example.com')", sandbox),
  'wildcard blocked host matches a subdomain',
);
assert(
  vm.runInContext("captureHostMatches('example.com', '*.example.com')", sandbox),
  'wildcard blocked host also covers the base host',
);
assert(
  vm.runInContext("captureHostMatches('cdn.alt.org', '.alt.org')", sandbox),
  'leading-dot blocked host matches a subdomain',
);
assert(
  !vm.runInContext("captureHostMatches('cdn.blocked.example', 'blocked.example')", sandbox),
  'exact blocked host does not match a subdomain',
);
assert(
  !vm.runInContext("captureHostMatches('evilexample.com', '*.example.com')", sandbox),
  'wildcard blocked host keeps a dot boundary',
);
assert(
  vm.runInContext("captureFileExtension('https://cdn.example.com/file%2Ezip?x=1')", sandbox) === 'zip',
  'capture extension decoding handles escaped URL names',
);
assert(
  vm.runInContext(
    "shouldCaptureBrowserDownload({ filename: 'a.zip', url: 'https://cdn.example.com/a.zip' }, { extensions: ['zip'], blockedHosts: [] })",
    sandbox,
  ),
  'listed extension allows automatic capture',
);
assert(
  !vm.runInContext(
    "shouldCaptureBrowserDownload({ filename: 'a.zip', url: 'https://cdn.example.com/a.zip', referrer: 'https://cdn.example.com/' }, { extensions: ['zip'], blockedHosts: ['*.example.com'] })",
    sandbox,
  ),
  'blocked wildcard host denies automatic capture',
);
assert(
  !vm.runInContext(
    "shouldCaptureBrowserDownload({ filename: 'a.zip', url: 'https://blocked.example/a.zip' }, { extensions: ['zip'], blockedHosts: ['blocked.example'] })",
    sandbox,
  ),
  'blocked download host denies automatic capture without a referrer',
);
assert(
  !vm.runInContext(
    "shouldCaptureBrowserDownload({ filename: 'a.zip', url: 'https://cdn.example.com/a.zip' }, { extensions: [], blockedHosts: [] })",
    sandbox,
  ),
  'empty capture extension list captures nothing',
);

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

{
  const html = readFileSync(path.join(__dirname, 'options.html'), 'utf8').replace(
    /<script src="[^"]+"><\/script>/g,
    '',
  );
  const localStore = { falconCaptureBlockedHosts: 'blocked.example' };
  const storageListeners = [];
  const storageGet = (store, query) => {
    if (typeof query === 'string') {
      return Object.hasOwn(store, query) ? { [query]: store[query] } : {};
    }
    if (query && typeof query === 'object' && !Array.isArray(query)) {
      const out = {};
      for (const [key, fallback] of Object.entries(query)) {
        out[key] = Object.hasOwn(store, key) ? store[key] : fallback;
      }
      return out;
    }
    return { ...store };
  };
  const dom = new JSDOM(html, {
    url: 'chrome-extension://abcdefghijklmnopqrstuvwxyzabcdef/options.html',
    pretendToBeVisual: true,
    runScripts: 'dangerously',
    beforeParse(win) {
      win.chrome = {
        runtime: {
          id: 'abcdefghijklmnopqrstuvwxyzabcdef',
          lastError: null,
          sendMessage(payload, cb) {
            const result =
              payload.action === 'check_status'
                ? { state: 'connected', paused: false, failClosed: true, notifySuccess: false }
                : { ok: true };
            if (cb) win.setTimeout(() => cb(result), 0);
          },
          onMessage: { addListener() {} },
        },
        i18n: { getMessage: () => '', getUILanguage: () => 'en' },
        storage: {
          local: {
            get: async (query) => storageGet(localStore, query),
            set: async (values) => {
              const changes = {};
              for (const [key, value] of Object.entries(values)) {
                changes[key] = { oldValue: localStore[key], newValue: value };
                localStore[key] = value;
              }
              for (const listener of storageListeners) listener(changes, 'local');
            },
            remove: async (key) => delete localStore[key],
          },
          session: {
            get: async (query) => query,
            set: async () => {},
            remove: async () => {},
          },
          onChanged: {
            addListener(fn) {
              storageListeners.push(fn);
            },
          },
        },
      };
    },
  });
  const win = dom.window;
  await withDomGlobals(win, async () => {
    loadScripts(win, ['media-utils.js', 'shared.js', 'options.js']);
    await tick(50);
    const types = win.document.getElementById('capture-types');
    const blocked = win.document.getElementById('blocked-sites');
    assert(types && blocked, 'options capture-rule textareas exist');
    assert(types.value.includes('zip'), 'options shows the default capture extensions');
    assert(blocked.value === 'blocked.example', 'options loads the blocked-site list');

    types.value = 'ZIP, pdf\n7z bad_ext';
    types.dispatchEvent(new win.Event('input', { bubbles: true }));
    await tick(20);
    assert(
      localStore.falconCaptureExtensions === 'zip pdf 7z',
      `options persists normalized extensions live, got ${localStore.falconCaptureExtensions}`,
    );
    types.dispatchEvent(new win.Event('change', { bubbles: true }));
    await tick(20);
    assert(types.value === 'zip pdf 7z', 'options rewrites extensions to canonical form on change');

    blocked.value = 'example.com\n*.Example.COM\nnot_a_host';
    blocked.dispatchEvent(new win.Event('input', { bubbles: true }));
    await tick(20);
    assert(
      localStore.falconCaptureBlockedHosts === 'example.com\n*.example.com',
      `options persists normalized blocked hosts live, got ${localStore.falconCaptureBlockedHosts}`,
    );
  });
  win.close();
}

function makeBackgroundHarness(initialLocal = {}) {
  const localStore = { apiToken: 'flow-token', ...initialLocal };
  const sessionStore = {};
  const storageListeners = [];
  const listeners = {};
  const fetchCalls = [];
  const cancels = [];
  const erased = [];
  const rows = new Map();

  const storageGet = (store, query) => {
    if (typeof query === 'string') {
      return Object.hasOwn(store, query) ? { [query]: store[query] } : {};
    }
    if (Array.isArray(query)) {
      const out = {};
      for (const key of query) if (Object.hasOwn(store, key)) out[key] = store[key];
      return out;
    }
    if (query && typeof query === 'object') {
      const out = {};
      for (const [key, fallback] of Object.entries(query)) {
        out[key] = Object.hasOwn(store, key) ? store[key] : fallback;
      }
      return out;
    }
    return { ...store };
  };
  const storageSet = (store, values, area) => {
    const changes = {};
    for (const [key, value] of Object.entries(values)) {
      changes[key] = { oldValue: store[key], newValue: value };
      store[key] = value;
    }
    for (const listener of storageListeners) listener(changes, area);
  };
  const storageRemove = (store, keys, area) => {
    const changes = {};
    for (const key of Array.isArray(keys) ? keys : [keys]) {
      changes[key] = { oldValue: store[key], newValue: undefined };
      delete store[key];
    }
    for (const listener of storageListeners) listener(changes, area);
  };
  const rowFor = (id) => {
    if (!rows.has(id)) rows.set(id, { id, state: 'in_progress', filename: '' });
    return rows.get(id);
  };

  const chromeMock = {
    storage: {
      local: {
        get: async (query) => storageGet(localStore, query),
        set: async (values) => storageSet(localStore, values, 'local'),
        remove: async (keys) => storageRemove(localStore, keys, 'local'),
      },
      session: {
        get: async (query) => storageGet(sessionStore, query),
        set: async (values) => storageSet(sessionStore, values, 'session'),
        remove: async (keys) => storageRemove(sessionStore, keys, 'session'),
      },
      onChanged: {
        addListener(fn) {
          storageListeners.push(fn);
        },
      },
    },
    action: {
      setBadgeBackgroundColor() {},
      setBadgeText() {},
    },
    downloads: {
      onDeterminingFilename: {
        addListener(fn) {
          listeners.determiningFilename = fn;
        },
      },
      onCreated: {
        addListener(fn) {
          listeners.created = fn;
        },
      },
      onChanged: {
        addListener(fn) {
          listeners.downloadChanged = fn;
        },
      },
      search: async ({ id }) => (rows.has(id) ? [rows.get(id)] : []),
      cancel(id, cb) {
        cancels.push(id);
        rowFor(id).state = 'interrupted';
        if (cb) cb();
      },
      erase(query, cb) {
        erased.push(query.id);
        if (cb) cb();
      },
      removeFile(id, cb) {
        rowFor(id).state = 'interrupted';
        if (cb) cb();
      },
      download(_options, cb) {
        if (cb) cb(900);
      },
    },
    tabs: {
      onRemoved: { addListener() {} },
      onUpdated: { addListener() {} },
      query: async () => [],
      create() {},
      remove() {},
      sendMessage(_tabId, _message, cb) {
        if (cb) cb({ ok: true });
        return Promise.resolve({ ok: true });
      },
    },
    windows: {
      getLastFocused: async () => ({ id: 1, type: 'normal', focused: true }),
    },
    webRequest: {
      onHeadersReceived: { addListener() {} },
    },
    contextMenus: {
      removeAll(cb) {
        if (cb) cb();
      },
      create() {},
      onClicked: {
        addListener(fn) {
          listeners.contextMenuClicked = fn;
        },
      },
    },
    runtime: {
      id: 'abcdefghijklmnopqrstuvwxyzabcdef',
      lastError: null,
      onInstalled: { addListener() {} },
      onStartup: { addListener() {} },
      onMessage: {
        addListener(fn) {
          listeners.message = fn;
        },
      },
    },
    cookies: {
      getAll: async () => [],
    },
    scripting: {
      executeScript: async () => [{ result: [] }],
    },
    notifications: {
      create() {},
    },
    i18n: { getMessage: () => '' },
  };

  const jsonResponse = (status, body) => ({
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  });
  const fetchMock = async (url, options = {}) => {
    let body = null;
    try {
      body = options.body ? JSON.parse(options.body) : null;
    } catch (_) {}
    fetchCalls.push({ url: String(url), options, body });
    const target = String(url);
    if (target.endsWith('/api/health')) return jsonResponse(200, { ok: true });
    if (target.endsWith('/api/ping')) return jsonResponse(200, { ok: true });
    if (target.endsWith('/api/add')) return jsonResponse(200, { ok: true, id: 77 });
    return jsonResponse(404, {});
  };

  const sandbox2 = {
    chrome: chromeMock,
    console,
    URL,
    setTimeout,
    clearTimeout,
    fetch: fetchMock,
    AbortController,
    Map,
    Set,
    Promise,
    navigator: { userAgent: 'flow-test' },
    crypto: globalThis.crypto,
  };
  sandbox2.globalThis = sandbox2;
  sandbox2.self = sandbox2;
  sandbox2.importScripts = (...files) => {
    for (const file of files) {
      vm.runInContext(readFileSync(path.join(__dirname, file), 'utf8'), sandbox2, {
        filename: file,
      });
    }
  };
  vm.createContext(sandbox2);
  vm.runInContext(readFileSync(path.join(__dirname, 'background.js'), 'utf8'), sandbox2, {
    filename: 'background.js',
  });
  return { sandbox: sandbox2, chrome: chromeMock, listeners, fetchCalls, cancels, erased };
}

async function backgroundReady(harness) {
  for (let i = 0; i < 30; i++) {
    if (vm.runInContext('prefsHydrated', harness.sandbox)) return;
    await tick(5);
  }
  throw new Error('background preferences did not hydrate');
}

async function fireDeterminingFilename(harness, item) {
  let suggested = false;
  harness.listeners.determiningFilename(item, () => {
    suggested = true;
  });
  await tick(60);
  return suggested;
}

{
  const harness = makeBackgroundHarness({
    falconCaptureExtensions: 'zip',
    falconCaptureBlockedHosts: 'blocked.example\n*.wild.example',
  });
  await backgroundReady(harness);

  assert(
    await fireDeterminingFilename(harness, {
      id: 1,
      url: 'https://files.other.example/file.zip',
      finalUrl: 'https://files.other.example/file.zip',
      referrer: 'https://blocked.example/page',
      filename: 'file.zip',
    }),
    'exact blocked host still calls suggest',
  );
  assert(
    await fireDeterminingFilename(harness, {
      id: 2,
      url: 'https://files.other.example/file.zip',
      finalUrl: 'https://files.other.example/file.zip',
      referrer: 'https://cdn.wild.example/page',
      filename: 'file.zip',
    }),
    'wildcard blocked host still calls suggest',
  );
  assert(harness.cancels.length === 0, 'blocked hosts never hijack the browser download');
  assert(
    !harness.fetchCalls.some((call) => call.url.endsWith('/api/add')),
    'blocked hosts never enqueue in Falcon',
  );

  assert(
    await fireDeterminingFilename(harness, {
      id: 3,
      url: 'https://files.other.example/file.zip',
      finalUrl: 'https://files.other.example/file.zip',
      referrer: 'https://else.example/page',
      filename: 'file.zip',
    }),
    'unblocked listed capture calls suggest',
  );
  assert(harness.cancels.includes(3), 'unblocked listed file is hijacked');
  assert(
    harness.fetchCalls.some(
      (call) => call.url.endsWith('/api/add') && call.body?.url === 'https://files.other.example/file.zip',
    ),
    'unblocked listed file enqueues in Falcon',
  );

  harness.listeners.created({
    id: 10,
    url: 'https://files.other.example/backup.zip',
    finalUrl: 'https://files.other.example/backup.zip',
    referrer: 'https://cdn.wild.example/page',
    filename: '',
  });
  await tick(600);
  assert(!harness.cancels.includes(10), 'created-download backup path also honors the blocklist');

  await harness.chrome.storage.local.set({
    falconCaptureBlockedHosts: 'else.example\n*.wild.example',
  });
  await fireDeterminingFilename(harness, {
    id: 11,
    url: 'https://files.other.example/live.zip',
    finalUrl: 'https://files.other.example/live.zip',
    referrer: 'https://else.example/page',
    filename: 'live.zip',
  });
  assert(!harness.cancels.includes(11), 'edited blocked-host list updates live capture decisions');

  await harness.listeners.contextMenuClicked({
    menuItemId: 'download_with_falcon',
    linkUrl: 'https://cdn.wild.example/manual.xyz',
    pageUrl: 'https://cdn.wild.example/page',
  });
  assert(
    harness.fetchCalls.some(
      (call) => call.url.endsWith('/api/add') && call.body?.url === 'https://cdn.wild.example/manual.xyz',
    ),
    'manual context-menu send bypasses the never-capture blocklist',
  );
}

{
  const harness = makeBackgroundHarness();
  await backgroundReady(harness);
  const item = (id, name) => ({
    id,
    url: `https://files.example.com/download?id=${id}`,
    finalUrl: `https://files.example.com/download?id=${id}`,
    referrer: 'https://files.example.com/page',
    filename: name,
  });

  await fireDeterminingFilename(harness, item(1, 'first.zip'));
  assert(harness.cancels.includes(1), 'default capture extension list allows zip');

  await harness.chrome.storage.local.set({ falconCaptureExtensions: 'mp4' });
  assert(
    vm.runInContext("captureExtensions.join(',')", harness.sandbox) === 'mp4',
    'storage edit updates the live capture extension cache',
  );
  await fireDeterminingFilename(harness, item(2, 'second.zip'));
  assert(!harness.cancels.includes(2), 'edited capture extension list stops zip hijacks');
  await fireDeterminingFilename(harness, item(3, 'movie.mp4'));
  assert(harness.cancels.includes(3), 'edited capture extension list allows mp4 hijacks');
  await harness.chrome.storage.local.set({ falconCaptureExtensions: '' });
  await fireDeterminingFilename(harness, item(4, 'third.zip'));
  assert(!harness.cancels.includes(4), 'stored empty capture extension list disables automatic capture');
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
