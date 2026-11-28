//! Subtitle extraction, conversion, and HLS rendition helpers.
//!
//! YouTube timedtext tracks are fetched as WebVTT and written as SRT next to
//! the finished video. HLS subtitle renditions are segmented WebVTT streams;
//! the pure playlist/concatenation helpers in this module are intentionally
//! independent of `hls.rs` so the orchestration layer can wire them in after
//! the media engine work lands.
//!
//! YouTube orchestration contract: run `youtube::extract`, download the chosen
//! media exactly as the orchestration layer already does, then after the final
//! filename is known call [`write_selected_subtitles_for_extraction`] with the
//! row's `subtitle_tracks` (or [`write_subtitles_for_extraction`] when there
//! is no row choice). Pass the finished file stem as `base_filename` and log
//! individual track failures without reclassifying the completed download.
//!
//! HLS orchestration contract (no `hls.rs` edit is made here):
//! 1. In the master-playlist arm of HLS plan resolution, keep the selected
//!    variant's `subtitles` group id and call [`hls_subtitle_renditions`]
//!    before the master playlist is dropped. The returned rendition objects
//!    expose language/name plus DEFAULT, AUTOSELECT and FORCED so the
//!    orchestrator can apply the product's selection policy.
//! 2. While media segments download, call
//!    [`fetch_hls_subtitle_rendition_segments`] with the original source URL,
//!    the final master-playlist URL, and the rendition. It resolves the media
//!    playlist, performs bounded validated/pinned fetches, and returns ordered
//!    [`HlsVttSegment`] bodies. The lower-level [`parse_hls_subtitle_media_playlist`]
//!    and [`hls_subtitle_segment_plan`] functions stay public for orchestrators
//!    that already have parsed playlist data.
//! 3. Keep subtitle errors out of the media download's fatal path; retry is
//!    already bounded inside the helper, and a permanent subtitle failure is
//!    not a failed video.
//! 4. After the video file has finalized, use [`write_hls_subtitle_rendition`]
//!    (or the lower-level concatenate/write pair) with the finalized file stem.
//!    Subtitle failure must be logged, never turn a completed video download
//!    into a failure.

use crate::download::http_client::{open, HttpHeaders, RangeRequest};
use crate::download::job::JobContext;
use crate::download::limiter::{cancelled, sleep_or_cancel};
use crate::download::youtube::{CaptionTrack, ExtractError, ValidatedFetcher, YoutubeExtraction};
use crate::download::DownloadError;
use crate::storage::models::SubtitleTrackSelection;
use crate::util::{
    move_file_unique, sanitize_filename, validate_fetch_url_async, write_file_exclusive,
};
use futures::{stream, StreamExt};
use m3u8_rs::{AlternativeMediaType, MasterPlaylist, MediaPlaylist, Playlist};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

const MAX_SUBTITLE_TRACKS: usize = 32;
const MAX_HLS_SUBTITLE_PLAYLIST_BYTES: usize = 2 * 1024 * 1024;
const MAX_HLS_SUBTITLE_SEGMENT_BYTES: usize = 8 * 1024 * 1024;
const MAX_HLS_SUBTITLE_SEGMENTS: usize = 20_000;
const MPEGTS_WRAP_TICKS: i128 = 1_i128 << 33;
const MPEGTS_TICKS_PER_MS: i128 = 90;
const MAX_MAPPED_SPAN_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubtitleError {
    InvalidUrl(String),
    Network(String),
    Http(u16),
    MalformedVtt(&'static str),
    Io(String),
}

impl std::fmt::Display for SubtitleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl(message) | Self::Network(message) | Self::Io(message) => {
                f.write_str(message)
            }
            Self::Http(status) => write!(f, "HTTP {status}"),
            Self::MalformedVtt(what) => write!(f, "malformed WebVTT: {what}"),
        }
    }
}

impl std::error::Error for SubtitleError {}

impl From<ExtractError> for SubtitleError {
    fn from(error: ExtractError) -> Self {
        Self::Network(error.to_string())
    }
}

pub type SubtitleResult<T> = Result<T, SubtitleError>;

fn canonical_language(raw: &str) -> String {
    raw.trim().replace('_', "-").to_ascii_lowercase()
}

fn languages_overlap(left: &str, right: &str) -> bool {
    let left = canonical_language(left);
    let right = canonical_language(right);
    !left.is_empty()
        && !right.is_empty()
        && (left == right
            || left.strip_prefix(&right).is_some_and(|rest| rest.starts_with('-'))
            || right.strip_prefix(&left).is_some_and(|rest| rest.starts_with('-')))
}

fn track_is_automatic(track: &CaptionTrack) -> bool {
    track.is_automatic || track.kind.as_deref().is_some_and(|kind| kind.eq_ignore_ascii_case("asr"))
}

/// Default YouTube track choice: every manual track, plus one ASR track when
/// the ASR language has no manual counterpart. `preferred_langs` only orders
/// ASR candidates (the first is normally the original audio language); manual
/// tracks are never filtered out in v1.
pub fn select_tracks(captions: &[CaptionTrack], preferred_langs: &[String]) -> Vec<CaptionTrack> {
    let mut manual = Vec::new();
    let mut manual_seen = HashSet::new();
    let mut automatic = Vec::new();
    for track in captions {
        if track.base_url.trim().is_empty() || track.language_code.trim().is_empty() {
            continue;
        }
        if track_is_automatic(track) {
            automatic.push(track.clone());
        } else {
            let key = canonical_language(&track.language_code);
            if manual_seen.insert(key) {
                manual.push(track.clone());
            }
        }
    }

    let mut ordered_asr = Vec::new();
    let mut used = HashSet::new();
    for preferred in preferred_langs {
        for (index, track) in automatic.iter().enumerate() {
            if !used.contains(&index) && languages_overlap(preferred, &track.language_code) {
                used.insert(index);
                ordered_asr.push(track.clone());
            }
        }
    }
    for (index, track) in automatic.iter().enumerate() {
        if !used.contains(&index) {
            ordered_asr.push(track.clone());
        }
    }

    let mut out = manual.clone();
    if let Some(track) = ordered_asr.into_iter().find(|asr| {
        !manual
            .iter()
            .any(|manual_track| languages_overlap(&manual_track.language_code, &asr.language_code))
    }) {
        out.push(track);
    }
    out
}

fn selection_kind_matches(selected: Option<&str>, track: &CaptionTrack) -> bool {
    match selected.map(str::trim).filter(|kind| !kind.is_empty()) {
        Some(kind) => {
            track.kind.as_deref().is_some_and(|actual| actual.eq_ignore_ascii_case(kind))
                || (kind.eq_ignore_ascii_case("asr") && track_is_automatic(track))
        }
        None => !track_is_automatic(track),
    }
}

/// Tracks explicitly chosen in the browser overlay, in the extraction's
/// original order. `None` in the selection kind means the manual track.
pub fn tracks_matching_selection(
    captions: &[CaptionTrack],
    selection: &[SubtitleTrackSelection],
) -> Vec<CaptionTrack> {
    captions
        .iter()
        .filter(|track| {
            selection.iter().any(|selected| {
                languages_overlap(&selected.lang, &track.language_code)
                    && selection_kind_matches(selected.kind.as_deref(), track)
            })
        })
        .cloned()
        .collect()
}

