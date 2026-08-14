
function mapHttpError(status, bodyError) {
  if (status === 401) return msg("errorTokenInvalid", "Connection lost — Falcon will re-pair automatically");
  if (status === 403)
    return msg(
      "errorExtensionBlocked",
      "Extension blocked — Falcon DM Settings → Reconnect extension"
    );
  if (status === 429) return msg("errorRateLimit", "Too many requests — try again shortly");
  if (bodyError) return bodyError;
  if (status) return msg("errorHttp", "Request failed") + ` (HTTP ${status})`;
  return msg("errorAppOffline", "Falcon DM is not running — open the desktop app");
}

async function postFalcon(path, body) {
  let token = await ensurePaired(false);

  let response;
  try {
    response = await fetchWithTimeout(
      `${FALCON_API}${path}`,
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-Falcon-Token": token,
        },
        body: JSON.stringify(body),
      },
      REQUEST_TIMEOUT_MS,
      `Falcon ${path}`
    );
  } catch {
    setState("offline");
    throw new Error(msg("errorAppOffline", "Falcon DM is not running — open the desktop app"));
  }

  if (response.status === 401) {
    token = await ensurePaired(true);
    try {
      response = await fetchWithTimeout(
        `${FALCON_API}${path}`,
        {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "X-Falcon-Token": token,
          },
          body: JSON.stringify(body),
        },
        REQUEST_TIMEOUT_MS,
        `Falcon ${path} retry`
      );
    } catch {
      setState("offline");
      throw new Error(msg("errorAppOffline", "Falcon DM is not running — open the desktop app"));
    }
  }

  let data = null;
  try {
    data = await response.json();
  } catch {
    data = null;
  }

  if (!response.ok) {
    throw new Error(mapHttpError(response.status, data && data.error));
  }
  if (data && data.success === false) {
    const err = data.error || msg("errorInvalidUrl", "Invalid download URL");
    if (err === "invalid url") throw new Error(msg("errorInvalidUrl", "No valid URL"));
    throw new Error(err);
  }
  try {
    if (body && body.url) {
      trackDownload(body.filename || "", body.url, path === "/api/intercept" ? "media" : "file");
    }
  } catch (_) {}
  return data;
}

async function sendToFalcon(path, body) {
  if (await appHealthy()) {
    return postFalcon(path, body);
  }

  await withTimeout(wakeFalcon(), 5000, "Falcon wake");
  if (await waitForHealthy(30000)) {
    return postFalcon(path, body);
  }

  setState("offline");
  throw new Error(msg("errorWaking", "Falcon DM başlatılamadı"));
}

