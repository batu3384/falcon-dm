# Falcon DM Hardening and UX Design

**Date:** 2026-08-09  
**Status:** Approved for implementation  
**Scope:** macOS desktop application + Chrome/Edge MV3 extension

## Goal

Make Falcon DM safe to run, predictable under failure, and professional to use
without replacing the existing Falcon DM v2 dark-glass visual language.

Success means:

- No confirmed P0 security or data-integrity finding remains.
- Download, pause, resume, cancel, move, archive, and recovery flows have
  deterministic state transitions.
- Browser capture keeps the native browser download when Falcon DM is offline,
  unpaired, or rejects a request.
- Frontend loading, empty, error, selection, modal, i18n, and accessibility
  states are explicit and testable.
- Chrome and Edge extension flows pass smoke and contract tests.
- Local frontend, Rust, extension, and CI/release checks pass.

## Delivery strategy

Work is split into independently verifiable phases. Each behavior change
follows:

1. Add a minimal failing regression test.
2. Implement the smallest root-cause fix.
3. Run focused and full verification.
4. Commit the phase as a reviewable unit.

No visual redesign outside the existing `design-system/falcon-dm-v2/MASTER.md`.
No Firefox implementation in this scope.

## Phase P0 — security and data integrity

### Pairing and local API trust

`Origin` remains a browser policy signal, not an authentication mechanism.
Pairing must gain an app-authenticated proof that a process occupying
`127.0.0.1:14201` cannot impersonate Falcon DM.

Use a Chrome/Edge native-messaging host for the pairing proof. The registered
host launches or contacts the real Falcon DM process through an OS-owned
stdio/Unix-socket channel, receives a short-lived proof for the extension's
challenge, and returns that proof to the extension. The extension includes
the proof in `POST /api/pair`; the desktop server accepts pairing only after
verifying it. The localhost API remains the authenticated data-plane API, but
never supplies the proof by trusting a browser-controlled header. Pairing
contracts remain:

- `POST /api/pair`
- approved response: `{ ok: true, token, extension_id }`
- pending response: HTTP `202` with `{ ok: false, pending: true, extension_id }`
- authenticated calls require `X-Falcon-Token` and allowlisted extension Origin

No token or pairing secret is placed in a deep-link query string.

### HLS request isolation

Validate every playlist, variant, and segment URL at the point it is fetched.
Reject loopback, private, link-local, and unsupported schemes. Disable automatic
redirect following or validate every redirect hop before following it.

Cookies are scoped to the actual request target. A cookie collected for one
host is never sent to an unrelated playlist or segment host. Add request
timeouts and bounded playlist, segment-count, response-size, and total-output
limits.

### File movement and overwrite safety

Route `move_download` through the same centralized save-path resolver used by
enqueue. Reject traversal, symlink escapes, arbitrary destinations, active
downloads, and silent destination overwrite. Permit move only from safe terminal
states and update the database only after the filesystem operation succeeds.

### Worker lifecycle and aria2 claims

Keep a stream task claim in the active-task map until its worker exits. Cancellation
must interrupt network waits and terminate the ffmpeg child, then perform one
terminal database transition. A cancelled worker must not later mark the record
completed.

Persist aria2 GID and `Downloading` status as one claim sequence. If persistence
fails, remove the newly-created aria2 job and leave the database record queued.
Completed downloads cannot be resumed into an existing file with overwrite
enabled.

### Secret and sidecar handling

Do not expose cookies in frontend download JSON. Clear cookies on every terminal
state, not only successful completion. Apply restrictive permissions to the
database and transient config files. Validate sidecar architecture and use
macOS-compatible real-path resolution. Release builds must upload both signed
and unsigned artifacts with checksums.

## Phase P1 — frontend and extension behavior

### Frontend state model

The download store owns the active filter, request sequence, loading state,
error state, retry action, selected IDs, selected download, and last-selection
invariants. Stale responses cannot replace newer filter results. Archived
polling always uses the Archived query.

Empty, loading, and failed states are distinct. Failed fetches show an inline
error and Retry CTA instead of an empty-list message.

### Status and actions

Define one status capability map for `Queued`, `Downloading`, `Merging`,
`Paused`, `Completed`, and `Failed`. Item actions, inspector, toolbar, filters,
status bar, and batch actions consume the same map.

Batch actions report partial success and failure per item. Destructive batch
actions require one confirmation dialog.

### Modal and form behavior

All dialogs, including move/rename, use `useModalA11y`: Escape, Tab trapping,
focus restoration, and Cancel-first focus. Settings and download forms track
loading and dirty state, validate numeric bounds before save, and prevent
defaults from overwriting asynchronous settings responses.

Command Palette closes after its action completes. Clipboard failures show
real error feedback.

### Extension contracts

Use per-target cookie collection for batch items. Clear media state on tab
navigation. Pairing and health checks have bounded timeouts; native browser
downloads are not held indefinitely. YouTube quality metadata must carry the
selected height/format. Batch responses report per-item results without
duplicating successful items on retry.

Keep these compatibility guarantees:

- API base: `http://127.0.0.1:14201`
- YouTube source is a watch URL, never a googlevideo CDN URL
- `format` remains the explicit yt-dlp selector field
- download enqueue occurs before native browser cancellation
- failure preserves the browser's native download

## Phase P2 — UI polish and accessibility

Keep the current Falcon dark-glass palette, dense desktop layout, blue primary,
amber CTA, Lucide icons, and existing component vocabulary.

Improve:

- small desktop window resilience without a mobile/landing-page redesign
- light-theme contrast to at least 4.5:1
- visible focus rings, pointer affordances, reduced-motion handling
- context-aware empty states with working Add CTA
- all remaining hardcoded UI strings and language attribute updates
- validated Logs/Stats payloads and recoverable error states
- responsive SpeedGraph with accessible summary
- keyboard and screen-reader semantics for list rows, tabs, and virtualized data

## Testing strategy

### Rust

Add focused tests for:

- pairing proof and token rotation
- private/redirected HLS targets and cookie host isolation
- path traversal, symlink escape, move collisions, and active-state rejection
- cancellation during segment fetch and ffmpeg merge
- duplicate worker prevention and aria2 claim rollback
- completed resume protection and concurrent database updates
- sidecar PID/config cleanup

### Frontend

Add store/component tests for:

- Archived polling and out-of-order responses
- fetch error/retry/empty separation
- selection pruning and category changes
- Merging capabilities
- modal focus behavior and dirty settings
- command palette close behavior
- batch partial failure and destructive confirmation
- clipboard rejection
- malformed Logs/Stats payloads
- responsive and accessibility-critical assertions

### Extension

Extend the smoke suite for:

- pairing timeout/fallback
- stale tab navigation
- per-item cookies
- YouTube quality transmission
- batch partial results and retry behavior
- Chrome/Edge manifest asset validation

### CI and release

CI must run frontend checks, Rust checks, cargo-deny, and extension smoke
validation. Release must provision and validate both Apple architectures,
produce checksums, and always upload an artifact: signed/notarized when Apple
credentials exist, otherwise unsigned fallback. Sidecar downloads must be
version-pinned and checksum-verified.

## Out of scope

- Firefox-specific packaging
- New cloud synchronization or telemetry
- Replacing the Falcon DM v2 design system
- Unrelated dependency upgrades
- Mobile UI or a marketing landing page
