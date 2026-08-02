function t(key, fallback) {
  return chrome.i18n.getMessage(key) || fallback;
}

const $ = (id) => document.getElementById(id);
const statusEl = $("status");
const queueEl = $("queue");
const pageUrlEl = $("page-url");
const grabBtn = $("grab");
const dlUrlBtn = $("dl-url");
const reconnectBtn = $("reconnect");
const pauseBtn = $("pause");

let paused = false;

function setState(state) {
  statusEl.dataset.state = state;
  statusEl.textContent = t(
    state === "connected"
      ? "popupStateConnected"
      : state === "pending"
        ? "popupStatePending"
        : "popupStateOffline",
    state === "connected" ? "Connected" : state === "pending" ? "Pending" : "Offline"
  );
}

function timeAgo(ts) {
  const s = Math.max(1, Math.round((Date.now() - ts) / 1000));
  if (s < 60) return t("popupTimeNow", "just now");
  return t("popupTimeMin", "{n}m ago").replace("{n}", String(Math.floor(s / 60)));
}

function renderQueue(recent) {
  queueEl.innerHTML = "";
  if (!recent || !recent.length) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = t("popupQueueEmpty", "No recent downloads");
    queueEl.appendChild(li);
    return;
  }
  recent.slice(0, 3).forEach((it) => {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.className = "qname";
    name.textContent = it.filename || "download";
    name.title = it.filename || "";
    const kind = document.createElement("span");
    kind.className = "qkind";
    kind.textContent = it.kind === "media" ? "MEDIA" : "FILE";
    const meta = document.createElement("span");
    meta.className = "qmeta";
    meta.textContent = (it.host ? it.host + " · " : "") + timeAgo(it.ts);
    li.appendChild(name);
    li.appendChild(kind);
    li.appendChild(meta);
    queueEl.appendChild(li);
  });
}

function applyStatus(resp) {
  if (!resp) return;
  setState(resp.state || "offline");
  paused = !!resp.paused;
  pauseBtn.textContent = paused ? t("popupResume", "Resume") : t("popupPause", "Pause");
  renderQueue(resp.recent || []);
}

function send(action, payload) {
  return new Promise((resolve) => {
    chrome.runtime.sendMessage({ action, ...payload }, (resp) => {
      if (chrome.runtime.lastError) resolve(null);
      else resolve(resp);
    });
  });
}

function refresh() {
  send("check_status").then(applyStatus);
}

async function initPage() {
  try {
    const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
    const url = tab && tab.url ? tab.url : "";
    pageUrlEl.textContent = url || "—";
    pageUrlEl.title = url || "";
    dlUrlBtn.disabled = !/^https?:/i.test(url);
  } catch (_) {
    dlUrlBtn.disabled = true;
  }
}

function flash(btn, text) {
  const orig = btn.textContent;
  btn.textContent = text;
  setTimeout(() => { btn.textContent = orig; }, 1800);
}

grabBtn.addEventListener("click", async () => {
  grabBtn.disabled = true;
  const resp = await send("grab_tab_media");
  grabBtn.disabled = false;
  if (resp && resp.ok) {
    window.close();
  } else {
    flash(grabBtn, (resp && resp.error) || t("errorAppOffline", "Failed"));
  }
});

dlUrlBtn.addEventListener("click", async () => {
  const url = pageUrlEl.title;
  if (!url) return;
  dlUrlBtn.disabled = true;
  const resp = await send("add_url", { url });
  dlUrlBtn.disabled = false;
  if (resp && resp.success) refresh();
  else flash(dlUrlBtn, (resp && resp.error) || t("errorAppOffline", "Failed"));
});

reconnectBtn.addEventListener("click", async () => {
  reconnectBtn.disabled = true;
  await send("auto_pair");
  reconnectBtn.disabled = false;
  refresh();
});

pauseBtn.addEventListener("click", async () => {
  const resp = await send("set_paused", { paused: !paused });
  applyStatus(
    resp || { state: statusEl.dataset.state, paused: !paused, recent: [] }
  );
});

$("settings").addEventListener("click", (e) => {
  e.preventDefault();
  chrome.runtime.openOptionsPage();
});

initPage();
refresh();
