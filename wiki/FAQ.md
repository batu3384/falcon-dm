# FAQ

## Is Falcon DM IDM for macOS?

It targets similar jobs (multi-connection HTTP, browser capture, queue) but is a smaller local-first app — not feature parity with IDM.

## Does it phone home?

No telemetry. Queue and settings stay on your machine.

## Which browsers are supported?

Chromium: Chrome and Edge via the bundled MV3 extension.

## Can I use it without the extension?

Yes. Add URLs manually or via the local API with a valid token.

## Where are files saved?

Default: `~/Downloads` (configurable in Settings). Category subfolders optional.

## Why aria2 if HTTP is native Rust?

aria2 remains for legacy session recovery and edge compatibility; primary HTTP path is Rust.

## How do I update?

Pull latest `main` or install a new release build from GitHub Releases when available.
