function t(key, fallback) {
  return chrome.i18n.getMessage(key) || fallback;
}

const statusEl = document.getElementById("status");

function setStatus(text, kind) {
  statusEl.textContent = text;
  statusEl.className = "status-card " + (kind || "");
}

document.getElementById("title").textContent = t("optionsTitle", "Falcon DM Connection");
document.getElementById("desc").textContent = t(
  "optionsDesc",
  "Connects automatically when Falcon DM desktop app is open. No token paste needed."
);
document.getElementById("pair").textContent = t("optionsPair", "Connect now");
document.getElementById("test").textContent = t("optionsTest", "Test connection");
document.getElementById("hint").textContent = t(
  "optionsHint",
  "If blocked: Falcon DM → Settings → Reconnect extension, then click Connect now."
);

function refresh() {
  setStatus(t("sending", "Checking..."), "");
  chrome.runtime.sendMessage({ action: "check_connection" }, (resp) => {
    if (resp && resp.ok) {
      setStatus(t("optionsConnected", "Connected — downloads go to Falcon DM"), "ok");
    } else {
      setStatus(resp?.error || t("errorAppOffline", "Falcon DM is not running"), "err");
    }
  });
}

document.getElementById("pair").addEventListener("click", () => {
  setStatus(t("sending", "Connecting..."), "");
  chrome.runtime.sendMessage({ action: "auto_pair" }, (resp) => {
    if (resp && resp.ok) {
      setStatus(t("optionsConnected", "Connected — downloads go to Falcon DM"), "ok");
    } else {
      setStatus(resp?.error || t("errorAppOffline", "Connection failed"), "err");
    }
  });
});

document.getElementById("test").addEventListener("click", refresh);

refresh();
