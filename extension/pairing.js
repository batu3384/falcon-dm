function newPairChallenge() {
  if (crypto.randomUUID) return crypto.randomUUID();
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function getNativePairProof(challenge, timeoutMs = 3000) {
  return new Promise((resolve, reject) => {
    if (!chrome.runtime.sendNativeMessage) {
      reject(new Error("Native messaging is unavailable"));
      return;
    }
    let settled = false;
    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        reject(new Error("Native pairing timed out"));
      }
    }, timeoutMs);
    try {
      chrome.runtime.sendNativeMessage(
        "com.falcondm.native",
        { extension_id: chrome.runtime.id, challenge },
        (response) => {
          if (settled) return;
          settled = true;
          clearTimeout(timer);
          const runtimeError = chrome.runtime.lastError;
          if (runtimeError) {
            reject(new Error(runtimeError.message));
            return;
          }
          if (!response || !response.ok || !response.proof) {
            reject(new Error(response?.error || "Native pairing failed"));
            return;
          }
          resolve(response.proof);
        },
      );
    } catch (error) {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        reject(error);
      }
    }
  });
}

async function requestPair() {
  const challenge = newPairChallenge();
  const proof = await getNativePairProof(challenge);
  return fetchWithTimeout(
    `${FALCON_API}/api/pair`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        extension_id: chrome.runtime.id,
        challenge,
        proof,
      }),
    },
    REQUEST_TIMEOUT_MS,
    "Pair request"
  );
}

async function ensurePaired(force = false) {
  if (pairInFlight) return pairInFlight;
  pairInFlight = (async () => {
    try {
      if (!force) {
        const existing = await getToken();
        if (existing && (await appHealthy())) {
          try {
            const r = await fetchWithTimeout(
              `${FALCON_API}/api/ping`,
              {
                method: "POST",
                headers: {
                  "Content-Type": "application/json",
                  "X-Falcon-Token": existing,
                },
                body: "{}",
              },
              REQUEST_TIMEOUT_MS,
              "Falcon ping"
            );
            if (r.ok) {
              setState("connected");
              return existing;
            }
          } catch (_) {}
        }
      }

      if (!(await appHealthy())) {
        await withTimeout(wakeFalcon(), 5000, "Falcon wake");
        if (!(await waitForHealthy(25000))) {
          throw new Error(
            msg("errorWaking", "Falcon DM başlatılamadı — uygulamayı kurun veya manuel açın")
          );
        }
      }

      const r = await requestPair();
      if (r.status === 403) {
        throw new Error(
          msg(
            "errorExtensionBlocked",
            "Extension blocked — open Falcon DM Settings → Reconnect extension"
          )
        );
      }

      const finishPair = async (resp) => {
        if (resp.status === 202) return { pending: true };
        // 503: app is up but token not provisioned yet (cold-start race). Caller retries.
        if (resp.status === 503) return { retry: true };
        if (!resp.ok) return { error: true };
        const data = await resp.json().catch(() => null);
        if (data && data.pending) return { pending: true };
        if (data && data.token && data.ok) {
          await chrome.storage.local.set({ apiToken: data.token });
          return { token: data.token };
        }
        return { error: true };
      };

      let first = await finishPair(r);
      // Cold-start 503 race: short backoff + retry before giving up.
      for (let i = 0; first.retry && i < 5; i++) {
        await sleep(500);
        const rr = await requestPair();
        if (rr.status === 403) {
          throw new Error(
            msg(
              "errorExtensionBlocked",
              "Extension blocked — open Falcon DM Settings → Reconnect extension"
            )
          );
        }
        first = await finishPair(rr);
      }
      if (first.token) {
        setState("connected");
        return first.token;
      }
      if (first.error) {
        throw new Error(msg("errorAppOffline", "Could not pair with Falcon DM"));
      }

      // Pending approval — poll until Settings approve (or timeout)
      setState("pending");
      notify(
        msg("appName", "Falcon DM"),
        msg(
          "errorPairPending",
          "Approve this extension in Falcon DM Settings, then try again"
        )
      );
      for (let i = 0; i < PAIR_POLL_ATTEMPTS; i++) {
        await new Promise((res) => setTimeout(res, 2000));
        let r2;
        try {
          r2 = await requestPair();
        } catch {
          continue;
        }
        if (r2.status === 403) {
          throw new Error(
            msg(
              "errorExtensionBlocked",
              "Extension blocked — open Falcon DM Settings → Reconnect extension"
            )
          );
        }
        const again = await finishPair(r2);
        if (again.token) {
          setState("connected");
          return again.token;
        }
      }
      throw new Error(
        msg(
          "errorPairPending",
          "Approve this extension in Falcon DM Settings, then try again"
        )
      );
    } catch (e) {
      setState("offline");
      throw e;
    } finally {
      pairInFlight = null;
    }
  })();
  return pairInFlight;
}
