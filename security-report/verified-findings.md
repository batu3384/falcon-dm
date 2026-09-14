# Verified Findings — Falcon DM (2026-09-06)

## Fixed this session

| ID | Severity | Location | Finding | Status |
|----|----------|----------|---------|--------|
| F-01 | Medium | extension/background.js intercept | googlevideo intercept used wrong cookie_url | **Fixed** — cookieLookupUrl |
| F-02 | Medium | extension/background.js context menu | Same cookie gap on right-click CDN links | **Fixed** |
| F-03 | Medium | extension/shared.js, content.js | Substring youtube/googlevideo spoof | **Fixed** — host-based helpers |
| F-04 | Medium | src-tauri/util/net.rs | Referrer substring in youtube_page_url_for_download | **Fixed** |
| F-05 | Low | src-tauri/lib.rs resolve_media_referer | Substring youtube.com check | **Fixed** |

## Accepted residual (documented)

| ID | Severity | Location | Finding | Rationale |
|----|----------|----------|---------|-----------|
| R-01 | Low | YouTube CDN | Signed URLs expire without playback | Inherent to YouTube; yt-dlp fallback exists |
| R-02 | Info | extension manifest | `<all_urls>` permission | Required for generic download capture |
| R-03 | Low | SQLite | Active cookies plaintext in DB | Local-only; cleared on complete |
| R-04 | Info | index.html | Google Fonts CDN | **Closed (F-131)** — system font stack; production CSP `font-src 'self'` |

## False positives eliminated
- Cookie leak on HTTP redirect — mitigated by `youtube_cdn_capture(current)` (not a finding)
- evil-youtube.com cookie_url — blocked by `is_youtube_host` (not exploitable)

## 2026-09-14 follow-up (fixed)