/// Normalize extension-supplied subtitle choices before they reach SQLite.
/// Invalid entries are dropped; duplicates collapse. The empty Vec is kept by
/// the caller as an explicit "download no subtitles" choice.
pub fn normalize_subtitle_selection(
    selection: &[SubtitleTrackSelection],
) -> Vec<SubtitleTrackSelection> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for track in selection.iter().take(MAX_SUBTITLE_TRACKS) {
        let lang = canonical_language(&track.lang);
        if lang.len() > 35
            || lang.is_empty()
            || !lang.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            continue;
        }
        let kind = track
            .kind
            .as_deref()
            .map(str::trim)
            .filter(|kind| !kind.is_empty())
            .map(str::to_ascii_lowercase)
            .filter(|kind| {
                kind.len() <= 16
                    && kind.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            });
        let key = format!("{lang}|{}", kind.as_deref().unwrap_or(""));
        if seen.insert(key) {
            out.push(SubtitleTrackSelection { lang, kind });
        }
    }
    out
}

/// Replace every exact `fmt` parameter value with `vtt` without decoding or
/// re-encoding anything else in the query. Returns `None` when no exact
/// byte-level `fmt` key exists. Value bytes are the only original bytes dropped;
/// signed parameters such as percent-encoded `sig` values stay bit-for-bit intact.
fn replace_fmt_values_preserving_query(query: &str) -> Option<String> {
    let bytes = query.as_bytes();
    let mut rewritten = Vec::with_capacity(query.len());
    let mut copied = 0usize;
    let mut position = 0usize;
    let mut replaced = false;
    loop {
        let end =
            query[position..].find('&').map(|relative| position + relative).unwrap_or(query.len());
        let equals = query[position..end].find('=').map(|relative| position + relative);
        let key_end = equals.unwrap_or(end);
        if bytes[position..key_end] == *b"fmt" {
            let value_start = equals.map(|index| index + 1).unwrap_or(end);
            rewritten.extend_from_slice(&bytes[copied..value_start]);
            if equals.is_none() {
                rewritten.push(b'=');
            }
            rewritten.extend_from_slice(b"vtt");
            copied = end;
            replaced = true;
        }
        if end == query.len() {
            break;
        }
        position = end + 1;
    }
    if !replaced {
        return None;
    }
    rewritten.extend_from_slice(&bytes[copied..]);
    // ASCII delimiter positions are UTF-8 boundaries, and every copied byte
    // comes from the original valid string; the inserted bytes are ASCII.
    Some(String::from_utf8(rewritten).expect("query rewrite preserves UTF-8"))
}

