# Design System Master File — Falcon DM

> **LOGIC:** This file is the v2 source of truth. There is no `pages/` overlay
> directory. Follow the rules below for all UI work.

---

**Project:** Falcon DM v2
**Category:** File Manager & Transfer (macOS Desktop App)
**Platform:** macOS 13+ (Apple Silicon primary), Tauri v2 + WKWebView
**Design Dials:** Variance 6/10 (Balanced / Modern) | Motion 4/10 (Standard) | Density 8/10 (Dense / Dashboard)

---

## Context (NOT a mobile/web landing page)

Falcon DM is a **1100×700 desktop download manager** window. This is a utility app, not a marketing page. The design language follows **macOS Tahoe native aesthetics** (Liquid Glass, vibrancy, Raycast-style keyboard-first interactions), NOT a mobile bento grid. All spacing, breakpoints, and patterns below are tuned for a desktop window with a vibrancy sidebar, toolbar, download list, and inspector panel.

## Global Rules

### Color Palette

| Role | Hex | CSS Variable |
|------|-----|--------------|
| Primary | `#2563EB` | `--color-primary` |
| On Primary | `#FFFFFF` | `--color-on-primary` |
| Secondary | `#3B82F6` | `--color-secondary` |
| Accent/CTA | `#D97706` | `--color-accent` / `--cta` |
| Background | transparent (vibrancy) | `--bg` |
| Foreground | `rgba(0,0,0,0.88)` / `rgba(255,255,255,0.96)` | `--text-1` |
| Border | `rgba(0,0,0,0.08)` / `rgba(255,255,255,0.1)` | `--border` |
| Destructive | `#d92d20` / `#ff453a` | `--danger` |
| Ring | `#2563EB` | `--accent` |

**Color Notes:** Folder blue + file amber (CTA). Dark-mode-first. Translucent surface layers (`--surface-1/2/3`) over native macOS vibrancy (`NSVisualEffectMaterial::UnderWindowBackground`). Avoid pure `#000000` (OLED smear).

### Typography

- **Font:** Inter (weights 300-700), `--font-mono: "Geist Mono"`
- **Base size:** 13px (dense dashboard), line-height 1.45
- **Numerical text:** `tabular-nums` (speed, sizes, counts)
- **Mood:** dark, cinematic, technical, precision, clean, premium, developer, professional

### Spacing Variables (density 8/10 — dashboard)

| Token | Value | Usage |
|-------|-------|-------|
| `--space-xs` | `2px` | Tight gaps |
| `--space-sm` | `4px` | Icon gaps |
| `--space-md` | `8px` | Standard padding |
| `--space-lg` | `12px` | Section padding |
| `--space-xl` | `16px` | Large gaps |
| `--space-2xl` | `24px` | Section margins |
| `--space-3xl` | `32px` | Hero padding |

### Shadow Depths

| Token | Value | Usage |
|-------|-------|-------|
| `--shadow-sm` | `0 1px 2px rgba(0,0,0,0.05)` | Subtle lift |
| `--shadow-md` | `0 4px 6px rgba(0,0,0,0.1)` | Cards |
| `--shadow-lg` | `0 10px 15px rgba(0,0,0,0.15)` | Context menus |
| `--shadow-xl` | `0 20px 25px rgba(0,0,0,0.2)` | Modals |

---

## Component Specs (Desktop)

### Buttons (dense, 30px height)

```css
.btn-primary { background: var(--color-accent); color: #fff; height: 30px; padding: 0 14px; border-radius: 8px; font-weight: 500; }
.btn-secondary { background: var(--surface-2); border: 1px solid var(--border-strong); }
```

### Modals (centered, 480px, backdrop blur 8px)

```css
.modal-overlay { backdrop-filter: blur(8px); }
.modal-panel { border-radius: 16px; box-shadow: 0 20px 50px rgba(0,0,0,0.3); }
```

### Command Palette (⌘K, top-anchored)