| ID | Severity | Location | Finding | Status |
|----|----------|----------|---------|--------|
| F-06 | Medium | extension/shared.js hydrate | Pause lived in `session` — Chrome restart re-enabled hijack | **Fixed** — `chrome.storage.local` + session migrate |
| F-07 | Medium | extension/background.js hijack | First download before prefs hydrate used `interceptPaused=false` | **Fixed** — `prefsReady` / `prefsHydrated` |
| F-08 | Medium | extension popup/background/content | Invalid URL catch used `/youtube.com/` path regex (spoof) | **Fixed** — `FalconMedia.isYoutubePageUrl` |
| F-09 | Low | clipboard + NewDownloadModal | Loopback/RFC1918 URLs queued then failed SSRF check | **Fixed** — `isPublicHttpUrl` (CGNAT/ULA too) |
| F-10 | Low | useModalA11y | New download modal focused Cancel (X), not URL field | **Fixed** — `data-modal-initial-focus` |
| F-11 | Low | shared.js vs publicHttpUrl.ts | Duplicate host-block drifted | **Fixed** — SSOT `FalconMedia.isBlockedLiteralHost` + TS sync marker |
| F-12 | Medium | media-utils.js `isYoutubeWatchPage` | `/shorts/` matched any host → overlay forced yt-dlp on spoof pages | **Fixed** — alias of `isYoutubeWatchUrl` (host-gated) |
| F-13 | Medium | lib.rs ingest_uploaded_file | `fs::write` overwrote existing blob dest | **Fixed** — unique name + `write_file_exclusive` |
| F-14 | Medium | path.rs `resolve_save_dir` | `create_dir_all` ran before allowlist | **Fixed** — resolve without mkdir |
| F-15 | Low | extension overlay | Focused close (X); stacked sheets; no alert | **Fixed** — filename focus, one overlay, restore focus, `role=alert` |
| F-16 | Medium | enqueue_download | Same filename reused → two rows, one file; delete-from-disk clobbers both | **Fixed** — `unique_download_filename` at enqueue; insert tx also reserves DB names (F-16b) |
| F-17 | Low | App drop | `text/uri-list` / multi-URL / private URL silent fail | **Fixed** — `publicHttpUrlsFromText` + batch + toast |
| F-18 | Low | DownloadList | Virtual list not keyboard-traversable | **Fixed** — listbox + Arrow/Home/End |
| F-19 | Low | DownloadList listbox | `option` nested under spacer/row; error banner inside listbox | **Fixed** — option on virtual row, `presentation` spacer, alert outside |
| F-20 | Low | move_download | Trailing `.` / `?` in rename rejected instead of sanitized | **Fixed** — store `sanitize_filename` result |
| F-21 | Medium | queue.rs tick | yt-dlp source `None` left row Queued — `take_queued(1)` blocked forever | **Fixed** — mark Failed |
| F-22 | Low | Settings/Log tabs | tablist without arrows / tabpanel | **Fixed** — roving tabindex + Arrow/Home/End |
| F-23 | Medium | queue.rs yt-dlp | `.webm`→`.mp4` rewrite reused existing dest (`clip.mp4`) | **Fixed** — `uniquify_assigned_filename` after rewrite + HLS spawn |
| F-24 | Medium | queue.rs `run_stream_task` | Missing dest still marked Completed (size 0) | **Fixed** — `validate_completed_file` before `finish_stream_if_active` |
| F-25 | Low | NewDownloadModal / drop | `[,;\\s]+` split cut query commas (`?ids=1,2`); batch `#` comments failed whole form | **Fixed** — `publicHttpUrlsFromMixedText` lookahead split |
| F-26 | Low | DownloadItem / Inspector | Progress tracks had no `progressbar` semantics | **Fixed** — `role=progressbar` + `aria-valuenow` |
| F-27 | Low | popup.html | Pending/blocked pairing boxes not announced | **Fixed** — `role=status` / `role=alert` |
| F-28 | Low | extension/content.js `el('button')` | Overlay/grabber buttons defaulted to `submit` | **Fixed** — `el()` sets `type=button` unless specified |
| F-29 | Low | grabber / `download_video` | Private literal URLs reached enqueue (backend SSRF still blocked) | **Fixed** — `isPublicHttpUrl` in grabber + SW enqueue |
| F-16b | Low | insert_download_deduped | Two queued jobs could share a dest (disk unique TOCTOU) | **Fixed** — uniquify vs disk + reserved DB names in `IMMEDIATE` tx |
| F-31 | Medium | http.rs dest exists | Invalid existing dest was `remove_file`'d — could delete a user file that appeared after enqueue | **Fixed** — `inspect_download_dest`; Occupied/foreign Complete uniquify, never delete |
| F-32 | Medium | store applyProgress / mergeLiveProgress | Late Downloading tick (event or poll) overwrote Completed | **Fixed** — Completed sticky; Failed still accepts retry; live speed is current not peak |
| F-33 | Medium | net.rs + publicHttpUrl + media-utils | `.localhost`, `0.0.0.0/8`, multicast, and hex-mapped `::ffff:7f00:1` reached the UI/enqueue filter | **Fixed** — same host block in Rust/TS/extension |
| F-34 | Low | NewDownloadModal dirty | Clipboard/`initialUrl` + auto filename counted as a draft — Escape asked to discard | **Fixed** — dirty only on user edits |
| F-35 | Low | NewDownloadModal dropzone | `role=button` wrapped file input (nested interactive) | **Fixed** — drop target is not a button; browse control stays the file input |
| F-36 | Low | LogPanel level filter | `tablist` without `tabpanel` | **Fixed** — `radiogroup` + `aria-checked` |
| F-37 | Medium | lib.rs fail_invalid_download | Aria2/poll finalize failure `remove_file`'d dest — could delete a user file | **Fixed** — mark Failed only, never delete |
| F-38 | Medium | NewDownloadModal batch cookies | `cookieUrl` was each download URL, so pasted cookies attached to every host | **Fixed** — one `enqueueCookieUrl` (referrer or first URL); backend origin match drops the rest |
| F-39 | Low | ProfilesTab | Profile fields had placeholders only, no accessible name | **Fixed** — `aria-label` on each input |
| F-40 | Low | DownloadList virtual listbox | Selected row unmounted → `aria-activedescendant` pointed at missing option | **Fixed** — `rangeExtractor` always includes selected index |
| F-41 | Low | SettingsModal tabs | Inactive `tabpanel` unmounted so `aria-controls` was a dangling id; Tab could enter hidden fields | **Fixed** — panels stay in DOM with `hidden`; modal focus skips `[hidden]` ancestors |
| F-42 | Low | extension overlay quality cards | Radios had a name but no `radiogroup`; video-only warning not live | **Fixed** — `radiogroup` + `role=status` |
| F-43 | Low | CommandPalette | Combobox list had no Home/End | **Fixed** — Home/End move active option |
| F-44 | Low | DownloadItem row keys | Space/Enter/Delete on pause/delete buttons also toggled select / opened remove confirm | **Fixed** — row shortcuts ignore inner controls |
| F-45 | Low | OnboardingWizard overlay | Backdrop click marked onboarding complete (step 2 has no Skip) | **Fixed** — overlay is not a dismiss target |
| F-46 | Low | extension overlay/grabber | Quality heading was a `<label>` (wrong implicit control); grabber errors not live | **Fixed** — heading `div`; grabber fail `role=alert` |
| F-47 | Low | sanitize_header_value | Only stripped CR/LF; NUL/other controls stayed in Cookie/Referer | **Fixed** — drop ASCII controls and DEL |
| F-48 | Medium | commands.rs save_settings | Stale Settings modal snapshot could wipe an extension ID approved while the modal was open | **Fixed** — allowlist owned by pair/reset; save keeps current |
| F-49 | Medium | approve_extension_pair | Invalid IDs (not `[a-p]{32}`) could enter the native allowlist | **Fixed** — `parse_extension_id` |
| F-50 | Medium | check_for_updates + GeneralTab | GitHub `html_url` opened without host/path gate; redirects could leave github.com | **Fixed** — repo path gate + GitHub-only redirects + UI `openUrl` check |
| F-51 | Medium | Settings / queue | IPC could set concurrent=99 or connections=0; queue only had `max(1)` | **Fixed** — `clamp_runtime_bounds` + queue `1..=32` |
| F-52 | Medium | yt-dlp `-f` | Extension `format` / `#falconfmt=` reached argv unsanitized | **Fixed** — selector charset; drop `--` / leading `-` |
| F-53 | Low | InspectorPanel | Invalid `created_at` rendered `Invalid Date` | **Fixed** — `formatLocaleDate` |
| F-54 | Low | App toolbar search | Every keystroke hit `get_downloads` | **Fixed** — 280ms debounce; client filter stays live |
| F-55 | Medium | default_download_path | `/`, `/etc`, `/tmp` as save root passed allowlist (`starts_with("/")`) | **Fixed** — `is_unsafe_download_root` before mkdir |
| F-56 | Low | validate_open_path | Open only allowed Downloads/Movies/… so Desktop/custom home saves could not open | **Fixed** — entire home dir is an allowed open root |
| F-57 | Medium | isPublicHttpUrl 2048 | Signed googlevideo URLs often exceed 2048 → hijack/grabber drop | **Fixed** — 8192 cap in TS/extension/Rust; Cookie/UA clipped to 8192 |
| F-58 | High | background hijackKey | URL-based in-flight aborted a second Chrome download of the same URL without enqueue | **Fixed** — key by Chrome download id |
| F-59 | Medium | onCreated backup | prefsReady later rescheduled backup after determining already hijacked | **Fixed** — skip aborted/watch ids after hydrate |
| F-60 | Medium | add_url / context / hijack | Private URLs reached Falcon; hijack cancelled Chrome LAN downloads | **Fixed** — `isPublicEnqueueUrl`; hijack lets Chrome keep LAN |
| F-61 | Low | check_status | Popup clipboard toggle always showed off | **Fixed** — `clipboardMonitor` on check_status |
| F-62 | Medium | HTTP Complete skip | Existing complete dest marked our job Completed | **Fixed** — uniquify Complete; HTTP treats as occupied |
| F-63 | Medium | store merge/applyProgress | Poll Queued stayed Downloading; Failed could not override premature Completed | **Fixed** — Queued/Failed from backend win |
| F-64 | Low | yt-dlp Completed emit | Worker emitted Completed before `finish_stream_if_active` | **Fixed** — only queue finalize emits Completed |
| F-65 | Medium | queue.rs tick | Scheduler/concurrency cancel timeout `continue` skipped Queued write — UI stuck Downloading | **Fixed** — always write Queued; `finish_stream_if_active` accepts Queued; pause command writes Paused on timeout |
| F-66 | Medium | get_downloads | Internal pause/tick used `limit: 256` so 257+ active rows were skipped | **Fixed** — internal callers use unlimited filter; IPC still clamps to 200 |
| F-67 | Low | DownloadItem menu | Focus landed on empty `role=menu`; no Home/End; Arrow leaked to listbox | **Fixed** — first menuitem + Home/End; keydown stopPropagation |
| F-68 | Medium | queue.rs scheduled pause | Expected statuses included Paused so a user pause could be rewritten Queued and auto-resume | **Fixed** — only Downloading/Merging → Queued |
| F-69 | Low | ConfirmDialog | Hardcoded `confirm-msg` id + labelledby=describedby; two dialogs collided | **Fixed** — `useId`; describedby only for delete hint |
| F-70 | Low | DownloadItem date + memo | Row used raw `Date` (`Invalid Date`); memo ignored `save_path` after move | **Fixed** — `formatLocaleDate`; memo includes path/date/archived |
| F-71 | Low | DownloadList live pill | Counted Downloading only, so Merging jobs disappeared from the active badge | **Fixed** — same filter as the Active view |
| F-72 | Low | InspectorPanel | Merging treated as idle (no active bar, false idle) | **Fixed** — live = Downloading\|Merging; speed still Downloading-only |
| F-73 | Medium | store selectDownload | Shift-select used the unfiltered store order, so a Video view range grabbed Music rows in between | **Fixed** — range uses visible ids from the list |
| F-74 | Low | OnboardingWizard | Escape on step 2 marked onboarding complete (no Skip on that step) | **Fixed** — Escape goes Back; skip only on step 1 |
| F-75 | Low | store applyProgress | Queued/Merging ticks kept the previous download speed | **Fixed** — speed 0 unless status is Downloading |
| F-76 | Medium | store fetchDownloads | Sidebar Failed/Video/etc. filtered the unpaginated All page, so older matching rows vanished | **Fixed** — `viewToDownloadFilter` + SQL `status IN` / category |
| F-77 | Low | NetworkTab number inputs | Empty field stored `NaN` and save failed | **Fixed** — parse fallback 1 |
| F-78 | Medium | store addDownload / DownloadList | Archived (and Failed) views prepended live captures; list treated Archived as already-filtered | **Fixed** — `downloadMatchesView` / `downloadAdmittedToView`; Active still admits Queued so progress can reveal it |
| F-79 | Low | GeneralTab | `useToastStore` used without import — update check toast would not compile | **Fixed** — import |
| F-80 | Medium | store applyProgress | Pause then stale Downloading/Merging ticks revived the row | **Fixed** — Paused sticky until Failed/Completed/Paused |
| F-81 | Medium | copy_file_exclusive / move_download | Source unlink fail left dest (retry blocked); DB fail after move left dest+DB src | **Fixed** — rollback dest on unlink fail; reverse-copy on DB fail |
| F-82 | Medium | ytdlp.rs | `ytdlp_use_browser_cookies` UI/setting never reached argv | **Fixed** — `--cookies-from-browser chrome` when enabled |
| F-83 | Low | ytdlp.rs | URL passed without `--` separator | **Fixed** — `cmd.arg("--").arg(url)` |
| F-84 | Low | get_downloads IPC | Unbounded `statuses` IN list | **Fixed** — truncate 8 |
| F-85 | Medium | queue.rs Cancelled | Worker wrote Paused, so scheduled/concurrency Queued update (Downloading\|Merging only) never landed — HTTP/yt-dlp/HLS jobs stayed Paused after the window | **Fixed** — Cancelled leaves status; caller writes Paused or Queued |
| F-86 | Medium | insert_download_deduped | YouTube `#falconfmt` LIKE used raw watch URL; `_` in video ids matched other videos and dropped the enqueue | **Fixed** — LIKE ESCAPE after `like_escape` |
| F-87 | Low | DownloadList | View count was the loaded page (200) while sidebar had the real total; ArrowDown at end never loaded more | **Fixed** — `200+` when hasMore; last-row ArrowDown calls load more |
| F-88 | Medium | resume_if_current | Pause→resume zeroed downloaded/total size so the bar reset | **Fixed** — keep bytes unless status is Failed |
| F-89 | Medium | tray Pause/Resume All | Pause skipped Queued (tick auto-started them); Resume skipped Failed | **Fixed** — same status sets as toolbar capabilities |
| F-90 | Low | is_junk_media_url | `doubleclick` / `youtube.com` substring rejected legitimate files | **Fixed** — host-gated; path/query for YouTube placeholders |
| F-91 | Low | is_googlevideo_url | Parse-fail fallback used `contains("googlevideo.com")` | **Fixed** — host only |
| F-92 | Low | background isPublicEnqueueUrl | Missing FalconMedia helper fail-opened private URLs | **Fixed** — fail-closed |
| F-93 | Low | media-utils isJunkUrl | `/doubleclick`, `/static/`, `youtube.com`+.mp3 substring flagged non-YouTube files | **Fixed** — parse URL; doubleclick host; YouTube UI paths host-gated |
| F-94 | Low | isSniffableMedia / hijackPayload | `videoplayback` / `mime=video` substring treated spoof paths as CDN | **Fixed** — googlevideo host only |
| F-95 | Low | DownloadItem context menu | Right-click on the selected row cleared selection / closed inspector | **Fixed** — context menu always selects the row |
| F-96 | Low | DownloadList | listbox+option wrapped nested pause/delete buttons | **Fixed** — `list`/`listitem`; keyboard nav kept |
| F-97 | Low | api/events.ts | zod `.parse` throw on one bad tick; pair toast could show `undefined` | **Fixed** — safeParse drop; pair id required |
| F-98 | Low | ProgressPayload / applyProgress | Failed stream event omitted `error_message` so Inspector stayed blank until poll | **Fixed** — optional error on ticks; UI copies it |
| F-99 | Low | media-utils hostLabel | `hostname.includes('youtube')` labeled `evil-youtube.com` as YouTube | **Fixed** — host helpers |
| F-100 | Low | DownloadList | Empty/skeleton lived inside `role=list` with no listitems | **Fixed** — list role only when rows exist |
| F-101 | Low | InspectorPanel | Unknown total still announced 0% (row already used indeterminate) | **Fixed** — same progressbar/… as the row |
| F-102 | Low | StatusBar | sr-only used completed count as Total; visible count ignored `hasMore` | **Fixed** — live region matches visible; `200+` |
| F-103 | Low | is_junk_media_url / isJunkUrl | `/s/search/`, `/ptracking`, `/log_event` matched any host | **Fixed** — YouTube-host only |
| F-104 | Medium | commands.rs pause_download | aria2 pause error aborted HLS/HTTP pause (tray already fail-open) | **Fixed** — warn + still cancel stream / write Paused |
| F-105 | Low | store selectDownload | Cmd-click deselect kept inspector on the removed row (last item left inspector open) | **Fixed** — inspector follows remaining selection or closes |
| F-106 | Low | DownloadItem / InspectorPanel | Indeterminate bar still announced `0 percent complete` | **Fixed** — label/valuetext use Downloading/Merging |
| F-107 | Low | useClipboardMonitor | `/api/stats` `/log_event` clipboard junk matched any host | **Fixed** — YouTube-host only (same as F-103) |
| F-108 | Low | DownloadList | `role=list` still had `aria-activedescendant` (listbox leftover) | **Fixed** — listitem `aria-current` only |
| F-109 | High | hls.rs temp dir vs ffmpeg sidecar | HLS wrote `downloads_temp/hls-{id}` but sidecar only allows `*.falcondm-temp/*` — mux spawn denied | **Fixed** — `hls-{id}.falcondm-temp`; sweep old `hls-N` dirs |
| F-110 | Low | hls.rs ffmpeg concat list | `file '{}'` interpolated names (quote/path injection if names ever drifted) | **Fixed** — allowlist `seg_NNNNN.ts` charset |
| F-111 | Medium | ytdlp.rs cookies | `--add-header Cookie` plus `--cookies-from-browser` mixed two jars | **Fixed** — browser cookies only when no explicit Cookie |
| F-112 | Low | commands.rs remove_download | HTTP `.falcon.segments` + HLS temp survived delete | **Fixed** — remove sidecars; migrate old `hls-N` |
| F-113 | Low | LogPanel radiogroup | Only Left/Right; APG radios also use Up/Down | **Fixed** — four arrows |
| F-114 | Low | App.tsx ⌘K | Opened palette over an existing modal | **Fixed** — skip if another overlay is open |
| F-115 | Medium | background download_video / batch | `isPublicHttpUrl` missing → fail-open private enqueue | **Fixed** — `isPublicEnqueueUrl` fail-closed |
| F-116 | Low | content.js grabber | Same fail-open if FalconMedia missing | **Fixed** — require helper |
| F-117 | Low | shared.js clipboard | No FalconMedia → only scheme check (LAN queued) | **Fixed** — fail-closed |
| F-118 | Low | shared.js blob fetch | Fallback cap 100MB vs backend 32MB | **Fixed** — 32MB |
| F-119 | Medium | settings.rs DownloadProfile | Explicit origin used `port()` so `https://host:443` missed default HTTPS; `https://host` matched `:8443` | **Fixed** — explicit origins use `port_or_known_default`; host-only still any port |
| F-120 | Low | App.tsx ⌘K / Inspector Escape | F-114 only checked `.modal-overlay`; onboarding is `.wizard-overlay` | **Fixed** — wizard overlay blocks palette + inspector Escape |
| F-121 | Low | content.js trapTab | `offsetParent !== null` is null for `position:fixed` overlay descendants | **Fixed** — `getClientRects()` |
| F-122 | Low | options.html / onboarding / GeneralTab | Hint text sat outside the label with no `aria-describedby` | **Fixed** — describedby on token + checkboxes + native-host field |
| F-123 | Low | CommandPalette | Combobox missing `aria-haspopup="listbox"` | **Fixed** |
| F-124 | Low | DownloadItem / DownloadList | Enter/Space on a focused row toggled select then bubbled; list re-selected | **Fixed** — stopPropagation + list skips listitem Enter/Space |
| F-125 | Low | App.tsx clipboard / drop | F-120 only gated ⌘K; drop/clipboard opened New Download over onboarding/settings (modal z > wizard) | **Fixed** — `isForegroundDialogOpen` shared gate |
| F-126 | Low | useClipboardMonitor | Overlay skip still wrote `lastRef` so the URL never queued after close | **Fixed** — commit lastRef only when onUrl does not return false |
| F-127 | Low | SettingsModal tabs | Horizontal tablist only Left/Right; APG also allows Up/Down | **Fixed** — four arrows like LogPanel |
| F-128 | Medium | pause/resume IPC + tray | DB write had no `download-progress`; F-80 Paused sticky ignored live ticks until the 5s poll — tray resume looked paused while running | **Fixed** — emit Paused/Queued; applyProgress accepts Queued after Paused |
| F-129 | Medium | resolve_download_save_path_with | Outside-root error was `"must be under the Downloads folder"` so the `"outside allowed"` hard-fail never matched — enqueue silently saved to Downloads | **Fixed** — same `"outside allowed"` string as `validate_open_path` |
| F-130 | Low | ProfilesTab | Host-match hint sat outside the pattern field with no `aria-describedby` | **Fixed** |
| F-131 | Low | index.html / popup / options | Google Fonts CDN vs production `font-src 'self'` (dead request + Google) | **Fixed** — system stack already in CSS |
| F-132 | Medium | resume_download / tray resume | Failed retry zeroed DB size but left `.part` / HLS / Occupied dest — HTTP resumed corrupt bytes or died `already exists` | **Fixed** — `clear_failed_retry_artifacts` (Occupied only; Complete dest kept) |
| F-133 | Low | SchedulerModal | Scope hint sat outside controls with no `aria-describedby` | **Fixed** |
| F-134 | Low | GeneralTab | Extension pairing hint not tied to the control group | **Fixed** — `role=group` + describedby |
| F-135 | Low | App.tsx speed Gauge | Settings save wrote `speed_limit_kbps` but toolbar state only loaded on mount — Gauge `aria-pressed` stale; toggle restored the old cap | **Fixed** — refetch when Settings closes |
| F-136 | Medium | set_schedule / save_settings | `set_schedule` wrote disk+queue but not the Settings mutex; Settings/speed-toggle save then restored a stale `schedule_active` and could disable the live queue after restart | **Fixed** — mutex updated on set_schedule; save_settings copies allowlist from mutex and schedule from the live queue |
| F-137 | Medium | SettingsModal | Save was enabled on the empty initial snapshot; a fast click wiped profiles / path / speed | **Fixed** — first `getSettings` always hydrates; Save disabled until then |
| F-138 | Low | Toolbar Clock | Scheduler had no `active` / `aria-pressed` (unlike Gauge / clipboard) | **Fixed** — refetch `getSchedule` when the modal closes |
| F-139 | Medium | SchedulerModal | Save before `getSchedule` returned `active=false` + empty times (valid) and could turn off a live schedule | **Fixed** — same hydrate gate; load failure announced |
| F-140 | Low | Settings profiles tabpanel | Intro hint `settings-profiles-hint` was not referenced | **Fixed** — `aria-describedby` on the tabpanel |
| F-141 | Low | CommandPalette | `run` then `onClose` restored list focus after the next dialog had taken it | **Fixed** — close palette first |
| F-142 | Low | options.html / popup.html | Extension ID, pair/test, and grab hints sat next to controls with no `aria-describedby` | **Fixed** |
| F-143 | Medium | save_settings profile cookies | Empty UI cookies restored only on exact `name+url_pattern`; rename/repattern dropped the session cookie | **Fixed** — name+pattern first, then unused same index |
| F-144 | Medium | shared.js pause hydrate | `local.get({ paused: false })` could not tell Resume from missing; leftover session pause overwrote Resume after SW restart | **Fixed** — migrate session only if local key absent; Resume clears session |
| F-145 | Low | background.js `check_status` | Popup/options read pause/fail-closed before `prefsReady` (unlike `get_status`) | **Fixed** — await hydrate first |
| F-146 | Low | Settings/Onboarding native host | UI accepted any 32-char id (`zzzz…`); rust host already `[a-p]{32}` but toast was generic fail | **Fixed** — same charset in UI; command uses `parse_extension_id` |
| F-147 | Medium | queue.rs schedule/concurrency | Demotion to `Queued` wrote DB but no `download-progress`; UI stayed Downloading until poll, and a late worker tick looked live | **Fixed** — `demote_active_to_queued` emits Queued (same as pause/resume) |
| F-148 | Medium | background.js `download_blob` | Blob used `postFalcon` so a connected-but-asleep app never got wake/retry | **Fixed** — `sendToFalcon` with 120s timeout |
| F-149 | Medium | shared.js / content.js | YouTube manifest CS + `ensureContentScript` re-injected; reset only cleared `__falconDmReady` so a second IIFE stacked listeners | **Fixed** — ping first; reset also deletes `__falconDmContentScript` |
| F-150 | Low | popup.js fail-closed | Success did not update `failClosed`; error revert used the stale flag | **Fixed** — revert via checkbox; store `resp.failClosed` |
| F-151 | Low | LogPanel | `getLogs` fail toasted on every 2s auto-refresh | **Fixed** — toast once until a later refresh succeeds |
| F-152 | Low | DownloadItem redownload | Deduped `add_download` returned the same id with no toast | **Fixed** — `already_queued` info toast; IPC still returns Ok so hijack does not fail-open |
| F-153 | Low | InspectorPanel | Speed showed `-` while Merging | **Fixed** — `isLive` includes Merging |
| F-154 | Medium | pairing.js 403 | Extension-blocked 403 was then overwritten to `offline` | **Fixed** — `throwIfExtensionBlocked`; catch skips `offline` when already `blocked` |
| F-155 | Medium | api.js `postFalcon` | 403 did not set `blocked` | **Fixed** — `setState('blocked')` before throwing |
| F-156 | Low | background.js fail-closed hijack | Notify preferred a generic string over `e.message` | **Fixed** — `e.message \|\| interceptBlocked` |
| F-157 | Low | background.js `grab_page_links` | Content-script inject fail was silent | **Fixed** — notify `errorMediaUtils` |
| F-158 | Medium | background.js fail-open hijack | Successful Chrome fallback still notified `e.message` (Falcon error) | **Fixed** — success uses `appClosedFallback`; fallback catch uses `fallbackErr` |
| F-159 | Medium | downloads.ts `mergeLiveProgress` | `applyProgress` kept Paused; 5s poll could revive Downloading | **Fixed** — same Paused sticky (Queued still resumes) |
| F-161 | Low | SchedulerModal | Load fail left body `inert` with no Retry | **Fixed** — footer Retry; alert outside inert |
| F-162 | Low | SettingsModal | Same hydrate/inert trap; Save only after load | **Fixed** — footer Retry |
| F-163 | Low | ProfilesTab | Index keys reused the wrong card after middle delete | **Fixed** — stable `allocProfileUiKey` |
| F-164 | Low | CommandPalette | Empty filter + Enter called `runAction(undefined)` | **Fixed** — no-op when `filtered.length === 0` |
| F-165 | Medium | downloads.ts Merging | Stale Downloading poll/tick rewound HLS merge UI | **Fixed** — Merging sticky vs Downloading in merge + applyProgress |
| F-166 | Low | popup.js grab | Offline/pending/blocked still opened the overlay (second error) | **Fixed** — help + `auto_pair`; overlay only when connected |
| F-167 | Low | background.js `open_grabber` | `sendMessage` fail swallowed `lastError` | **Fixed** — notify on `lastError` |
| F-168 | Low | SettingsModal pending pairs | `getPendingPairs` fail was `catch(() => {})` | **Fixed** — `pair_load_failed` toast; Retry refetches |
| F-169 | Low | CommandPalette arrows | Empty list ArrowDown set `activeIndex` to `-1` | **Fixed** — ignore arrows/Home/End when no results |
| F-170 | Low | NewDownloadModal batch | Sequential `addDownload` stopped on first error; earlier URLs stayed queued with a single fail toast | **Fixed** — `Promise.allSettled` + `batch_result`; close if any succeeded |

## Residual after 2026-09-14 follow-up

| ID | Severity | Location | Finding | Rationale |
|----|----------|----------|---------|-----------|
| R-05 | Info | Toolbar / row actions | 36px / 32px targets vs WCAG 44px | Layout density; focus rings present |
| R-06 | Info | Live Chrome hijack | Unpacked extension + real download not exercised | Falcon API `127.0.0.1:14201` not listening this pass; `flow-test.mjs` mocks Chrome |
| R-07 | Info | `finish_stream_if_active` Queued | Worker that finishes after 35s cancel-wait Completes a Queued row (may be off-window) | F-65: complete dest must not restart; schedule resume needs Queued not Paused. Ceiling: dedicated hold status |

## Confidence
Fixed items verified by cargo `--lib` 123, vitest 142, `npx tsc --noEmit`, `node extension/smoke-test.mjs` / `flow-test.mjs`. Live Chrome hijack E2E not run — desktop API `127.0.0.1:14201` not listening.