/// Replace (never normalize) the timedtext `fmt` parameter with `vtt`.
///
/// YouTube signs timedtext URLs. `Url::query_pairs_mut()` decodes and then
/// re-encodes every parameter, which can turn byte-distinct but semantically
/// equal encodings into a signature mismatch. Validation therefore uses `Url`,
/// while the returned string is assembled from the original bytes.
pub fn timedtext_vtt_url(base_url: &str) -> SubtitleResult<String> {
    let original = base_url.trim();
    let url = Url::parse(original)
        .map_err(|error| SubtitleError::InvalidUrl(format!("invalid timedtext URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(SubtitleError::InvalidUrl("timedtext URL must be HTTP(S)".into()));
    }

    let end_before_fragment = original.find('#').unwrap_or(original.len());
    let query_start = original[..end_before_fragment].find('?').map(|index| index + 1);
    let mut out = String::with_capacity(original.len() + "&fmt=vtt".len());
    if let Some(query_start) = query_start {
        let query_end = end_before_fragment;
        let query = &original[query_start..query_end];
        out.push_str(&original[..query_start]);
        if let Some(rewritten) = replace_fmt_values_preserving_query(query) {
            out.push_str(&rewritten);
        } else {
            out.push_str(query);
            if !query.is_empty() && !query.ends_with('&') {
                out.push('&');
            }
            out.push_str("fmt=vtt");
        }
        out.push_str(&original[query_end..]);
    } else {
        out.push_str(&original[..end_before_fragment]);
        out.push_str("?fmt=vtt");
        out.push_str(&original[end_before_fragment..]);
    }
    Ok(out)
}

fn subtitle_filename(base_filename: &str, language: &str, extension: &str) -> String {
    let base = sanitize_filename(base_filename).chars().take(150).collect::<String>();
    let language =
        sanitize_filename(&canonical_language(language)).chars().take(40).collect::<String>();
    let language = if language.is_empty() { "und".into() } else { language };
    format!("{base}.{language}.{extension}")
}

fn write_unique_text(out_dir: &Path, filename: &str, text: &str) -> SubtitleResult<PathBuf> {
    std::fs::create_dir_all(out_dir).map_err(|error| {
        SubtitleError::Io(format!("could not create subtitle directory: {error}"))
    })?;
    let filename = sanitize_filename(filename);
    let dir = out_dir
        .to_str()
        .ok_or_else(|| SubtitleError::Io("subtitle save path is not valid UTF-8".into()))?;
    let destination =
        crate::util::resolve_download_target(dir, &filename).map_err(SubtitleError::Io)?;
    let temp = out_dir.join(format!(".{filename}.{}.tmp", uuid::Uuid::new_v4()));
    write_file_exclusive(&temp, text.as_bytes()).map_err(|error| {
        SubtitleError::Io(format!("could not write subtitle temp file: {error}"))
    })?;
    match move_file_unique(&temp, &destination, &HashSet::new()) {
        Ok(path) => Ok(path),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(SubtitleError::Io(format!("could not place subtitle file: {error}")))
        }
    }
}

fn parse_vtt_timestamp(raw: &str) -> Option<u64> {
    let (time, millis) = raw.trim().split_once('.')?;
    if millis.len() != 3 || !millis.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let parts: Vec<&str> = time.split(':').collect();
    if parts.len() != 2 && parts.len() != 3 {
        return None;
    }
    let (hours, minutes, seconds) =
        if parts.len() == 3 { (parts[0], parts[1], parts[2]) } else { ("0", parts[0], parts[1]) };
    if hours.is_empty()
        || !hours.bytes().all(|byte| byte.is_ascii_digit())
        || minutes.len() != 2
        || seconds.len() != 2
        || !minutes.bytes().all(|byte| byte.is_ascii_digit())
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hours: u64 = hours.parse().ok()?;
    let minutes: u64 = minutes.parse().ok()?;
    let seconds: u64 = seconds.parse().ok()?;
    if minutes >= 60 || seconds >= 60 {
        return None;
    }
    let millis: u64 = millis.parse().ok()?;
    Some(hours.checked_mul(3_600_000)? + minutes * 60_000 + seconds * 1000 + millis)
}

fn parse_timing_line(line: &str) -> Option<(u64, u64)> {
    let mut parts = line.split_whitespace();
    let start = parse_vtt_timestamp(parts.next()?)?;
    if parts.next()? != "-->" {
        return None;
    }
    let end = parse_vtt_timestamp(parts.next()?)?;
    Some((start, end))
}

fn format_timestamp(ms: u64, decimal: char) -> String {
    let hours = ms / 3_600_000;
    let minutes = (ms / 60_000) % 60;
    let seconds = (ms / 1000) % 60;
    let millis = ms % 1000;
    format!("{hours:02}:{minutes:02}:{seconds:02}{decimal}{millis:03}")
}

fn decode_entity(entity: &str) -> Option<String> {
    match entity {
        "amp" => Some("&".into()),
        "lt" => Some("<".into()),
        "gt" => Some(">".into()),
        "quot" => Some("\"".into()),
        "apos" => Some('\''.into()),
        "nbsp" => Some(" ".into()),
        _ => {
            let numeric = entity.strip_prefix('#')?;
            let value = if let Some(hex) = numeric.strip_prefix(['x', 'X']) {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                numeric.parse::<u32>().ok()?
            };
            char::from_u32(value).map(|ch| ch.to_string())
        }
    }
}

fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let Some(end) = after.find(';').filter(|end| *end <= 32) else {
            out.push('&');
            rest = after;
            continue;
        };
        let entity = &after[..end];
        if let Some(decoded) = decode_entity(entity) {
            out.push_str(&decoded);
        } else {
            out.push('&');
            out.push_str(entity);
            out.push(';');
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn strip_inline_timestamp_tags(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let tag = &after[..end];
        if parse_vtt_timestamp(tag).is_none() {
            out.push_str(&rest[start..start + end + 2]);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn strip_vtt_tags(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn clean_vtt_line(line: &str) -> String {
    decode_entities(&strip_vtt_tags(line)).split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct VttCue {
    start_ms: u64,
    end_ms: u64,
    /// Entity-decoded, tag-free lines used for SRT and dedupe.
    lines: Vec<String>,
    /// Lines with only per-word timestamp tags removed; formatting tags stay
    /// intact for the concatenated HLS WebVTT output.
    vtt_lines: Vec<String>,
}

fn parse_cue_block(lines: &[&str], cues: &mut Vec<VttCue>) {
    if lines.is_empty() {
        return;
    }
    let first = lines[0].trim();
    if first.starts_with("WEBVTT")
        || first.starts_with("NOTE")
        || first == "STYLE"
        || first == "REGION"
    {
        return;
    }
    let Some(timing_index) = lines.iter().position(|line| line.contains("-->")) else {
        return;
    };
    if timing_index > 2 {
        return;
    }
    let Some((start_ms, end_ms)) = parse_timing_line(lines[timing_index]) else {
        return;
    };
    if end_ms <= start_ms {
        return;
    }
    let mut clean_lines = Vec::new();
    let mut vtt_lines = Vec::new();
    for line in &lines[timing_index + 1..] {
        let for_vtt = strip_inline_timestamp_tags(line).trim().to_string();
        if !for_vtt.is_empty() {
            vtt_lines.push(for_vtt);
        }
        let clean = clean_vtt_line(line);
        if !clean.is_empty() {
            clean_lines.push(clean);
        }
    }
    if clean_lines.is_empty() || vtt_lines.is_empty() {
        return;
    }
    cues.push(VttCue { start_ms, end_ms, lines: clean_lines, vtt_lines });
}

fn parse_vtt(input: &str) -> SubtitleResult<Vec<VttCue>> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let first = input.lines().map(str::trim).find(|line| !line.is_empty());
    if !first.is_some_and(|line| line.starts_with("WEBVTT")) {
        return Err(SubtitleError::MalformedVtt("missing WEBVTT header"));
    }
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    let mut block = Vec::new();
    for line in normalized.lines() {
        if line.trim().is_empty() {
            parse_cue_block(&block, &mut cues);
            block.clear();
        } else {
            block.push(line);
        }
    }
    parse_cue_block(&block, &mut cues);
    Ok(cues)
}

fn leading_line_overlap(previous: &[String], current: &[String]) -> usize {
    let max = previous.len().min(current.len());
    (1..=max).rev().find(|len| previous[previous.len() - len..] == current[..*len]).unwrap_or(0)
}

fn words(lines: &[String]) -> Vec<String> {
    lines.iter().flat_map(|line| line.split_whitespace().map(str::to_string)).collect()
}

fn word_overlap(previous: &[String], current: &[String]) -> usize {
    let previous = words(previous);
    let current = words(current);
    let max = previous.len().min(current.len());
    (1..=max).rev().find(|len| previous[previous.len() - len..] == current[..*len]).unwrap_or(0)
}

fn drop_prefix_words(lines: &[String], mut count: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in lines {
        let words: Vec<&str> = line.split_whitespace().collect();
        if count >= words.len() {
            count -= words.len();
            continue;
        }
        let kept = words[count..].join(" ");
        count = 0;
        if !kept.is_empty() {
            out.push(kept);
        }
    }
    out
}

fn remove_inline_duplicate_lines(cue: &mut VttCue) {
    let mut lines = Vec::new();
    let mut vtt_lines = Vec::new();
    for (index, line) in cue.lines.iter().enumerate() {
        if lines.last() == Some(line) {
            continue;
        }
        lines.push(line.clone());
        if let Some(raw) = cue.vtt_lines.get(index) {
            vtt_lines.push(raw.clone());
        }
    }
    cue.lines = lines;
    if !vtt_lines.is_empty() {
        cue.vtt_lines = vtt_lines;
    }
}

/// Flatten YouTube's rolling ASR window. Each new cue repeats the previous
/// visible line before adding a line underneath; the 10 ms flush cues repeat
/// everything. Compare against the previous *raw* cue, not only emitted cues,
/// because those flush cues are part of the renderer state.
fn dedupe_asr_cues(cues: Vec<VttCue>) -> Vec<VttCue> {
    let mut out = Vec::new();
    let mut previous_lines: Vec<String> = Vec::new();
    let mut previous_end = 0u64;
    for mut cue in cues {
        let original_lines = cue.lines.clone();
        let line_overlap = leading_line_overlap(&previous_lines, &cue.lines);
        if line_overlap > 0 {
            cue.lines.drain(..line_overlap);
            cue.vtt_lines.drain(..line_overlap.min(cue.vtt_lines.len()));
        } else if cue.start_ms <= previous_end.saturating_add(100) {
            let overlap = word_overlap(&previous_lines, &cue.lines);
            if overlap > 0 {
                cue.lines = drop_prefix_words(&cue.lines, overlap);
                // Tag positions no longer map cleanly after a word-level cut.
                // ASR output is plain text anyway, so the clean lines are the VTT lines.
                cue.vtt_lines = cue.lines.clone();
            }
        }
        remove_inline_duplicate_lines(&mut cue);
        if !cue.lines.is_empty() {
            out.push(cue.clone());
        }
        previous_lines = original_lines;
        previous_end = cue.end_ms;
    }
    out
}

/// Convert WebVTT to SRT. Cue identifiers/settings are discarded and cues are
/// renumbered. When `automatic` is true, YouTube rolling-caption overlaps are
/// flattened before rendering.
pub fn vtt_to_srt(input: &str, automatic: bool) -> SubtitleResult<String> {
    let cues = parse_vtt(input)?;
    let cues = if automatic { dedupe_asr_cues(cues) } else { cues };
    let mut out = String::new();
    for (index, cue) in cues.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            index + 1,
            format_timestamp(cue.start_ms, ','),
            format_timestamp(cue.end_ms, ','),
            cue.lines.join("\n")
        ));
    }
    Ok(out)
}

fn render_vtt(cues: &[VttCue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for cue in cues {
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            format_timestamp(cue.start_ms, '.'),
            format_timestamp(cue.end_ms, '.'),
            cue.vtt_lines.join("\n")
        ));
    }
    out
}

fn parse_timestamp_map(body: &str) -> Option<(u64, u64)> {
    for line in body.lines().take(32) {
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        let Some(rest) = line.strip_prefix("X-TIMESTAMP-MAP=") else {
            continue;
        };
        let mut local = None;
        let mut mpegts = None;
        for part in rest.split(',') {
            let Some((key, value)) = part.trim().split_once(':') else {
                continue;
            };
            if key == "LOCAL" {
                local = parse_vtt_timestamp(value);
            } else if key == "MPEGTS" {
                mpegts = value.parse::<u64>().ok();
            }
        }
        if let (Some(local), Some(mpegts)) = (local, mpegts) {
            return Some((local, mpegts));
        }
    }
    None
}

fn normalized_ticks(value: i128) -> i128 {
    value.rem_euclid(MPEGTS_WRAP_TICKS)
}

fn forward_tick_distance(from: i128, to: i128) -> i128 {
    (normalized_ticks(to) - normalized_ticks(from)).rem_euclid(MPEGTS_WRAP_TICKS)
}

/// One downloaded WebVTT segment and its EXTINF duration in milliseconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlsVttSegment {
    pub duration_ms: u64,
    pub body: String,
}

