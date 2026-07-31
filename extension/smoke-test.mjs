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

console.log("extension smoke ok");
