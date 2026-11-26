// Pairing with the desktop app.
//
// The extension shows a 6-digit code and asks Falcon DM to pair. The app shows
// the same code and the user approves it there. The API token only reaches the
// extension after that approval. Web pages cannot forge the chrome-extension://
// Origin the request carries.

const PAIR_CODE_KEY = 'falconPairCode';
let pairCode = '';

function newPairCode() {
  const bytes = new Uint32Array(1);
  crypto.getRandomValues(bytes);
  return String(bytes[0] % 1000000).padStart(6, '0');
}

/** The code currently shown to the user. Kept until the app answers. */
async function currentPairCode(fresh = false) {
  if (!fresh && pairCode) return pairCode;
  if (!fresh) {
    const stored = await chrome.storage.local.get(PAIR_CODE_KEY).catch(() => ({}));
    if (/^\d{6}$/.test(stored[PAIR_CODE_KEY] || '')) {
      pairCode = stored[PAIR_CODE_KEY];
      return pairCode;
    }
  }
  pairCode = newPairCode();
  await chrome.storage.local.set({ [PAIR_CODE_KEY]: pairCode }).catch(() => {});
  return pairCode;
}

async function forgetPairCode() {
  pairCode = '';
  await chrome.storage.local.remove(PAIR_CODE_KEY).catch(() => {});
}

/** Code to show in the popup while the app waits for approval. */
function visiblePairCode() {
  return connectionState === 'pending' ? pairCode : '';
}

async function requestPair() {
  const code = await currentPairCode();
  return fetchWithTimeout(
    `${FALCON_API}/api/pair`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ extension_id: chrome.runtime.id, code }),
    },
    REQUEST_TIMEOUT_MS,
    'Pair request',
  );
}

/** 200 token · 202 waiting · 403 declined · 503 app not ready yet. */
async function readPairResponse(resp) {
  if (resp.status === 202) return { pending: true };
  if (resp.status === 403) return { rejected: true };
  if (resp.status === 503) return { retry: true };
  if (!resp.ok) return { error: true };
  const data = await resp.json().catch(() => null);
  if (data && data.ok && data.token) {
    await chrome.storage.local.set({ apiToken: data.token });
    await forgetPairCode();
    return { token: data.token };
  }
  return data && data.pending ? { pending: true } : { error: true };
}

async function pingToken(token) {
  try {
    const r = await fetchWithTimeout(
      `${FALCON_API}/api/ping`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Falcon-Token': token },
        body: '{}',
      },
      REQUEST_TIMEOUT_MS,
      'Falcon ping',
    );
    return r.status;
  } catch {
    return 0;
  }
}

async function declined() {
  // The next attempt shows a new code, so the user can try again on purpose.
  await forgetPairCode();
  setState('blocked');
  throw new Error(
    msg('errorPairRejected', 'Falcon DM declined the request. Try again to get a new code.'),
  );
}

async function ensurePaired(force = false) {
  if (pairInFlight) return pairInFlight;
  pairInFlight = (async () => {
    try {
      if (!(await appHealthy())) {
        await withTimeout(wakeFalcon(), 5000, 'Falcon wake');
        if (!(await waitForHealthy(25000))) {
          throw new Error(msg('errorWaking', 'Falcon DM did not start. Open it manually.'));
        }
      }

      const existing = force ? '' : await getToken();
      if (existing) {
        const status = await pingToken(existing);
        if (status >= 200 && status < 300) {
          setState('connected');
          return existing;
        }
        // 401: token rotated (another extension was disconnected) — pair again.
        // 403: this extension was disconnected — needs a new approval.
        await chrome.storage.local.remove('apiToken');
      }

      let result = { retry: true };
      for (let i = 0; result.retry && i < 6; i++) {
        if (i) await sleep(500);
        result = await readPairResponse(await requestPair());
      }
      if (result.token) {
        setState('connected');
        return result.token;
      }
      if (result.rejected) return declined();
      if (!result.pending) {
        throw new Error(msg('errorAppOffline', 'Could not pair with Falcon DM'));
      }

      setState('pending');
      notify(
        msg('appName', 'Falcon DM'),
        msg('pairNotify', 'Approve code {code} in Falcon DM').replace('{code}', formatPairCode(pairCode)),
      );
      for (let i = 0; i < PAIR_POLL_ATTEMPTS; i++) {
        await sleep(2000);
        let next;
        try {
          next = await readPairResponse(await requestPair());
        } catch {
          continue;
        }
        if (next.token) {
          setState('connected');
          return next.token;
        }
        if (next.rejected) return declined();
      }
      throw new Error(msg('errorPairPending', 'Approve the code in Falcon DM, then try again.'));
    } catch (e) {
      if (connectionState !== 'blocked' && connectionState !== 'pending') setState('offline');
      throw e;
    } finally {
      pairInFlight = null;
    }
  })();
  return pairInFlight;
}

function formatPairCode(code) {
  return code ? `${code.slice(0, 3)} ${code.slice(3)}` : '';
}