/// Concatenate segmented HLS WebVTT into one media-relative WebVTT document.
///
/// X-TIMESTAMP-MAP is interpreted as a mapping from segment-local cue time to
/// the MPEGTS clock. The first mapped segment establishes the output zero;
/// later MPEGTS offsets (including 33-bit wraparound) move cues forward. If a
/// segment lacks the map or the mapped clock jumps backwards/discontinuously,
/// the EXTINF-cumulative position is the conservative append-only fallback.
pub fn concatenate_hls_vtt_segments(segments: &[HlsVttSegment]) -> SubtitleResult<String> {
    let mut cues_out = Vec::new();
    let mut positions = Vec::with_capacity(segments.len());
    let mut base_offset_ticks = None;
    let mut next_fallback_ms = 0u64;

    for segment in segments {
        let fallback_position = next_fallback_ms;
        let map_offset_ticks = parse_timestamp_map(&segment.body).map(|(local_ms, mpegts)| {
            normalized_ticks(i128::from(mpegts) - i128::from(local_ms) * MPEGTS_TICKS_PER_MS)
        });
        let mapped_position = map_offset_ticks.and_then(|offset| {
            let base = match base_offset_ticks {
                Some(base) => base,
                None => {
                    // Missing maps in earlier segments are presumed append-contiguous.
                    let base = normalized_ticks(
                        offset - i128::from(fallback_position) * MPEGTS_TICKS_PER_MS,
                    );
                    base_offset_ticks = Some(base);
                    base
                }
            };
            let delta_ms = forward_tick_distance(base, offset) / MPEGTS_TICKS_PER_MS;
            u64::try_from(delta_ms).ok().filter(|ms| *ms <= MAX_MAPPED_SPAN_MS)
        });
        let mut position = mapped_position.unwrap_or(fallback_position);
        if let Some(previous) = positions.last().copied() {
            if position < previous {
                if previous - position <= 250 {
                    position = previous;
                } else {
                    position = fallback_position.max(previous);
                }
            }
        }
        positions.push(position);

        let mut cues = parse_vtt(&segment.body)?;
        for cue in &mut cues {
            cue.start_ms = cue.start_ms.saturating_add(position);
            cue.end_ms = cue.end_ms.saturating_add(position);
        }
        cues_out.extend(cues);
        next_fallback_ms = position.saturating_add(segment.duration_ms);
    }
    Ok(render_vtt(&cues_out))
}

/// A TYPE=SUBTITLES rendition discovered in a parsed HLS master playlist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlsSubtitleRendition {
    pub group_id: String,
    pub language: Option<String>,
    pub name: String,
    pub uri: String,
    pub is_default: bool,
    pub is_autoselect: bool,
    pub is_forced: bool,
}

/// List subtitle renditions. `subtitles_group` should be the selected video
/// variant's SUBTITLES attribute; pass None to list every subtitle group.
pub fn hls_subtitle_renditions(
    master: &MasterPlaylist,
    subtitles_group: Option<&str>,
) -> Vec<HlsSubtitleRendition> {
    master
        .alternatives
        .iter()
        .filter(|media| media.media_type == AlternativeMediaType::Subtitles)
        .filter(|media| subtitles_group.is_none_or(|group| media.group_id == group))
        .filter_map(|media| {
            let uri = media.uri.as_deref()?.trim();
            if uri.is_empty() {
                return None;
            }
            Some(HlsSubtitleRendition {
                group_id: media.group_id.clone(),
                language: media.language.clone(),
                name: media.name.clone(),
                uri: uri.to_string(),
                is_default: media.default,
                is_autoselect: media.autoselect,
                is_forced: media.forced,
            })
        })
        .collect()
}

/// A resolved subtitle media segment from a parsed media playlist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlsSubtitleSegmentPlan {
    pub url: Url,
    pub duration_ms: u64,
    /// Inclusive byte range when EXT-X-BYTERANGE is present. Offsets missing
    /// from the playlist are resolved against the previous range for that URI,
    /// as RFC 8216 requires.
    pub byte_range: Option<(u64, u64)>,
}

/// Resolve the segment URLs and durations of an HLS subtitle media playlist.
pub fn hls_subtitle_segment_plan(
    playlist: &MediaPlaylist,
    base_url: &Url,
) -> SubtitleResult<Vec<HlsSubtitleSegmentPlan>> {
    if playlist.segments.len() > 100_000 {
        return Err(SubtitleError::InvalidUrl("too many HLS subtitle segments".into()));
    }
    let mut out = Vec::with_capacity(playlist.segments.len());
    let mut previous_range: Option<(String, u64)> = None;
    for segment in &playlist.segments {
        let url = base_url.join(&segment.uri).map_err(|error| {
            SubtitleError::InvalidUrl(format!("invalid subtitle segment URL: {error}"))
        })?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(SubtitleError::InvalidUrl(
                "unsupported subtitle segment URL scheme".into(),
            ));
        }
        if segment.key.as_ref().is_some_and(|key| key.method != m3u8_rs::KeyMethod::None)
            || segment.map.is_some()
        {
            return Err(SubtitleError::InvalidUrl(
                "encrypted or externally initialized subtitle segments are unsupported".into(),
            ));
        }
        let byte_range = if let Some(range) = &segment.byte_range {
            if range.length == 0 || range.length > MAX_HLS_SUBTITLE_SEGMENT_BYTES as u64 {
                return Err(SubtitleError::InvalidUrl(
                    "invalid subtitle segment byte range".into(),
                ));
            }
            let start = match range.offset {
                Some(offset) => offset,
                None => {
                    let (previous_url, next) =
                        previous_range.as_ref().ok_or(SubtitleError::InvalidUrl(
                            "subtitle byte range is missing its first offset".into(),
                        ))?;
                    if previous_url != url.as_str() {
                        return Err(SubtitleError::InvalidUrl(
                            "implicit subtitle byte range changed resources".into(),
                        ));
                    }
                    *next
                }
            };
            let end = start
                .checked_add(range.length - 1)
                .ok_or(SubtitleError::InvalidUrl("subtitle byte range overflows".into()))?;
            previous_range = Some((url.as_str().to_string(), end + 1));
            Some((start, end))
        } else {
            previous_range = None;
            None
        };
        let duration_ms = if segment.duration.is_sign_positive() {
            (f64::from(segment.duration) * 1000.0).round() as u64
        } else {
            0
        };
        out.push(HlsSubtitleSegmentPlan { url, duration_ms, byte_range });
    }
    Ok(out)
}