Raycast-style: input at top, filtered action list below, keyboard navigation (arrows + Enter).

---

## Anti-Patterns (Do NOT Use)

- ❌ **Emojis as icons** — Use SVG (Lucide)
- ❌ **Missing cursor:pointer** on clickable elements
- ❌ **Layout-shifting hovers** — No scale transforms that shift layout
- ❌ **Low contrast text** — 4.5:1 minimum (light mode `--text-3` must differ from `--text-2`)
- ❌ **Instant state changes** — Always 150-300ms transitions
- ❌ **Invisible focus states** — `:focus-visible` ring required on ALL interactive elements including list items
- ❌ **`aria-live` on rapidly-changing values** — StatusBar speed changes every tick; use separate hidden live region
- ❌ **Default-focus on destructive actions** — ConfirmDialog must focus Cancel, not Confirm

## Pre-Delivery Checklist

- [ ] No emojis as icons (Lucide SVG)
- [ ] `cursor-pointer` on all clickable elements
- [ ] Hover states 150-300ms
- [ ] Light mode: text contrast ≥ 4.5:1 (`--text-3` must be lighter than `--text-2`)
- [ ] `:focus-visible` on ALL interactive elements (including `.dl-item`)
- [ ] `prefers-reduced-motion` respected
- [ ] No horizontal scroll
- [ ] All modal a11y: Escape/Tab trap/focus restore via `useModalA11y`
- [ ] i18n: no hardcoded strings — all via `t()` / `chrome.i18n.getMessage`

## Loading & Empty States (Raycast/Linear discipline)

### Skeleton Loading (first paint)
- **Never** show a blank list or a frozen spinner on initial load. Use **shimmer skeletons** — grey placeholder rows (`--surface-3`) with a slow `shimmer` sweep (1.8s, `--ease-out`).
- Skeleton rows mirror the real `.dl-item` geometry: thumb (28×28), two text bars (60% + 35% width), a thin progress bar.
- **Phase:** `loading=true` → skeletons (5 rows) → first data arrives → `loading=false`. On refresh (not initial), keep showing stale data (no skeleton flash).

### Rich Empty States
Every empty state has: **ikon (Lucide, 28px, `--text-3`)** + **başlık** + **açıklama** + **CTA (varsa)**. Vary by context:
- *No downloads at all* → `Inbox` icon + "No downloads yet" + "Add your first download" CTA.
- *Category empty (e.g. Video)* → category-specific icon + "{Category} is empty" + hint.
- *Search no results* → `SearchX` icon + "No matches for '{query}'" + "Try a different term".
- *Failed-only view* → `AlertTriangle` + "No failed downloads" (positive framing).

## Micro-interactions

- **`cursor: pointer`** on every `button`, `.dl-item`, `[role="tab"]`, `.cmd-item`, clickable row.
- **`:active` press feedback** — `transform: scale(0.97)` + 80ms transition on buttons/icon-btns (conveys tactility). List items use `background` shift instead (scale would reflow).
- **`:focus-visible` ring** — `box-shadow: 0 0 0 2px var(--surface-2), 0 0 0 4px var(--accent)` (offset ring readable on vibrancy). Never `outline: none` without replacement.
- **Hover lift** — cards/panels gain `border-color` shift to `--border-strong` + subtle `translateY(-1px)` on hover (150ms). Avoid layout-affecting scale on dense rows.

## Statistics Panel

A modal/overlay accessible from the Toolbar (BarChart icon) + Command Palette. Layout:
- **4 KPI cards** in a row: Active / Queued / Completed / Failed — each with count + small icon + accent dot.
- **Totals row**: Total downloaded (bytes, `formatBytes`), average speed, session uptime.
- **Speed sparkline** — reuse `SpeedGraph` at larger size (full width, 60s window).
- Cards use `--surface-1` + hairline border; numbers use `tabular-nums` + `--font-mono`.
