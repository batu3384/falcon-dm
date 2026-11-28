# Falcon DM Wiki

Welcome to the **Falcon DM** documentation wiki.

Falcon DM is a local-only macOS download manager for HTTP(S), HLS, and YouTube — built with Tauri 2 (Rust) and React/TypeScript. No telemetry, no cloud queue.

**Highlights:** multi-connection HTTP with byte-exact resume, optional checksum verification, HLS segment resume, fully native YouTube/DASH downloads with quality selection, batch URL import, expired-link refresh, weekly scheduler and completion actions, extension capture rules, one-shot 6-digit code pairing, GitHub release update check.

## Quick links

| Page | Description |
|------|-------------|
| [Getting Started](Getting-Started) | Install, build, first run |
| [Architecture](Architecture) | Download engine, queue, API |
| [Browser Extension](Browser-Extension) | MV3 extension, capture rules, pairing |
| [CLI](CLI) | `falconctl` command line and automation API |
| [Security](Security) | Threat model and hardening |
| [Troubleshooting](Troubleshooting) | Common issues |
| [FAQ](FAQ) | Short answers |
| [Roadmap](Roadmap) | Now / Next / Later milestones |

## Repository

- **Source:** [github.com/batu3384/falcon-dm](https://github.com/batu3384/falcon-dm)
- **Issues:** use GitHub Issues for bugs and features
- **Security:** see [SECURITY.md](https://github.com/batu3384/falcon-dm/blob/main/SECURITY.md) — do not file public issues for vulnerabilities

## Platforms

macOS 13+ (Ventura or later), Intel and Apple Silicon. Browser extension targets Chrome and Edge (Chromium).