/// Parse bytes that must be a media playlist, not another master playlist.
pub fn parse_hls_subtitle_media_playlist(bytes: &[u8]) -> SubtitleResult<MediaPlaylist> {
    // KEY and MAP are stateful playlist tags. Scan the raw playlist as well as
    // the parsed segments so an encrypted representation can never be mistaken
    // for malformed UTF-8 ciphertext.
    if let Ok(text) = std::str::from_utf8(bytes) {
        for line in text.lines().map(str::trim) {
            let plaintext_key = line.strip_prefix("#EXT-X-KEY:").is_some_and(|attributes| {
                attributes.split(',').any(|part| part.trim() == "METHOD=NONE")
            });
            if line.starts_with("#EXT-X-MAP:")
                || (line.starts_with("#EXT-X-KEY:") && !plaintext_key)
            {
                return Err(SubtitleError::InvalidUrl(
                    "encrypted or externally initialized subtitle playlists are unsupported".into(),
                ));
            }
        }
    }
    match m3u8_rs::parse_playlist_res(bytes) {
        Ok(Playlist::MediaPlaylist(playlist)) => Ok(playlist),
        Ok(Playlist::MasterPlaylist(_)) => {
            Err(SubtitleError::InvalidUrl("subtitle rendition pointed at a master playlist".into()))
        }
        Err(_) => Err(SubtitleError::InvalidUrl("could not parse subtitle media playlist".into())),
    }
}

async fn fetch_hls_subtitle_bytes_once(
    ctx: &JobContext,
    source: &Url,
    target: &Url,
    headers: &HttpHeaders,
    max_bytes: usize,
    what: &str,
    byte_range: Option<(u64, u64)>,
) -> Result<(Vec<u8>, Url), DownloadError> {
    let mut cancel = ctx.cancel.clone();
    let work = async {
        let range = byte_range
            .map(|(start, end)| RangeRequest {
                start: Some(start),
                end: Some(end),
                ..Default::default()
            })
            .unwrap_or_default();
        let opened = open(source, target, headers, &range, None).await?;
        let final_url = opened.final_url.clone();
        let status = opened.response.status();
        if !status.is_success() {
            return Err(DownloadError::from_status(status.as_u16()));
        }
        if byte_range.is_some() && status != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(DownloadError::fatal("subtitle byte range was not honored"));
        }
        let mut body = Vec::new();
        let mut chunks = opened.response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|error| {
                DownloadError::transient(format!("{what} read failed: {error}"))
            })?;
            if body.len() + chunk.len() > max_bytes {
                return Err(DownloadError::fatal(format!("{what} is too large")));
            }
            body.extend_from_slice(&chunk);
            // Subtitles do not count toward video progress, but they do share
            // the configured network token bucket.
            ctx.limiter.acquire(chunk.len() as u64, &ctx.cancel).await?;
        }
        Ok((body, final_url))
    };
    tokio::select! {
        result = work => result,
        _ = cancelled(&mut cancel) => Err(DownloadError::Cancelled),
    }
}

async fn fetch_hls_subtitle_bytes(
    ctx: &JobContext,
    source: &Url,
    target: &Url,
    headers: &HttpHeaders,
    max_bytes: usize,
    what: &str,
    byte_range: Option<(u64, u64)>,
) -> Result<(Vec<u8>, Url), DownloadError> {
    for attempt in 0..3 {
        match fetch_hls_subtitle_bytes_once(
            ctx, source, target, headers, max_bytes, what, byte_range,
        )
        .await
        {
            Err(DownloadError::Transient(_)) if attempt < 2 => {
                sleep_or_cancel(Duration::from_millis(150 * (attempt + 1) as u64), &ctx.cancel)
                    .await?;
            }
            result => return result,
        }
    }
    unreachable!()
}

/// Download one HLS subtitle rendition as ordered WebVTT segment bodies.
///
/// `source` is the original download/master URL used for cookie and Referer
/// origin decisions; `master_url` is the final URL of the playlist containing
/// the rendition. This function performs the same validated/pinned HTTP path
/// as the media engine but deliberately does not touch the video job's
/// progress counters.
pub async fn fetch_hls_subtitle_rendition_segments(
    ctx: &JobContext,
    source: &Url,
    master_url: &Url,
    rendition: &HlsSubtitleRendition,
    headers: &HttpHeaders,
) -> Result<Vec<HlsVttSegment>, DownloadError> {
    let playlist_url = master_url
        .join(&rendition.uri)
        .map_err(|error| DownloadError::fatal(format!("Invalid subtitle playlist URL: {error}")))?;
    let playlist_url =
        validate_fetch_url_async(playlist_url.as_str()).await.map_err(DownloadError::fatal)?;
    let (bytes, playlist_base) = fetch_hls_subtitle_bytes(
        ctx,
        source,
        &playlist_url,
        headers,
        MAX_HLS_SUBTITLE_PLAYLIST_BYTES,
        "subtitle playlist",
        None,
    )
    .await?;
    let playlist = parse_hls_subtitle_media_playlist(&bytes)
        .map_err(|error| DownloadError::fatal(error.to_string()))?;
    if playlist.segments.len() > MAX_HLS_SUBTITLE_SEGMENTS {
        return Err(DownloadError::fatal("too many HLS subtitle segments"));
    }
    if !playlist.end_list {
        return Err(DownloadError::fatal("ERR_HLS_LIVE"));
    }
    let plan = hls_subtitle_segment_plan(&playlist, &playlist_base)
        .map_err(|error| DownloadError::fatal(error.to_string()))?;

    // Text segments are small; four ordered fetches keep latency bounded
    // without competing with the media workers.
    let results = stream::iter(plan.into_iter().map(|segment| {
        let ctx = ctx.clone();
        let source = source.clone();
        let headers = HttpHeaders {
            cookies: headers.cookies.clone(),
            referrer: headers.referrer.clone(),
            user_agent: headers.user_agent.clone(),
            proxy: headers.proxy.clone(),
            max_connections: headers.max_connections,
        };
        async move {
            let (bytes, _) = fetch_hls_subtitle_bytes(
                &ctx,
                &source,
                &segment.url,
                &headers,
                MAX_HLS_SUBTITLE_SEGMENT_BYTES,
                "subtitle segment",
                segment.byte_range,
            )
            .await?;
            let body = String::from_utf8(bytes)
                .map_err(|_| DownloadError::fatal("subtitle segment is not valid UTF-8"))?;
            Ok::<HlsVttSegment, DownloadError>(HlsVttSegment {
                duration_ms: segment.duration_ms,
                body,
            })
        }
    }))
    .buffered(4)
    .collect::<Vec<_>>()
    .await;

    let mut out = Vec::with_capacity(results.len());
    for result in results {
        out.push(result?);
    }
    Ok(out)
}

/// Fetch, concatenate and write one HLS subtitle rendition. The orchestrator
/// should call this only after the video's final name is known and treat the
/// error as non-fatal to the already completed media download.
pub async fn write_hls_subtitle_rendition(
    ctx: &JobContext,
    source: &Url,
    master_url: &Url,
    rendition: &HlsSubtitleRendition,
    headers: &HttpHeaders,
    out_dir: &Path,
    base_filename: &str,
) -> Result<Vec<PathBuf>, DownloadError> {
    let segments =
        fetch_hls_subtitle_rendition_segments(ctx, source, master_url, rendition, headers).await?;
    let vtt = concatenate_hls_vtt_segments(&segments)
        .map_err(|error| DownloadError::fatal(error.to_string()))?;
    let language = rendition.language.as_deref().unwrap_or(rendition.name.as_str());
    write_hls_subtitle_outputs(&vtt, out_dir, base_filename, language)
        .map_err(|error| DownloadError::fatal(error.to_string()))
}

