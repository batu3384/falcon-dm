/**
 * Smoke checks for extension media helpers.
 * Usage: node extension/smoke-test.mjs
 */
import { readFileSync } from "fs";
import vm from "vm";
import path from "path";
import { fileURLToPath } from "url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const src = readFileSync(path.join(__dirname, "media-utils.js"), "utf8");
const background = readFileSync(path.join(__dirname, "background.js"), "utf8");
const content = readFileSync(path.join(__dirname, "content.js"), "utf8");
const manifest = JSON.parse(readFileSync(path.join(__dirname, "manifest.json"), "utf8"));
const sandbox = { console, globalThis: {} };
sandbox.globalThis = sandbox;
vm.createContext(sandbox);
vm.runInContext(src, sandbox);
const FM = sandbox.FalconMedia || sandbox.globalThis.FalconMedia;
if (!FM) throw new Error("FalconMedia not exported");

function assert(cond, msg) {
  if (!cond) throw new Error(msg);
}

assert(FM.isJunkUrl("https://x.com/no_input.mp3"), "junk no_input");
assert(!FM.isJunkUrl("https://cdn.example.com/video.mp4"), "real mp4");
assert(FM.isCapturableMedia("https://x.com/a.m3u8", "application/vnd.apple.mpegurl"), "hls");
const norm = FM.normalizeMediaUrl("https://googlevideo.com/videoplayback?id=abc&range=0-100&other=1");
assert(!norm.includes("range="), "strip range");
assert(norm.includes("id=abc"), "keep id");
assert(manifest.permissions.includes("nativeMessaging"), "native messaging permission");
assert(
  manifest.host_permissions.includes("http://127.0.0.1:14201/*"),
  "localhost host permission"
);
assert(background.includes("sendNativeMessage"), "native pairing proof");
assert(background.includes("withTimeout"), "bounded request timeout");
assert(background.includes('suggest({ cancel: false })'), "download fallback");
assert(background.includes("Promise.allSettled"), "batch partial results");
assert(background.includes("getCookiesHeader"), "target cookie lookup");
assert(background.includes("results"), "batch result contract");
assert(background.includes("format"), "YouTube format field");
assert(background.includes("chrome.tabs.onUpdated"), "tab navigation cleanup");
assert(content.includes("pageUrl.split"), "YouTube watch URL");
assert(content.includes("Math.min(Math.max(h, 144), 2160)"), "bounded YouTube height");
assert(content.includes("googlevideo"), "YouTube CDN source guard");

console.log("extension smoke ok");
