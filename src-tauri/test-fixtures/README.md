# Test fixtures

Small synthetic media files used by the unit tests in
`src/download/{mux,dash,hls}.rs` (loaded via `CARGO_MANIFEST_DIR`). They were
generated locally for testing and checked in so tests never hit the network or
need an encoder at runtime. They contain a few seconds of synthetic
video/audio payload — no copyrighted content.

Total size is intentionally tiny (~460 KB); keep it that way.

## `hls/` — MPEG-TS segments

- `index.m3u8` — minimal VOD playlist referencing `part0..2.ts` (1 s each).
- `part0.ts`, `part1.ts`, `part2.ts` — sequential MPEG-TS segments used by the
  HLS end-to-end and muxing tests.
- `video-only.ts`, `audio-only.ts` — single-track segments for track-detection
  and muxing edge cases.
- `multi-audio.ts` — segment carrying multiple audio tracks.
- `h265.ts` — HEVC segment for codec-specific muxing paths.

## `dash/` — fragmented MP4 (DASH) segments

- `vinit.mp4`, `v1.m4s`, `v2.m4s` — video initialization segment plus two media
  segments, matching the synthetic `SegmentTemplate` MPDs built in `dash.rs`
  tests.
- `ainit.mp4`, `a0.m4s` — the audio equivalents.
- `video-frag.mp4`, `audio-frag.m4a` — fragmented MP4 samples for codec/track
  parsing.
- `video-av1-frag.mp4`, `video-vp9-frag.mp4`, `video-h265-frag.mp4`,
  `audio-opus.mp4` — codec-variant fragments (AV1, VP9, H.265, Opus).
- `video-progressive.mp4`, `video-progressive-late.mp4`,
  `audio-progressive.m4a`, `audio-progressive-late.m4a` — progressive
  (non-fragmented) MP4/M4A samples; the `late` variants exercise
  moov-at-end layouts.