/// Write one concatenated WebVTT and its SRT conversion next to the video.
pub fn write_hls_subtitle_outputs(
    vtt: &str,
    out_dir: &Path,
    base_filename: &str,
    language: &str,
) -> SubtitleResult<Vec<PathBuf>> {
    if parse_vtt(vtt)?.is_empty() {
        return Ok(Vec::new());
    }
    let mut written = Vec::new();
    written.push(write_unique_text(
        out_dir,
        &subtitle_filename(base_filename, language, "vtt"),
        vtt,
    )?);
    let srt = vtt_to_srt(vtt, false)?;
    if !srt.trim().is_empty() {
        written.push(write_unique_text(
            out_dir,
            &subtitle_filename(base_filename, language, "srt"),
            &srt,
        )?);
    }
    Ok(written)
}

/// Fetch one YouTube caption track as WebVTT and write it as SRT.
///
/// `base_filename` is the finished video's file stem (for `movie.mp4`, pass
/// `movie`; the output is `movie.en.srt`). PO-token-restricted tracks that
/// return HTTP 200 with an empty body are skipped as `Ok(None)` so the other
/// tracks can still be delivered.
pub async fn fetch_subtitle_track(
    fetcher: &ValidatedFetcher,
    track: &CaptionTrack,
    out_dir: &Path,
    base_filename: &str,
) -> SubtitleResult<Option<PathBuf>> {
    let url = timedtext_vtt_url(&track.base_url)?;
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("text/vtt,text/plain,*/*;q=0.8"));
    let (status, body) = fetcher.get_text(&url, headers).await?;
    if !(200..300).contains(&status) {
        return Err(SubtitleError::Http(status));
    }
    if body.trim().is_empty() {
        log::warn!(
            "subtitle track {} returned an empty body (possibly PO-token restricted)",
            track.language_code
        );
        return Ok(None);
    }
    let srt = vtt_to_srt(&body, track_is_automatic(track))?;
    if srt.trim().is_empty() {
        log::warn!("subtitle track {} contained no cues", track.language_code);
        return Ok(None);
    }
    write_unique_text(out_dir, &subtitle_filename(base_filename, &track.language_code, "srt"), &srt)
        .map(Some)
}

async fn write_caption_tracks(
    tracks: Vec<CaptionTrack>,
    fetcher: &ValidatedFetcher,
    save_dir: &Path,
    base_filename: &str,
) -> Vec<PathBuf> {
    let mut written = Vec::new();
    for track in tracks {
        match fetch_subtitle_track(fetcher, &track, save_dir, base_filename).await {
            Ok(Some(path)) => written.push(path),
            Ok(None) => {}
            Err(error) => {
                log::warn!("could not write {} subtitle track: {error}", track.language_code)
            }
        }
    }
    written
}

/// Post-finalize YouTube integration seam. This uses the v1 default choice
/// (all manual tracks plus one unmatched ASR track), never fails the completed
/// video, and returns every subtitle file that landed.
pub async fn write_subtitles_for_extraction(
    extraction: &YoutubeExtraction,
    fetcher: &ValidatedFetcher,
    save_dir: &Path,
    base_filename: &str,
) -> Vec<PathBuf> {
    write_caption_tracks(select_tracks(&extraction.captions, &[]), fetcher, save_dir, base_filename)
        .await
}

/// Same integration seam with the extension's persisted track choice. `None`
/// means the default policy; `Some([])` is the user's explicit "no subtitles".
pub async fn write_selected_subtitles_for_extraction(
    extraction: &YoutubeExtraction,
    fetcher: &ValidatedFetcher,
    save_dir: &Path,
    base_filename: &str,
    selection: Option<&[SubtitleTrackSelection]>,
) -> Vec<PathBuf> {
    let tracks = match selection {
        None => select_tracks(&extraction.captions, &[]),
        Some([]) => Vec::new(),
        Some(selection) => tracks_matching_selection(&extraction.captions, selection),
    };
    write_caption_tracks(tracks, fetcher, save_dir, base_filename).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::youtube::CaptionTrack;
    use crate::storage::models::SubtitleTrackSelection;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct TimedtextFixture {
        origin: String,
        requests: Arc<Mutex<Vec<String>>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for TimedtextFixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn start_timedtext_fixture(body: &'static str) -> TimedtextFixture {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let task_requests = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    continue;
                };
                let requests = Arc::clone(&task_requests);
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        if raw.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                        let read = socket.read(&mut chunk).await.unwrap_or(0);
                        if read == 0 || raw.len() > 64 * 1024 {
                            return;
                        }
                        raw.extend_from_slice(&chunk[..read]);
                    }
                    let target = String::from_utf8_lossy(&raw)
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("")
                        .to_string();
                    requests.lock().unwrap().push(target);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/vtt\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        TimedtextFixture { origin: format!("http://127.0.0.1:{port}"), requests, task }
    }

    fn track(lang: &str, kind: Option<&str>) -> CaptionTrack {
        CaptionTrack {
            base_url: format!("https://www.youtube.com/api/timedtext?v=x&lang={lang}"),
            language_code: lang.into(),
            kind: kind.map(str::to_string),
            is_automatic: kind == Some("asr"),
            name: Some(lang.into()),
        }
    }

    #[test]
    fn timedtext_fmt_is_replaced_not_appended() {
        let url = timedtext_vtt_url(
            "https://www.youtube.com/api/timedtext?v=x&lang=en&fmt=srv3&sig=abc&fmt=json3",
        )
        .unwrap();
        let parsed = Url::parse(&url).unwrap();
        let fmts: Vec<_> = parsed.query_pairs().filter(|(key, _)| key == "fmt").collect();
        assert_eq!(fmts, vec![("fmt".into(), "vtt".into()), ("fmt".into(), "vtt".into())]);
        assert!(parsed.query_pairs().any(|(key, value)| key == "sig" && value == "abc"));
        assert!(!parsed.query_pairs().any(|(key, value)| key == "fmt" && value != "vtt"));
    }

    #[test]
    fn timedtext_rewrite_preserves_every_signed_query_byte() {
        let input = "https://www.youtube.com/api/timedtext?v=abc%2Fdef&sig=Z%2Fy%2B+x%3D&expire=1900000000&lang=en-US&fmt=srv3&origin=https%3A%2F%2Fexample.com%2Fa+b#caption%2Ffrag";
        let expected = "https://www.youtube.com/api/timedtext?v=abc%2Fdef&sig=Z%2Fy%2B+x%3D&expire=1900000000&lang=en-US&fmt=vtt&origin=https%3A%2F%2Fexample.com%2Fa+b#caption%2Ffrag";
        assert_eq!(timedtext_vtt_url(input).unwrap(), expected);
    }

    #[test]
    fn timedtext_rewrite_appends_without_touching_existing_bytes() {
        let input = "https://www.youtube.com/api/timedtext?v=abc%2Fdef&sig=Z%2Fy%2B+x%3D";
        assert_eq!(timedtext_vtt_url(input).unwrap(), format!("{input}&fmt=vtt"));
        let fragment = "https://www.youtube.com/api/timedtext#captions%2Ffrag";
        assert_eq!(
            timedtext_vtt_url(fragment).unwrap(),
            "https://www.youtube.com/api/timedtext?fmt=vtt#captions%2Ffrag"
        );
        let empty_query = "https://www.youtube.com/api/timedtext?#captions";
        assert_eq!(
            timedtext_vtt_url(empty_query).unwrap(),
            "https://www.youtube.com/api/timedtext?fmt=vtt#captions"
        );
    }

    #[test]
    fn select_tracks_keeps_all_manual_and_one_unmatched_asr() {
        let captions = vec![track("en", None), track("tr", None), track("en", Some("asr"))];
        let selected = select_tracks(&captions, &[]);
        assert_eq!(selected.len(), 2);
        assert!(selected.iter().filter(|track| !track_is_automatic(track)).count() == 2);
        assert!(!selected.iter().any(track_is_automatic));

        let captions = vec![track("tr", None), track("en", Some("asr"))];
        let selected = select_tracks(&captions, &[]);
        assert_eq!(selected.len(), 2);
        assert!(selected.iter().any(track_is_automatic));
    }

    #[test]
    fn select_tracks_preferred_language_orders_asr_candidates() {
        let captions = vec![track("en", Some("asr")), track("tr", Some("asr"))];
        let selected = select_tracks(&captions, &["tr".to_string()]);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].language_code, "tr");
    }

    #[test]
    fn explicit_selection_distinguishes_manual_and_asr() {
        let captions = vec![track("en", None), track("en", Some("asr")), track("tr", None)];
        let selected = tracks_matching_selection(
            &captions,
            &[SubtitleTrackSelection { lang: "en".into(), kind: Some("asr".into()) }],
        );
        assert_eq!(selected.len(), 1);
        assert!(track_is_automatic(&selected[0]));
    }

    #[test]
    fn normalizes_extension_selection_and_keeps_explicit_empty() {
        let normalized = normalize_subtitle_selection(&[
            SubtitleTrackSelection { lang: " EN_us ".into(), kind: Some("ASR".into()) },
            SubtitleTrackSelection { lang: "en-us".into(), kind: Some("asr".into()) },
            SubtitleTrackSelection { lang: "../bad".into(), kind: None },
        ]);
        assert_eq!(
            normalized,
            vec![SubtitleTrackSelection { lang: "en-us".into(), kind: Some("asr".into()) }]
        );
    }

    #[test]
    fn vtt_to_srt_converts_times_tags_entities_and_numbers() {
        let input = "WEBVTT\n\nidentifier\n00:00.500 --> 00:02.000 position:0% align:start\nHello <c>world</c> &amp; friends\nsecond line\n\nNOTE ignored\n\n01:00:01.250 --> 01:00:03.000\n&quot;next&quot;\n";
        let srt = vtt_to_srt(input, false).unwrap();
        assert!(
            srt.contains("1\n00:00:00,500 --> 00:00:02,000\nHello world & friends\nsecond line\n")
        );
        assert!(srt.contains("2\n01:00:01,250 --> 01:00:03,000\n\"next\"\n"));
        assert!(!srt.contains("position:0%"));
    }

    #[test]
    fn asr_rolling_captions_are_flattened() {
        let input = concat!(
            "WEBVTT\n\n",
            "00:00:01.000 --> 00:00:03.830 align:start position:0%\n",
            "hi<00:00:01.400><c> my</c><00:00:01.700><c> name</c>\n\n",
            "00:00:03.830 --> 00:00:03.840 align:start position:0%\n",
            "hi my name\n\n",
            "00:00:03.840 --> 00:00:05.000 align:start position:0%\n",
            "hi my name\n",
            "gene<00:00:04.100><c> ai</c>\n\n",
            "00:00:05.000 --> 00:00:06.000 align:start position:0%\n",
            "gene ai\n",
            "speaks\n",
        );
        let srt = vtt_to_srt(input, true).unwrap();
        assert_eq!(srt.matches("hi my name").count(), 1, "{srt}");
        assert_eq!(srt.matches("gene ai").count(), 1, "{srt}");
        assert!(srt.contains("speaks"), "{srt}");
    }

    #[test]
    fn asr_word_overlap_is_removed_when_lines_do_not_align() {
        let input = concat!(
            "WEBVTT\n\n",
            "00:00:00.000 --> 00:00:01.000\nhello rolling world\n\n",
            "00:00:01.000 --> 00:00:02.000\nrolling world continues\n",
        );
        let srt = vtt_to_srt(input, true).unwrap();
        assert!(srt.contains("hello rolling world"));
        assert!(srt.contains("continues"));
        assert!(!srt.contains("rolling world continues"), "{srt}");
    }

    #[test]
    fn concatenates_hls_segments_with_timestamp_map_offsets() {
        let one = HlsVttSegment {
            duration_ms: 2_000,
            body: "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:900000\n\n00:00:00.000 --> 00:00:01.000\none\n".into(),
        };
        let two = HlsVttSegment {
            duration_ms: 2_000,
            body: "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:1080000\n\n00:00:00.000 --> 00:00:01.000\ntwo\n".into(),
        };
        let vtt = concatenate_hls_vtt_segments(&[one, two]).unwrap();
        assert!(vtt.contains("00:00:00.000 --> 00:00:01.000\none"), "{vtt}");
        assert!(vtt.contains("00:00:02.000 --> 00:00:03.000\ntwo"), "{vtt}");
        assert!(!vtt.contains("X-TIMESTAMP-MAP"), "{vtt}");
    }

    #[test]
    fn concatenation_handles_mpegts_wrap_and_missing_map() {
        let wrapped = HlsVttSegment {
            duration_ms: 2_000,
            body: format!(
                "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:{}\n\n00:00:00.000 --> 00:00:01.000\none\n",
                (1_u64 << 33) - 90_000
            ),
        };
        let after = HlsVttSegment {
            duration_ms: 2_000,
            body: "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:90000\n\n00:00:00.000 --> 00:00:01.000\ntwo\n".into(),
        };
        let missing = HlsVttSegment {
            duration_ms: 2_000,
            body: "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nthree\n".into(),
        };
        let vtt = concatenate_hls_vtt_segments(&[wrapped, after, missing]).unwrap();
        assert!(vtt.contains("00:00:02.000 --> 00:00:03.000\ntwo"), "{vtt}");
        assert!(vtt.contains("00:00:04.000 --> 00:00:05.000\nthree"), "{vtt}");
    }

    #[test]
    fn hls_output_writes_vtt_and_srt_without_overwriting() {
        let root = std::env::temp_dir().join(format!("falcon-subtitles-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("movie.en.srt"), "keep").unwrap();
        let vtt = "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nhello\n";
        let written = write_hls_subtitle_outputs(vtt, &root, "movie", "en").unwrap();
        assert_eq!(written.len(), 2);
        assert_eq!(std::fs::read_to_string(root.join("movie.en.srt")).unwrap(), "keep");
        assert!(written.iter().any(|path| path.file_name().unwrap() == "movie.en (1).srt"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn hls_renditions_follow_the_selected_variant_group() {
        let master = br#"#EXTM3U
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",LANGUAGE="en",NAME="English",DEFAULT=YES,URI="subs/en.m3u8"
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",LANGUAGE="tr",NAME="Turkish",URI="subs/tr.m3u8"
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="other",LANGUAGE="fr",NAME="French",URI="subs/fr.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=1000,SUBTITLES="subs"
video.m3u8
"#;
        let playlist = match m3u8_rs::parse_playlist_res(master).unwrap() {
            Playlist::MasterPlaylist(master) => master,
            _ => panic!("fixture must be a master playlist"),
        };
        let renditions = hls_subtitle_renditions(&playlist, Some("subs"));
        assert_eq!(renditions.len(), 2);
        assert_eq!(renditions[0].language.as_deref(), Some("en"));
        assert!(renditions[0].is_default);
        assert_eq!(renditions[1].uri, "subs/tr.m3u8");
        assert_eq!(hls_subtitle_renditions(&playlist, Some("missing")), Vec::new());
    }

    #[test]
    fn hls_segment_plan_resolves_urls_and_extinf_durations() {
        let media = br#"#EXTM3U
#EXT-X-TARGETDURATION:2
#EXT-X-MEDIA-SEQUENCE:0
#EXTINF:2.000,
part0.vtt
#EXTINF:1.500,
nested/part1.vtt
#EXT-X-ENDLIST
"#;
        let playlist = match m3u8_rs::parse_playlist_res(media).unwrap() {
            Playlist::MediaPlaylist(playlist) => playlist,
            _ => panic!("fixture must be a media playlist"),
        };
        let base = Url::parse("https://cdn.example.com/hls/subs/en.m3u8").unwrap();
        let plan = hls_subtitle_segment_plan(&playlist, &base).unwrap();
        assert_eq!(plan[0].url.as_str(), "https://cdn.example.com/hls/subs/part0.vtt");
        assert_eq!(plan[0].duration_ms, 2_000);
        assert_eq!(plan[1].url.as_str(), "https://cdn.example.com/hls/subs/nested/part1.vtt");
        assert_eq!(plan[1].duration_ms, 1_500);
        assert_eq!(plan[1].byte_range, None);
    }

    #[test]
    fn hls_segment_plan_resolves_implicit_byte_range_offsets() {
        let media = br#"#EXTM3U
#EXT-X-TARGETDURATION:2
#EXTINF:2.000,
#EXT-X-BYTERANGE:100@50
all.vtt
#EXTINF:2.000,
#EXT-X-BYTERANGE:80
all.vtt
#EXT-X-ENDLIST
"#;
        let playlist = match m3u8_rs::parse_playlist_res(media).unwrap() {
            Playlist::MediaPlaylist(playlist) => playlist,
            _ => panic!("fixture must be a media playlist"),
        };
        let base = Url::parse("https://cdn.example.com/subs/en.m3u8").unwrap();
        let plan = hls_subtitle_segment_plan(&playlist, &base).unwrap();
        assert_eq!(plan[0].byte_range, Some((50, 149)));
        assert_eq!(plan[1].byte_range, Some((150, 229)));
    }

    #[test]
    fn concatenation_uses_nonzero_local_timestamp_mapping() {
        let one = HlsVttSegment {
            duration_ms: 2_000,
            body: "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:02.000,MPEGTS:180000\n\n00:00:00.000 --> 00:00:01.000\none\n".into(),
        };
        let two = HlsVttSegment {
            duration_ms: 2_000,
            body: "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:02.000,MPEGTS:360000\n\n00:00:00.000 --> 00:00:01.000\ntwo\n".into(),
        };
        let vtt = concatenate_hls_vtt_segments(&[one, two]).unwrap();
        assert!(vtt.contains("00:00:00.000 --> 00:00:01.000\none"), "{vtt}");
        assert!(vtt.contains("00:00:02.000 --> 00:00:03.000\ntwo"), "{vtt}");
    }

    async fn start_hls_fixture() -> TimedtextFixture {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let task_requests = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    continue;
                };
                let requests = Arc::clone(&task_requests);
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        if raw.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                        let read = socket.read(&mut chunk).await.unwrap_or(0);
                        if read == 0 || raw.len() > 64 * 1024 {
                            return;
                        }
                        raw.extend_from_slice(&chunk[..read]);
                    }
                    let target = String::from_utf8_lossy(&raw)
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("")
                        .to_string();
                    requests.lock().unwrap().push(target.clone());
                    let (status, body, extra) = match target.as_str() {
                        "/subs/en.m3u8" => (
                            "302 Found",
                            String::new(),
                            "location: /subs/final.m3u8\r\n".to_string(),
                        ),
                        "/subs/final.m3u8" => (
                            "200 OK",
                            "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.000,\npart0.vtt\n#EXTINF:2.000,\npart1.vtt\n#EXT-X-ENDLIST\n".into(),
                            String::new(),
                        ),
                        "/subs/part0.vtt" => (
                            "200 OK",
                            "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:900000\n\n00:00:00.000 --> 00:00:01.000\none\n".into(),
                            String::new(),
                        ),
                        "/subs/part1.vtt" => (
                            "200 OK",
                            "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:1080000\n\n00:00:00.000 --> 00:00:01.000\ntwo\n".into(),
                            String::new(),
                        ),
                        _ => ("404 Not Found", String::new(), String::new()),
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: text/vtt\r\ncontent-length: {}\r\n{extra}connection: close\r\n\r\n{body}",
                        body.len(),
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        TimedtextFixture { origin: format!("http://127.0.0.1:{port}"), requests, task }
    }

    #[tokio::test]
    async fn hls_rendition_download_follows_redirects_and_preserves_order() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let server = start_hls_fixture().await;
        let source = Url::parse(&format!("{}/master.m3u8", server.origin)).unwrap();
        let rendition = HlsSubtitleRendition {
            group_id: "subs".into(),
            language: Some("en".into()),
            name: "English".into(),
            uri: "subs/en.m3u8".into(),
            is_default: true,
            is_autoselect: true,
            is_forced: false,
        };
        let (cancel, rx) = tokio::sync::watch::channel(false);
        drop(cancel);
        let limiter = crate::download::limiter::CompositeRateLimiter::global_only(Arc::new(
            crate::download::limiter::RateLimiter::new(0),
        ));
        let ctx = JobContext::new(991, None, None, rx, limiter);
        let segments = fetch_hls_subtitle_rendition_segments(
            &ctx,
            &source,
            &source,
            &rendition,
            &HttpHeaders::default(),
        )
        .await
        .unwrap();
        assert_eq!(segments.len(), 2);
        let vtt = concatenate_hls_vtt_segments(&segments).unwrap();
        assert!(vtt.contains("00:00:00.000 --> 00:00:01.000\none"), "{vtt}");
        assert!(vtt.contains("00:00:02.000 --> 00:00:03.000\ntwo"), "{vtt}");
        let requested = server.requests.lock().unwrap().join("\n");
        assert!(requested.contains("/subs/final.m3u8"), "{requested}");
        assert!(requested.contains("/subs/part0.vtt"), "{requested}");
    }

    #[tokio::test]
    async fn fetches_timedtext_as_vtt_and_writes_srt() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let vtt = "WEBVTT\n\n00:00:00.500 --> 00:00:02.000\nHello <c>world</c>\n";
        let server = start_timedtext_fixture(vtt).await;
        let fetcher = ValidatedFetcher::with_endpoint_origin(&server.origin);
        let root = std::env::temp_dir().join(format!("falcon-timedtext-{}", uuid::Uuid::new_v4()));
        let track = CaptionTrack {
            base_url: "https://www.youtube.com/api/timedtext?v=x&lang=en&fmt=srv3".into(),
            language_code: "en".into(),
            kind: None,
            is_automatic: false,
            name: Some("English".into()),
        };
        let path = fetch_subtitle_track(&fetcher, &track, &root, "movie").await.unwrap().unwrap();
        assert_eq!(path.file_name().unwrap(), "movie.en.srt");
        let srt = std::fs::read_to_string(path).unwrap();
        assert!(srt.contains("00:00:00,500 --> 00:00:02,000\nHello world"), "{srt}");
        let requested = server.requests.lock().unwrap().join("\n");
        assert!(requested.contains("/api/timedtext?"), "{requested}");
        assert!(requested.contains("fmt=vtt"), "{requested}");
        assert!(!requested.contains("fmt=srv3"), "{requested}");
        let _ = std::fs::remove_dir_all(root);
    }
}
