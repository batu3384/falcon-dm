//! HLS (m3u8) engine for MPEG-TS and fragmented-MP4 playlists.
//!
//! Segments are fetched in parallel into a per-download temp dir, each written
//! to `*.tmp` and renamed when complete, so an interrupted run never leaves a
//! half segment that a resume would trust. AES-128 pieces are decrypted after
//! their HTTP length/range validation and before the final atomic rename; key
//! bytes stay memory-only and are refetched to validate encrypted-piece caches.
//! The native remuxer assembles MPEG-TS; fMP4 is already an MP4 representation and
//! is concatenated as init sections + media fragments.
//!
//! EXT-X-DISCONTINUITY does not change the VOD byte plan: pieces are fetched in
//! playlist order and concatenated across the boundary. That is the same byte
//! sequence a typical HLS concat path produces; players and the TS demuxer see
//! the timestamp reset. DISCONTINUITY-SEQUENCE is a live-window indexing fact
//! and needs no special handling for a finite download.

use crate::download::http_client::{open, parse_content_range, HttpHeaders, Opened, RangeRequest};
use crate::download::job::JobContext;
use crate::download::limiter::{cancelled, sleep_or_cancel};
use crate::download::mux::{self, MergeError, RemuxError};
use crate::download::subtitles::{
    hls_subtitle_renditions, write_hls_subtitle_rendition, HlsSubtitleRendition,
};
use crate::download::DownloadError;
use crate::util::validate_fetch_url_async;
use aes::cipher::{
    block_padding::{Padding, Pkcs7},
    Block, BlockModeDecrypt, KeyIvInit,
};
use futures::stream::{self, StreamExt};
use m3u8_rs::{AlternativeMedia, AlternativeMediaType, MasterPlaylist};
use reqwest::{Client, Response, StatusCode};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

const MAX_PLAYLIST_BYTES: usize = 16 * 1024 * 1024;
const MAX_SEGMENT_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_SEGMENTS: usize = 100_000;
const SEGMENT_ATTEMPTS: u32 = 4;
const AES_BLOCK_BYTES: usize = 16;
const AES_KEY_BYTES: usize = 16;
const ERR_HLS_DRM: &str = "ERR_HLS_DRM";

pub(crate) fn hls_session_dirname(download_id: i64) -> String {
    format!("hls-{download_id}.falcondm-temp")
}

pub(crate) fn hls_session_dir(download_id: i64) -> PathBuf {
    crate::util::app_data_dir().join("downloads_temp").join(hls_session_dirname(download_id))
}

/// Remove the segment cache of a download (remove, or retry of a failed row).
pub fn remove_artifacts(download_id: i64) {
    let _ = std::fs::remove_dir_all(hls_session_dir(download_id));
}

fn segment_name(index: usize) -> String {
    format!("seg_{index:05}.ts")
}

/// Segment names are engine-generated files. Validate before joining anyway so
/// a bad cache or future caller cannot escape the rendition dir.
fn segment_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || name.contains("..")
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
    {
        return Err("Invalid HLS segment name".into());
    }
    Ok(dir.join(name))
}

/// Highest-bandwidth real variant. I-frame-only playlists are trick-play tracks.
fn pick_best_variant(variants: &[m3u8_rs::VariantStream]) -> Option<&m3u8_rs::VariantStream> {
    variants.iter().filter(|v| !v.is_i_frame).max_by_key(|v| v.bandwidth)
}

/// Default audio rendition of the variant's group, else the first URI-bearing one.
fn pick_audio_rendition<'a>(
    master: &'a MasterPlaylist,
    variant: &m3u8_rs::VariantStream,
) -> Option<&'a AlternativeMedia> {
    let group = variant.audio.as_deref()?;
    let mut first = None;
    for media in master.alternatives.iter().filter(|media| {
        media.media_type == AlternativeMediaType::Audio
            && media.group_id == group
            && media.uri.as_deref().is_some_and(|uri| !uri.trim().is_empty())
    }) {
        if first.is_none() {
            first = Some(media);
        }
        if media.default {
            return Some(media);
        }
    }
    first
}

fn io_error(context: &str, error: &std::io::Error) -> DownloadError {
    if error.raw_os_error() == Some(28) {
        DownloadError::fatal("ERR_DISK_FULL")
    } else {
        DownloadError::fatal(format!("{context}: {error}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HlsKeyMethod {
    None,
    Aes128,
    SampleAes,
    Other(String),
}

#[derive(Clone, Debug)]
struct KeyTag {
    method: HlsKeyMethod,
    uri: Option<String>,
    iv: Option<[u8; AES_BLOCK_BYTES]>,
    keyformat: Option<String>,
}

#[derive(Clone, Debug)]
struct MapTag {
    uri: String,
    byte_range: Option<(u64, Option<u64>)>,
    /// Encryption state at the instant the map was declared. RFC 8216 applies
    /// a key to every EXT-X-MAP declared between it and the next key tag.
    key: Option<KeyTag>,
}

#[derive(Clone, Debug)]
struct SegmentTags {
    uri: String,
    duration: f32,
    byte_range: Option<(u64, Option<u64>)>,
    key: Option<KeyTag>,
    map: Option<MapTag>,
}

#[derive(Clone, Debug)]
struct MediaData {
    media_sequence: u64,
    end_list: bool,
    segments: Vec<SegmentTags>,
}

/// Parse an HLS attribute list without splitting quoted commas. Quoted strings
/// cannot contain quotes by RFC 8216, so no escape layer is needed.
fn parse_attribute_list(raw: &str) -> Result<HashMap<String, String>, DownloadError> {
    let bytes = raw.as_bytes();
    let mut attrs = HashMap::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        let key_start = pos;
        while pos < bytes.len() && bytes[pos] != b'=' && bytes[pos] != b',' {
            pos += 1;
        }
        if pos >= bytes.len() || bytes[pos] != b'=' {
            return Err(DownloadError::fatal("Invalid HLS attribute list"));
        }
        let key = raw[key_start..pos].trim().to_string();
        if key.is_empty() {
            return Err(DownloadError::fatal("Invalid HLS attribute list"));
        }
        pos += 1;
        let value = if pos < bytes.len() && bytes[pos] == b'"' {
            pos += 1;
            let start = pos;
            while pos < bytes.len() && bytes[pos] != b'"' {
                pos += 1;
            }
            if pos >= bytes.len() {
                return Err(DownloadError::fatal("Unterminated HLS quoted attribute"));
            }
            let value = raw[start..pos].to_string();
            pos += 1;
            value
        } else {
            let start = pos;
            while pos < bytes.len() && bytes[pos] != b',' {
                pos += 1;
            }
            raw[start..pos].trim().to_string()
        };
        if pos < bytes.len() {
            if bytes[pos] != b',' {
                return Err(DownloadError::fatal("Invalid HLS attribute list"));
            }
            pos += 1;
        }
        attrs.insert(key, value);
    }
    Ok(attrs)
}

fn parse_range_value(raw: &str) -> Result<(u64, Option<u64>), DownloadError> {
    let invalid = || DownloadError::fatal(format!("Invalid HLS byte range: {raw}"));
    let (length, offset) =
        raw.trim().split_once('@').map_or((raw.trim(), None), |(n, o)| (n.trim(), Some(o.trim())));
    let length = length.parse::<u64>().map_err(|_| invalid())?;
    let offset = offset.map(|value| value.parse::<u64>().map_err(|_| invalid())).transpose()?;
    Ok((length, offset))
}

/// RFC 8216 treats IV as a 128-bit integer. Accept leading-zero-shorter hex
/// forms and normalize them to the required 16-byte big-endian value.
fn parse_iv(raw: &str) -> Result<[u8; AES_BLOCK_BYTES], DownloadError> {
    let hex = raw
        .trim()
        .strip_prefix("0x")
        .or_else(|| raw.trim().strip_prefix("0X"))
        .ok_or_else(|| DownloadError::fatal("Invalid HLS AES-128 IV"))?;
    if hex.is_empty() || hex.len() > 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(DownloadError::fatal("Invalid HLS AES-128 IV"));
    }
    let value = u128::from_str_radix(hex, 16)
        .map_err(|_| DownloadError::fatal("Invalid HLS AES-128 IV"))?;
    Ok(value.to_be_bytes())
}

fn parse_key_tag(raw: &str) -> Result<KeyTag, DownloadError> {
    let attrs = parse_attribute_list(raw)?;
    let method = match attrs.get("METHOD").map(String::as_str) {
        Some("NONE") => HlsKeyMethod::None,
        Some("AES-128") => HlsKeyMethod::Aes128,
        Some("SAMPLE-AES") => HlsKeyMethod::SampleAes,
        Some(other) => HlsKeyMethod::Other(other.to_string()),
        None => return Err(DownloadError::fatal("EXT-X-KEY is missing METHOD")),
    };
    let iv = attrs.get("IV").map(|value| parse_iv(value)).transpose()?;
    Ok(KeyTag {
        method,
        uri: attrs.get("URI").cloned(),
        iv,
        keyformat: attrs.get("KEYFORMAT").cloned(),
    })
}

fn parse_map_tag(raw: &str, key: Option<KeyTag>) -> Result<MapTag, DownloadError> {
    let attrs = parse_attribute_list(raw)?;
    let uri = attrs
        .get("URI")
        .map(String::as_str)
        .map(str::trim)
        .filter(|uri| !uri.is_empty())
        .ok_or_else(|| DownloadError::fatal("EXT-X-MAP is missing URI"))?
        .to_string();
    let byte_range = attrs.get("BYTERANGE").map(|value| parse_range_value(value)).transpose()?;
    Ok(MapTag { uri, byte_range, key })
}

/// Media-playlist parser for the segment facts this engine consumes. m3u8-rs
/// remains the master-playlist parser, but its media model collapses stateful
/// EXT-X-KEY/EXT-X-MAP scoping after the first URI; RFC 8216 makes both tags
/// persistent, so the media scanner keeps that state explicitly.
fn parse_hls_media(bytes: &[u8]) -> Result<MediaData, DownloadError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| DownloadError::fatal("HLS playlist is not valid UTF-8"))?;
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("#EXTM3U") {
        return Err(DownloadError::fatal("Failed to parse m3u8 playlist"));
    }
    let mut media = MediaData { media_sequence: 0, end_list: false, segments: Vec::new() };
    let mut current_key: Option<KeyTag> = None;
    let mut current_map: Option<MapTag> = None;
    let mut pending_range = None;
    let mut pending_duration = 0.0f32;
    for raw_line in lines {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(raw) = line.strip_prefix("#EXT-X-KEY:") {
            let key = parse_key_tag(raw)?;
            current_key = (key.method != HlsKeyMethod::None).then_some(key);
            continue;
        }
        if let Some(raw) = line.strip_prefix("#EXT-X-MAP:") {
            current_map = Some(parse_map_tag(raw, current_key.clone())?);
            continue;
        }
        if let Some(raw) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            pending_range = Some(parse_range_value(raw)?);
            continue;
        }
        if let Some(raw) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            media.media_sequence = raw
                .trim()
                .parse::<u64>()
                .map_err(|_| DownloadError::fatal("Invalid HLS media sequence"))?;
            continue;
        }
        if let Some(raw) = line.strip_prefix("#EXTINF:") {
            let duration = raw.split(',').next().unwrap_or("").trim();
            pending_duration = duration
                .parse::<f32>()
                .map_err(|_| DownloadError::fatal("Invalid HLS segment duration"))?;
            continue;
        }
        if line == "#EXT-X-ENDLIST" {
            media.end_list = true;
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        media.segments.push(SegmentTags {
            uri: line.to_string(),
            duration: pending_duration,
            byte_range: pending_range.take(),
            key: current_key.clone(),
            map: current_map.clone(),
        });
        pending_duration = 0.0;
    }
    Ok(media)
}

fn playlist_is_master(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    text.lines().map(str::trim).any(|line| {
        line.starts_with("#EXT-X-STREAM-INF")
            || line.starts_with("#EXT-X-I-FRAME-STREAM-INF")
            || line.starts_with("#EXT-X-MEDIA:")
            || line.starts_with("#EXT-X-SESSION-KEY")
            || line.starts_with("#EXT-X-SESSION-DATA")
    })
}

/// The only playlist encryption feature the native engine still refuses:
/// SAMPLE-AES is sample-level encryption normally used with FairPlay DRM.
fn unsupported_feature(playlist: &MediaData) -> Option<&'static str> {
    playlist
        .segments
        .iter()
        .any(|segment| {
            segment.key.as_ref().is_some_and(|key| key.method == HlsKeyMethod::SampleAes)
                || segment
                    .map
                    .as_ref()
                    .and_then(|map| map.key.as_ref())
                    .is_some_and(|key| key.method == HlsKeyMethod::SampleAes)
        })
        .then_some("SAMPLE-AES DRM")
}

async fn fetch_playlist(
    ctx: &JobContext,
    source: &Url,
    target: &Url,
    headers: &HttpHeaders,
) -> Result<(Vec<u8>, Url), DownloadError> {
    let mut cancel = ctx.cancel.clone();
    let work = async {
        let Opened { response, final_url, .. } =
            open(source, target, headers, &RangeRequest::default(), None).await?;
        let status = response.status();
        if !status.is_success() {
            return Err(DownloadError::from_status(status.as_u16()));
        }
        let mut body = Vec::new();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk
                .map_err(|e| DownloadError::transient(format!("playlist read failed: {e}")))?;
            if body.len() + chunk.len() > MAX_PLAYLIST_BYTES {
                return Err(DownloadError::fatal("HLS playlist is too large"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok((body, final_url))
    };
    tokio::select! {
        result = work => result,
        _ = cancelled(&mut cancel) => Err(DownloadError::Cancelled),
    }
}

async fn fetch_media_playlist(
    ctx: &JobContext,
    source: &Url,
    target: &Url,
    headers: &HttpHeaders,
) -> Result<(MediaData, Vec<u8>, Url), DownloadError> {
    let (bytes, base) = fetch_playlist(ctx, source, target, headers).await?;
    if playlist_is_master(&bytes) {
        return Err(DownloadError::fatal("Expected a media playlist in variant"));
    }
    let media = parse_hls_media(&bytes)?;
    Ok((media, bytes, base))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    /// Inclusive final byte, matching HTTP Range/Content-Range.
    end: u64,
}

impl ByteRange {
    fn len(self) -> u64 {
        self.end - self.start + 1
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Encryption {
    key: usize,
    iv: [u8; AES_BLOCK_BYTES],
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Piece {
    url: Url,
    range: Option<ByteRange>,
    encryption: Option<Encryption>,
}

#[derive(Clone, Debug)]
struct SegmentPlan {
    piece: Piece,
    init: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Container {
    Ts,
    Fmp4,
}

#[derive(Clone, Debug)]
struct MediaPlan {
    media_url: Url,
    container: Container,
    inits: Vec<Piece>,
    segments: Vec<SegmentPlan>,
    estimated_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
struct KeyPlan {
    url: Url,
}

struct Plan {
    /// Playlist URL the user gave us: cookies are only sent to its origin.
    source: Url,
    keys: Vec<KeyPlan>,
    /// Main rendition first; a selected split-audio rendition second.
    renditions: Vec<MediaPlan>,
    estimated_bytes: Option<u64>,
    /// FNV-1a over query-stripped piece locators plus range/IV/init/key-path
    /// facts. Signed media-URL rotation must not invalidate cache, while a
    /// re-cut or re-laddered playlist (different paths or addressing) must.
    /// Key bytes are fingerprinted separately once fetched: they, not a
    /// signed key URL's query string, decide whether plaintext pieces still
    /// mean the same media.
    fingerprint: String,
    /// Subtitle renditions selected from the master playlist, written next to
    /// the video after it finalizes. `None` for plain media playlists and for
    /// masters without a subtitle selection.
    subtitles: Option<SubtitlePlan>,
}

#[derive(Clone, Debug)]
struct SubtitlePlan {
    /// Final (post-redirect) master-playlist URL the rendition URIs resolve
    /// against and segment fetches pin to.
    master_url: Url,
    renditions: Vec<HlsSubtitleRendition>,
}

/// Product selection: the DEFAULT=YES rendition when one exists, else the
/// first AUTOSELECT=YES, else nothing — plus every FORCED=YES rendition.
/// Output stays in master-playlist order and never duplicates a rendition
/// that is both default/autoselected and forced.
fn select_subtitle_renditions(renditions: &[HlsSubtitleRendition]) -> Vec<HlsSubtitleRendition> {
    let primary = renditions
        .iter()
        .position(|rendition| rendition.is_default)
        .or_else(|| renditions.iter().position(|rendition| rendition.is_autoselect));
    renditions
        .iter()
        .enumerate()
        .filter(|(index, rendition)| Some(*index) == primary || rendition.is_forced)
        .map(|(_, rendition)| rendition.clone())
        .collect()
}

#[derive(Default)]
struct KeyRegistry {
    keys: Vec<KeyPlan>,
    indices: HashMap<String, usize>,
}

impl KeyRegistry {
    fn index(&mut self, url: &Url) -> usize {
        if let Some(index) = self.indices.get(url.as_str()) {
            return *index;
        }
        let index = self.keys.len();
        self.keys.push(KeyPlan { url: url.clone() });
        self.indices.insert(url.as_str().to_string(), index);
        index
    }
}

fn resolve_reference(base: &Url, reference: &str, what: &str) -> Result<Url, DownloadError> {
    let url = base
        .join(reference.trim())
        .map_err(|e| DownloadError::fatal(format!("Invalid HLS {what} URL: {e}")))?;
    if matches!(url.scheme(), "http" | "https") {
        Ok(url)
    } else {
        Err(DownloadError::fatal(format!("Unsupported HLS {what} URL scheme")))
    }
}

fn sequence_iv(sequence: u64) -> [u8; AES_BLOCK_BYTES] {
    u128::from(sequence).to_be_bytes()
}

fn encryption_for_key(
    key: &KeyTag,
    base: &Url,
    registry: &mut KeyRegistry,
    media_sequence: u64,
    explicit_iv_required: bool,
) -> Result<Option<Encryption>, DownloadError> {
    match &key.method {
        HlsKeyMethod::None => Ok(None),
        HlsKeyMethod::SampleAes => Err(DownloadError::fatal(ERR_HLS_DRM)),
        HlsKeyMethod::Other(method) => {
            Err(DownloadError::fatal(format!("Unsupported HLS encryption method: {method}")))
        }
        HlsKeyMethod::Aes128 => {
            if key
                .keyformat
                .as_deref()
                .is_some_and(|format| !format.eq_ignore_ascii_case("identity"))
            {
                return Err(DownloadError::fatal(ERR_HLS_DRM));
            }
            let uri = key
                .uri
                .as_deref()
                .map(str::trim)
                .filter(|uri| !uri.is_empty())
                .ok_or_else(|| DownloadError::fatal("EXT-X-KEY is missing URI"))?;
            let url = resolve_reference(base, uri, "key")?;
            let iv = match key.iv {
                Some(iv) => iv,
                None if explicit_iv_required => {
                    return Err(DownloadError::fatal(
                        "Encrypted HLS initialization section requires an explicit IV",
                    ));
                }
                None => sequence_iv(media_sequence),
            };
            let key = registry.index(&url);
            Ok(Some(Encryption { key, iv }))
        }
    }
}

fn checked_range(length: u64, start: u64) -> Result<ByteRange, DownloadError> {
    if length == 0 || length > MAX_SEGMENT_BYTES {
        return Err(DownloadError::fatal("Invalid HLS byte range length"));
    }
    let end = start
        .checked_add(length - 1)
        .ok_or_else(|| DownloadError::fatal("HLS byte range overflow"))?;
    Ok(ByteRange { start, end })
}

fn estimate_media_bytes(
    inits: &[Piece],
    segments: &[SegmentPlan],
    bandwidth: Option<u64>,
    duration: f64,
) -> Option<u64> {
    let mut exact = 0u64;
    let mut all_ranged = true;
    let mut count = 0usize;
    for piece in inits.iter().chain(segments.iter().map(|segment| &segment.piece)) {
        count += 1;
        if let Some(range) = piece.range {
            exact = exact.checked_add(range.len())?;
        } else {
            all_ranged = false;
        }
    }
    if count > 0 && all_ranged {
        return Some(exact);
    }
    let bandwidth = bandwidth.filter(|bandwidth| *bandwidth > 0)?;
    (duration > 0.0).then(|| (bandwidth as f64 / 8.0 * duration) as u64)
}

fn build_media_plan(
    media: &MediaData,
    base: &Url,
    bandwidth: Option<u64>,
    registry: &mut KeyRegistry,
) -> Result<MediaPlan, DownloadError> {
    if unsupported_feature(media).is_some() {
        return Err(DownloadError::fatal(ERR_HLS_DRM));
    }
    if !media.end_list {
        // A live window would save a few seconds and call it done.
        return Err(DownloadError::fatal("ERR_HLS_LIVE"));
    }
    if media.segments.is_empty() {
        return Err(DownloadError::fatal("No segments found"));
    }
    if media.segments.len() > MAX_SEGMENTS {
        return Err(DownloadError::fatal(format!(
            "Playlist contains too many segments: {}",
            media.segments.len()
        )));
    }

    let duration: f64 = media.segments.iter().map(|s| f64::from(s.duration)).sum();
    let mut inits = Vec::new();
    let mut segments = Vec::with_capacity(media.segments.len());
    let mut next_range_offsets = HashMap::<String, u64>::new();
    for (index, segment) in media.segments.iter().enumerate() {
        let url = resolve_reference(base, &segment.uri, "segment")?;
        let range = match segment.byte_range {
            Some((length, explicit)) => {
                let key = url.as_str().to_string();
                let start = explicit.or_else(|| next_range_offsets.get(&key).copied()).unwrap_or(0);
                let range = checked_range(length, start)?;
                if let Some(next) = range.end.checked_add(1) {
                    next_range_offsets.insert(key, next);
                }
                Some(range)
            }
            None => None,
        };
        let sequence = media
            .media_sequence
            .checked_add(index as u64)
            .ok_or_else(|| DownloadError::fatal("HLS media sequence overflow"))?;
        let encryption = match segment.key.as_ref() {
            Some(key) => encryption_for_key(key, base, registry, sequence, false)?,
            None => None,
        };
        let init = match segment.map.as_ref() {
            Some(map) => {
                let init_url = resolve_reference(base, &map.uri, "initialization segment")?;
                let init_range = map
                    .byte_range
                    .map(|(length, offset)| checked_range(length, offset.unwrap_or(0)))
                    .transpose()?;
                let init_encryption = match map.key.as_ref() {
                    Some(key) => encryption_for_key(key, base, registry, 0, true)?,
                    None => None,
                };
                let piece = Piece { url: init_url, range: init_range, encryption: init_encryption };
                let position = inits.iter().position(|candidate: &Piece| *candidate == piece);
                Some(position.unwrap_or_else(|| {
                    inits.push(piece);
                    inits.len() - 1
                }))
            }
            None => None,
        };
        segments.push(SegmentPlan { piece: Piece { url, range, encryption }, init });
    }
    let container = if segments.iter().any(|segment| segment.init.is_some()) {
        Container::Fmp4
    } else {
        Container::Ts
    };
    let estimated_bytes = estimate_media_bytes(&inits, &segments, bandwidth, duration);
    Ok(MediaPlan { media_url: base.clone(), container, inits, segments, estimated_bytes })
}

fn hash_feed(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn hash_text(hash: &mut u64, text: &str) {
    hash_feed(hash, &(text.len() as u64).to_le_bytes());
    hash_feed(hash, text.as_bytes());
}

fn hash_u64(hash: &mut u64, value: u64) {
    hash_feed(hash, &value.to_le_bytes());
}

fn normalized_url_text(url: &Url) -> String {
    let mut normalized = url.clone();
    normalized.set_query(None);
    normalized.set_fragment(None);
    normalized.to_string()
}

fn hash_url(hash: &mut u64, url: &Url) {
    hash_text(hash, &normalized_url_text(url));
}

fn hash_piece(hash: &mut u64, piece: &Piece, keys: &[KeyPlan]) {
    hash_url(hash, &piece.url);
    match piece.range {
        Some(range) => {
            hash_u64(hash, 1);
            hash_u64(hash, range.start);
            hash_u64(hash, range.end);
        }
        None => hash_u64(hash, 0),
    }
    match piece.encryption.as_ref() {
        Some(encryption) => {
            hash_u64(hash, 1);
            hash_feed(hash, &encryption.iv);
            if let Some(key) = keys.get(encryption.key) {
                hash_url(hash, &key.url);
            }
        }
        None => hash_u64(hash, 0),
    }
}

fn plan_fingerprint(renditions: &[MediaPlan], keys: &[KeyPlan]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    hash_text(&mut hash, "falcon-hls-plan-v2");
    hash_u64(&mut hash, renditions.len() as u64);
    for rendition in renditions {
        hash_url(&mut hash, &rendition.media_url);
        hash_u64(&mut hash, rendition.container as u64);
        hash_u64(&mut hash, rendition.inits.len() as u64);
        for init in &rendition.inits {
            hash_piece(&mut hash, init, keys);
        }
        hash_u64(&mut hash, rendition.segments.len() as u64);
        for segment in &rendition.segments {
            hash_piece(&mut hash, &segment.piece, keys);
            match segment.init {
                Some(index) => hash_u64(&mut hash, index as u64 + 1),
                None => hash_u64(&mut hash, 0),
            }
        }
    }
    format!("{hash:016x}")
}

async fn resolve_media_url(base: &Url, reference: &str, what: &str) -> Result<Url, DownloadError> {
    let target = base
        .join(reference.trim())
        .map_err(|e| DownloadError::fatal(format!("Invalid HLS {what} URL: {e}")))?;
    validate_fetch_url_async(target.as_str()).await.map_err(DownloadError::fatal)
}

async fn resolve_plan(
    ctx: &JobContext,
    url: &str,
    headers: &HttpHeaders,
) -> Result<Plan, DownloadError> {
    let source = validate_fetch_url_async(url).await?;
    let (bytes, base) = fetch_playlist(ctx, &source, &source, headers).await?;
    let mut registry = KeyRegistry::default();
    let mut renditions = Vec::new();
    let mut subtitles = None;
    if playlist_is_master(&bytes) {
        let master = m3u8_rs::parse_master_playlist_res(&bytes)
            .map_err(|_| DownloadError::fatal("Failed to parse m3u8 playlist"))?;
        let variant = pick_best_variant(&master.variants)
            .ok_or_else(|| DownloadError::fatal("No variants found in master playlist"))?;
        let variant_url = resolve_media_url(&base, &variant.uri, "variant").await?;
        let (video_media, video_bytes, video_base) =
            fetch_media_playlist(ctx, &source, &variant_url, headers).await?;
        renditions.push(build_media_plan(
            &video_media,
            &video_base,
            Some(variant.bandwidth),
            &mut registry,
        )?);
        drop(video_bytes);

        let audio_uri = pick_audio_rendition(&master, variant)
            .and_then(|media| media.uri.as_deref())
            .map(str::trim)
            .filter(|uri| !uri.is_empty())
            .map(str::to_string);
        if let Some(audio_uri) = audio_uri {
            let audio_url = resolve_media_url(&base, &audio_uri, "audio rendition").await?;
            let (audio_media, _audio_bytes, audio_base) =
                fetch_media_playlist(ctx, &source, &audio_url, headers).await?;
            renditions.push(build_media_plan(&audio_media, &audio_base, None, &mut registry)?);
        }

        // Subtitle renditions are owned copies, so the master playlist can be
        // dropped here; the segments themselves are fetched after finalize.
        let selected = select_subtitle_renditions(&hls_subtitle_renditions(
            &master,
            variant.subtitles.as_deref(),
        ));
        if !selected.is_empty() {
            subtitles = Some(SubtitlePlan { master_url: base.clone(), renditions: selected });
        }
    } else {
        let media = parse_hls_media(&bytes)?;
        renditions.push(build_media_plan(&media, &base, None, &mut registry)?);
    }

    let keys = registry.keys;
    let estimated_bytes = renditions
        .iter()
        .try_fold(0u64, |total, rendition| total.checked_add(rendition.estimated_bytes?));
    let fingerprint = plan_fingerprint(&renditions, &keys);
    Ok(Plan { source, keys, renditions, estimated_bytes, fingerprint, subtitles })
}

type HostClient = Option<(String, Client)>;

/// Mutex serialising progress adjustments so a rollback cannot clobber the
/// bytes parallel workers added between its read and its write.
type ProgressLock = Arc<std::sync::Mutex<()>>;

/// Live byte accounting for one piece attempt. Dropping before `commit` rolls
/// back exactly what this attempt added, never another worker's bytes.
struct AttemptProgress {
    progress: crate::download::progress::Progress,
    lock: ProgressLock,
    counted: u64,
    committed: bool,
}

impl AttemptProgress {
    fn new(progress: &crate::download::progress::Progress, lock: &ProgressLock) -> Self {
        Self { progress: progress.clone(), lock: lock.clone(), counted: 0, committed: false }
    }

    fn add(&mut self, bytes: u64) {
        let _guard = crate::util::lock_or_recover(&self.lock);
        self.progress.add(bytes);
        self.counted += bytes;
    }

    /// AES padding is network overhead, not output media bytes. Remove it once
    /// decryption proves how much padding the ciphertext carried.
    fn subtract(&mut self, bytes: u64) {
        let bytes = bytes.min(self.counted);
        if bytes == 0 {
            return;
        }
        let _guard = crate::util::lock_or_recover(&self.lock);
        self.progress.set_downloaded(self.progress.downloaded().saturating_sub(bytes));
        self.counted -= bytes;
    }

    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for AttemptProgress {
    fn drop(&mut self) {
        if !self.committed && self.counted > 0 {
            let _guard = crate::util::lock_or_recover(&self.lock);
            self.progress.set_downloaded(self.progress.downloaded().saturating_sub(self.counted));
        }
    }
}

fn check_content_encoding(response: &Response) -> Result<(), DownloadError> {
    let encoded = response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .is_some_and(|value| !value.is_empty() && !value.eq_ignore_ascii_case("identity"));
    if encoded {
        Err(DownloadError::fatal("ERR_CONTENT_ENCODING"))
    } else {
        Ok(())
    }
}

fn validate_response(response: &Response, range: Option<ByteRange>) -> Result<(), DownloadError> {
    let status = response.status();
    if let Some(range) = range {
        if status == StatusCode::PARTIAL_CONTENT {
            return match parse_content_range(response.headers()) {
                Some((start, end, _)) if start == range.start && end == range.end => Ok(()),
                _ => Err(DownloadError::fatal("HLS byte-range response is invalid")),
            };
        }
        if status.is_success() {
            return Err(DownloadError::fatal("HLS byte-range request was not honored"));
        }
        return Err(DownloadError::from_status(status.as_u16()));
    }
    if status.is_success() {
        Ok(())
    } else {
        Err(DownloadError::from_status(status.as_u16()))
    }
}

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// Streaming file-to-file AES-128-CBC decryptor. The final ciphertext block is
/// held back until EOF so PKCS#7 can be validated and stripped; memory stays
/// bounded even though the engine permits very large segments.
async fn decrypt_file_cbc(
    input: &Path,
    output_path: &Path,
    key: &[u8; AES_KEY_BYTES],
    iv: &[u8; AES_BLOCK_BYTES],
) -> Result<u64, DownloadError> {
    let cipher_len = tokio::fs::metadata(input)
        .await
        .map_err(|e| io_error("could not inspect encrypted HLS segment", &e))?
        .len();
    if cipher_len == 0 || cipher_len % AES_BLOCK_BYTES as u64 != 0 {
        return Err(DownloadError::fatal("HLS AES-128 ciphertext length is invalid"));
    }
    let mut input = tokio::fs::File::open(input)
        .await
        .map_err(|e| io_error("could not open encrypted HLS segment", &e))?;
    let mut output = tokio::fs::File::create(output_path)
        .await
        .map_err(|e| io_error("could not create decrypted HLS segment", &e))?;
    let mut decryptor = Aes128CbcDec::new(key.into(), iv.into());
    let mut buffer = vec![0u8; 64 * 1024];
    let mut pending = Vec::with_capacity(64 * 1024 + AES_BLOCK_BYTES);
    let mut written = 0u64;
    loop {
        let read = input
            .read(&mut buffer)
            .await
            .map_err(|e| io_error("could not read encrypted HLS segment", &e))?;
        if read == 0 {
            break;
        }
        pending.extend_from_slice(&buffer[..read]);
        // Keep at least one ciphertext block undecrypted: it may be the padded
        // final block. Everything before it is guaranteed not to carry padding.
        let process_len = (pending.len().saturating_sub(1) / AES_BLOCK_BYTES) * AES_BLOCK_BYTES;
        if process_len == 0 {
            continue;
        }
        let mut head: Vec<u8> = pending.drain(..process_len).collect();
        for chunk in head.as_chunks_mut::<AES_BLOCK_BYTES>().0 {
            decryptor.decrypt_block(chunk.into());
        }
        output
            .write_all(&head)
            .await
            .map_err(|e| io_error("could not write decrypted HLS segment", &e))?;
        written += head.len() as u64;
    }
    if pending.len() != AES_BLOCK_BYTES {
        return Err(DownloadError::fatal("HLS AES-128 ciphertext length is invalid"));
    }
    let block: &mut Block<aes::Aes128> =
        pending.as_mut_slice().try_into().expect("validated final ciphertext block length");
    decryptor.decrypt_block(block);
    let plaintext_len = Pkcs7::unpad(block)
        .map_err(|_| DownloadError::fatal("HLS AES-128 padding is invalid"))?
        .len();
    output
        .write_all(&block[..plaintext_len])
        .await
        .map_err(|e| io_error("could not write decrypted HLS segment", &e))?;
    written += plaintext_len as u64;
    output.flush().await.map_err(|e| io_error("could not flush decrypted HLS segment", &e))?;
    output.sync_all().await.map_err(|e| io_error("could not sync decrypted HLS segment", &e))?;
    Ok(written)
}

/// One attempt: stream a piece into a temp file, validate exact HTTP facts,
/// then decrypt it if needed. Returns the trusted plaintext byte length.
async fn fetch_piece_once(
    ctx: &JobContext,
    source: &Url,
    piece: &Piece,
    headers: &HttpHeaders,
    keys: &[[u8; AES_KEY_BYTES]],
    client: &mut HostClient,
    tmp: &Path,
    encrypted_tmp: &Path,
    progress_lock: &ProgressLock,
) -> Result<u64, DownloadError> {
    let host = piece.url.host_str().unwrap_or_default().to_string();
    let reuse = client.as_ref().filter(|(h, _)| *h == host).map(|(_, c)| c);
    let range_request = piece.range.map_or_else(RangeRequest::default, |range| RangeRequest {
        start: Some(range.start),
        end: Some(range.end),
        if_range: None,
    });
    let Opened { response, final_url, client: used } =
        open(source, &piece.url, headers, &range_request, reuse).await?;
    if final_url.host_str() == Some(host.as_str()) {
        *client = Some((host, used));
    }
    validate_response(&response, piece.range)?;
    check_content_encoding(&response)?;
    let response_length = response.content_length();
    let required_length = piece.range.map(ByteRange::len);
    let byte_limit = required_length.unwrap_or(MAX_SEGMENT_BYTES);
    if byte_limit > MAX_SEGMENT_BYTES {
        return Err(DownloadError::fatal("HLS segment is too large"));
    }
    let encrypted = piece.encryption.is_some();
    let download_tmp = if encrypted { encrypted_tmp } else { tmp };
    let mut file = tokio::fs::File::create(download_tmp)
        .await
        .map_err(|e| io_error("could not create segment file", &e))?;
    let mut counted = AttemptProgress::new(&ctx.progress, progress_lock);
    let mut written = 0u64;
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk =
            chunk.map_err(|e| DownloadError::transient(format!("segment read failed: {e}")))?;
        written += chunk.len() as u64;
        if written > byte_limit {
            return Err(DownloadError::fatal("HLS segment is too large"));
        }
        file.write_all(&chunk).await.map_err(|e| io_error("segment write failed", &e))?;
        counted.add(chunk.len() as u64);
        ctx.limiter.acquire(chunk.len() as u64, &ctx.cancel).await?;
    }
    file.flush().await.map_err(|e| io_error("segment flush failed", &e))?;
    if !encrypted {
        // Resume trusts this file across restarts: it must be on disk, not in cache.
        file.sync_all().await.map_err(|e| io_error("segment sync failed", &e))?;
    }
    drop(file);
    if written == 0 {
        return Err(DownloadError::transient("segment is empty"));
    }
    if response_length.is_some_and(|len| len != written)
        || required_length.is_some_and(|len| len != written)
    {
        return Err(DownloadError::transient("segment is incomplete"));
    }

    if let Some(encryption) = piece.encryption.as_ref() {
        let key = keys
            .get(encryption.key)
            .copied()
            .ok_or_else(|| DownloadError::fatal("HLS encryption key is missing"))?;
        let plaintext = decrypt_file_cbc(encrypted_tmp, tmp, &key, &encryption.iv).await?;
        let _ = tokio::fs::remove_file(encrypted_tmp).await;
        if plaintext == 0 {
            return Err(DownloadError::fatal("HLS segment decrypts to empty media"));
        }
        counted.subtract(written.saturating_sub(plaintext));
        counted.commit();
        return Ok(plaintext);
    }
    counted.commit();
    Ok(written)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PieceKind {
    Init(usize),
    Media(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PieceKey {
    rendition: usize,
    kind: PieceKind,
}

fn rendition_dir(root: &Path, rendition: usize) -> PathBuf {
    match rendition {
        0 => root.to_path_buf(),
        1 => root.join("audio"),
        other => root.join(format!("rendition-{other}")),
    }
}

fn piece_name(plan: &Plan, key: PieceKey) -> String {
    match key.kind {
        PieceKind::Init(index) => format!("init_{index:03}.bin"),
        PieceKind::Media(index) => {
            let fmp4 =
                plan.renditions.get(key.rendition).is_some_and(|r| r.container == Container::Fmp4);
            if fmp4 {
                format!("seg_{index:05}.m4s")
            } else {
                segment_name(index)
            }
        }
    }
}

fn piece_path(root: &Path, plan: &Plan, key: PieceKey) -> Result<PathBuf, String> {
    let dir = rendition_dir(root, key.rendition);
    segment_path(&dir, &piece_name(plan, key))
}

fn piece_for(plan: &Plan, key: PieceKey) -> Result<&Piece, DownloadError> {
    let rendition = plan
        .renditions
        .get(key.rendition)
        .ok_or_else(|| DownloadError::fatal("HLS piece plan is invalid"))?;
    match key.kind {
        PieceKind::Init(index) => rendition
            .inits
            .get(index)
            .ok_or_else(|| DownloadError::fatal("HLS piece plan is invalid")),
        PieceKind::Media(index) => rendition
            .segments
            .get(index)
            .map(|segment| &segment.piece)
            .ok_or_else(|| DownloadError::fatal("HLS piece plan is invalid")),
    }
}

fn piece_keys(plan: &Plan) -> Vec<PieceKey> {
    let mut keys = Vec::new();
    for (rendition, plan) in plan.renditions.iter().enumerate() {
        keys.extend(
            (0..plan.inits.len()).map(|index| PieceKey { rendition, kind: PieceKind::Init(index) }),
        );
    }
    // Round-robin the media pieces so split audio downloads overlap the video
    // instead of forming a serial tail behind it.
    let max_segments =
        plan.renditions.iter().map(|rendition| rendition.segments.len()).max().unwrap_or(0);
    for index in 0..max_segments {
        for (rendition, plan) in plan.renditions.iter().enumerate() {
            if index < plan.segments.len() {
                keys.push(PieceKey { rendition, kind: PieceKind::Media(index) });
            }
        }
    }
    keys
}

fn len_sidecar_path(path: &Path) -> PathBuf {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("piece");
    path.with_file_name(format!("{name}.len"))
}

/// Write the byte-length sidecar that lets a later run trust the piece.
/// Best effort: without it the piece is simply fetched again on resume.
async fn write_len_sidecar(path: &Path, bytes: u64) {
    let sidecar = len_sidecar_path(path);
    let written = async {
        let mut file = tokio::fs::File::create(&sidecar).await.ok()?;
        file.write_all(bytes.to_string().as_bytes()).await.ok()?;
        file.sync_all().await.ok()
    };
    if written.await.is_none() {
        log::warn!("could not write the length sidecar of HLS piece {}", path.display());
    }
}

async fn fetch_piece(
    ctx: &JobContext,
    plan: &Plan,
    headers: &HttpHeaders,
    keys: &Arc<Vec<[u8; AES_KEY_BYTES]>>,
    client: &mut HostClient,
    key: PieceKey,
    root: &Path,
    progress_lock: &ProgressLock,
) -> Result<u64, DownloadError> {
    let name = piece_name(plan, key);
    let dir = rendition_dir(root, key.rendition);
    let path = dir.join(&name);
    let tmp = dir.join(format!("{name}.tmp"));
    let encrypted_tmp = dir.join(format!("{name}.enc.tmp"));
    let piece = piece_for(plan, key)?.clone();
    let mut cancel = ctx.cancel.clone();
    let mut attempt = 0u32;
    loop {
        ctx.check_cancel()?;
        let result = tokio::select! {
            result = fetch_piece_once(ctx, &plan.source, &piece, headers, keys, client, &tmp, &encrypted_tmp, progress_lock) => result,
            _ = cancelled(&mut cancel) => Err(DownloadError::Cancelled),
        };
        match result {
            Ok(bytes) => {
                tokio::fs::rename(&tmp, &path)
                    .await
                    .map_err(|e| io_error("segment rename failed", &e))?;
                // The sidecar lands after the piece: a crash in between just
                // re-downloads it, never trusts a partial file.
                write_len_sidecar(&path, bytes).await;
                return Ok(bytes);
            }
            Err(error) => {
                // Partial bytes of a failed attempt never reached the counter:
                // AttemptProgress rolled back exactly what this attempt added.
                let _ = tokio::fs::remove_file(&tmp).await;
                let _ = tokio::fs::remove_file(&encrypted_tmp).await;
                attempt += 1;
                match error {
                    DownloadError::Transient(_) if attempt < SEGMENT_ATTEMPTS => {
                        sleep_or_cancel(Duration::from_millis(300u64 << attempt), &ctx.cancel)
                            .await?;
                    }
                    DownloadError::Transient(message) => {
                        return Err(DownloadError::Transient(format!(
                            "segment {name} failed after retries: {message}"
                        )));
                    }
                    other => return Err(other),
                }
            }
        }
    }
}

/// What the cache remembers of the playlist the pieces were fetched for.
/// Per-piece byte lengths live next to the pieces in `*.len` sidecars.
#[derive(serde::Serialize, serde::Deserialize)]
struct CacheManifest {
    version: u32,
    playlist: String,
    fingerprint: String,
    renditions: usize,
    pieces: usize,
    keys: usize,
    /// SHA-256 over the ordered key bytes. Plaintext pieces are encryption-output
    /// artifacts, so the cache identity must change when the key material does.
    #[serde(default)]
    key_fingerprint: String,
}

const CACHE_VERSION: u32 = 3;

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("manifest.json")
}

fn key_material_fingerprint(keys: &[[u8; AES_KEY_BYTES]]) -> String {
    use sha2::Digest;
    use std::fmt::Write as _;

    let mut hasher = sha2::Sha256::new();
    hasher.update(b"falcon-hls-key-material-v1");
    hasher.update((keys.len() as u64).to_le_bytes());
    for key in keys {
        hasher.update(key);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

async fn write_manifest(
    dir: &Path,
    plan: &Plan,
    pieces: usize,
    key_fingerprint: &str,
) -> Result<(), DownloadError> {
    let manifest = CacheManifest {
        version: CACHE_VERSION,
        playlist: plan.renditions.first().map(|r| r.media_url.to_string()).unwrap_or_default(),
        fingerprint: plan.fingerprint.clone(),
        renditions: plan.renditions.len(),
        pieces,
        keys: plan.keys.len(),
        key_fingerprint: key_fingerprint.to_string(),
    };
    let body = serde_json::to_vec(&manifest)
        .map_err(|e| DownloadError::fatal(format!("could not encode cache manifest: {e}")))?;
    let tmp = dir.join("manifest.json.tmp");
    tokio::fs::write(&tmp, body)
        .await
        .map_err(|e| io_error("could not write cache manifest", &e))?;
    tokio::fs::rename(&tmp, manifest_path(dir))
        .await
        .map_err(|e| io_error("could not write cache manifest", &e))?;
    Ok(())
}

/// A cached piece is trusted only when its byte length matches the sidecar
/// written right after it completed — truncated nonzero files are corruption.
async fn trusted_piece_len(root: &Path, plan: &Plan, key: PieceKey) -> Option<u64> {
    let path = piece_path(root, plan, key).ok()?;
    let meta = tokio::fs::metadata(&path).await.ok()?;
    if meta.len() == 0 {
        return None;
    }
    let sidecar = tokio::fs::read(len_sidecar_path(&path)).await.ok()?;
    let recorded = std::str::from_utf8(&sidecar).ok()?.trim().parse::<u64>().ok()?;
    (recorded == meta.len()).then_some(meta.len())
}

/// Key bytes are memory-only. Both the current per-key directory and the
/// legacy single-file name are deleted so upgrades never leave media keys on disk.
async fn remove_legacy_key_caches(dir: &Path) {
    let _ = tokio::fs::remove_dir_all(dir.join("keys")).await;
    let _ = tokio::fs::remove_file(dir.join("key-cache.bin")).await;
}

/// Drop anything a previous run left when playlist addressing or key material
/// no longer matches: mixing pieces of two different cuts produces a broken file.
async fn validate_cache(
    dir: &Path,
    plan: &Plan,
    pieces: usize,
    key_fingerprint: &str,
) -> Result<(), DownloadError> {
    let cached: Option<CacheManifest> = tokio::fs::read(manifest_path(dir))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let matches = cached.as_ref().is_some_and(|manifest| {
        manifest.version == CACHE_VERSION
            && manifest.fingerprint == plan.fingerprint
            && manifest.renditions == plan.renditions.len()
            && manifest.pieces == pieces
            && manifest.keys == plan.keys.len()
            && manifest.key_fingerprint == key_fingerprint
    });
    if matches {
        return Ok(());
    }
    if cached.is_some() {
        log::info!(
            "HLS playlist or key material changed since the cache was written — restarting clean"
        );
    }
    let _ = tokio::fs::remove_dir_all(dir).await;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| io_error("could not create temp folder", &e))?;
    write_manifest(dir, plan, pieces, key_fingerprint).await
}

async fn fetch_key_once(
    ctx: &JobContext,
    source: &Url,
    target: &Url,
    headers: &HttpHeaders,
) -> Result<[u8; AES_KEY_BYTES], DownloadError> {
    let Opened { response, .. } =
        open(source, target, headers, &RangeRequest::default(), None).await?;
    let status = response.status();
    if !status.is_success() {
        return Err(DownloadError::from_status(status.as_u16()));
    }
    check_content_encoding(&response)?;
    if response.content_length().is_some_and(|len| len != AES_KEY_BYTES as u64) {
        return Err(DownloadError::fatal("HLS AES-128 key must be exactly 16 bytes"));
    }
    let mut body = Vec::with_capacity(AES_KEY_BYTES);
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk =
            chunk.map_err(|e| DownloadError::transient(format!("HLS key read failed: {e}")))?;
        if body.len() + chunk.len() > AES_KEY_BYTES {
            return Err(DownloadError::fatal("HLS AES-128 key must be exactly 16 bytes"));
        }
        body.extend_from_slice(&chunk);
        ctx.limiter.acquire(chunk.len() as u64, &ctx.cancel).await?;
    }
    if body.len() != AES_KEY_BYTES {
        return Err(DownloadError::fatal("HLS AES-128 key must be exactly 16 bytes"));
    }
    <[u8; AES_KEY_BYTES]>::try_from(body.as_slice())
        .map_err(|_| DownloadError::fatal("HLS AES-128 key must be exactly 16 bytes"))
}

/// Fetch one key with segment-style transient retries. The fixed 16-byte length
/// is the trust validator; key bytes stay memory-only and are never persisted.
async fn fetch_key(
    ctx: &JobContext,
    plan: &Plan,
    headers: &HttpHeaders,
    index: usize,
) -> Result<[u8; AES_KEY_BYTES], DownloadError> {
    let target =
        &plan.keys.get(index).ok_or_else(|| DownloadError::fatal("HLS key plan is invalid"))?.url;
    let mut cancel = ctx.cancel.clone();
    let mut attempt = 0u32;
    loop {
        ctx.check_cancel()?;
        let result = tokio::select! {
            result = fetch_key_once(ctx, &plan.source, target, headers) => result,
            _ = cancelled(&mut cancel) => Err(DownloadError::Cancelled),
        };
        match result {
            Ok(key) => return Ok(key),
            Err(error) => {
                attempt += 1;
                match error {
                    DownloadError::Transient(_) if attempt < SEGMENT_ATTEMPTS => {
                        sleep_or_cancel(Duration::from_millis(300u64 << attempt), &ctx.cancel)
                            .await?;
                    }
                    DownloadError::Transient(message) => {
                        return Err(DownloadError::Transient(format!(
                            "HLS key {index} failed after retries: {message}"
                        )));
                    }
                    other => return Err(other),
                }
            }
        }
    }
}

/// Fetch every planned key before cache validation. Cached pieces are plaintext
/// output of those keys, so even a fully cached interrupted run must prove the
/// key material still matches before any piece may be trusted.
async fn fetch_keys(
    ctx: &JobContext,
    plan: &Plan,
    headers: &HttpHeaders,
) -> Result<Vec<[u8; AES_KEY_BYTES]>, DownloadError> {
    let mut keys = Vec::with_capacity(plan.keys.len());
    for index in 0..plan.keys.len() {
        keys.push(fetch_key(ctx, plan, headers, index).await?);
    }
    Ok(keys)
}

/// Fetch every missing init/media piece of all selected renditions.
async fn download_pieces(
    ctx: &JobContext,
    plan: &Plan,
    headers: &HttpHeaders,
    dir: &Path,
) -> Result<(), DownloadError> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| io_error("could not create temp folder", &e))?;
    let keys = piece_keys(plan);
    remove_legacy_key_caches(dir).await;
    let key_material = Arc::new(fetch_keys(ctx, plan, headers).await?);
    let key_fingerprint = key_material_fingerprint(&key_material);
    validate_cache(dir, plan, keys.len(), &key_fingerprint).await?;
    for rendition in 0..plan.renditions.len() {
        tokio::fs::create_dir_all(rendition_dir(dir, rendition))
            .await
            .map_err(|e| io_error("could not create HLS rendition folder", &e))?;
    }

    let count = keys.len();
    let mut have_bytes = 0u64;
    let mut missing = Vec::new();
    for key in keys {
        if let Some(bytes) = trusted_piece_len(dir, plan, key).await {
            have_bytes += bytes;
        } else {
            missing.push(key);
            if let Ok(path) = piece_path(dir, plan, key) {
                let name = piece_name(plan, key);
                let artifact_dir = rendition_dir(dir, key.rendition);
                let _ = tokio::fs::remove_file(&path).await;
                let _ = tokio::fs::remove_file(len_sidecar_path(&path)).await;
                let _ = tokio::fs::remove_file(artifact_dir.join(format!("{name}.tmp"))).await;
                let _ = tokio::fs::remove_file(artifact_dir.join(format!("{name}.enc.tmp"))).await;
            }
        }
    }
    let done_pieces = Arc::new(AtomicU64::new((count - missing.len()) as u64));
    let done_bytes = Arc::new(AtomicU64::new(have_bytes));
    let estimate = plan.estimated_bytes.unwrap_or(0);
    ctx.progress.set_downloaded(have_bytes);
    ctx.progress.set_total(estimate.max(have_bytes));
    let connections = headers.max_connections.clamp(1, 16).min(missing.len().max(1));
    ctx.progress.set_connections(connections as u32);

    let queue = Arc::new(std::sync::Mutex::new(missing.into_iter().collect::<VecDeque<_>>()));
    let progress_lock: ProgressLock = Arc::new(std::sync::Mutex::new(()));
    let workers = (0..connections).map(|_| {
        let queue = queue.clone();
        let done_pieces = done_pieces.clone();
        let done_bytes = done_bytes.clone();
        let progress_lock = progress_lock.clone();
        let key_material = key_material.clone();
        async move {
            let mut client: HostClient = None;
            loop {
                let Some(key) = crate::util::lock_or_recover(&queue).pop_front() else {
                    return Ok::<(), DownloadError>(());
                };
                let bytes = fetch_piece(
                    ctx,
                    plan,
                    headers,
                    &key_material,
                    &mut client,
                    key,
                    dir,
                    &progress_lock,
                )
                .await?;
                let pieces = done_pieces.fetch_add(1, Ordering::Relaxed) + 1;
                let total_bytes = done_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
                // Sizes seen so far project the total better than peak BANDWIDTH.
                let projected = total_bytes / pieces * count as u64;
                let blended =
                    if pieces * 10 >= count as u64 { projected } else { estimate.max(projected) };
                ctx.progress.set_total(blended.max(ctx.progress.downloaded()));
            }
        }
    });
    // First error wins; dropping the stream stops the other workers.
    let mut running = stream::iter(workers).buffer_unordered(connections);
    while let Some(result) = running.next().await {
        result?;
    }
    drop(running);
    ctx.check_cancel()?;
    let final_bytes = done_bytes.load(Ordering::Relaxed);
    ctx.progress.set_downloaded(final_bytes);
    ctx.progress.set_total(final_bytes);
    Ok(())
}

fn output_extension(dest: &Path) -> &'static str {
    match dest.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("ts") => "ts",
        _ => "mp4",
    }
}

fn requires_mp4_container(plan: &Plan) -> bool {
    plan.renditions.len() > 1
        || plan.renditions.first().is_some_and(|rendition| rendition.container == Container::Fmp4)
}

/// Playlist names and container extensions the native output no longer
/// produces must not mislabel the bytes. fMP4 and split-audio always land as
/// MP4; plain TS keeps the historical explicit `.ts` passthrough choice.
fn container_corrected_name(dest: &Path, force_mp4: bool) -> Option<String> {
    let ext = dest.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if force_mp4 {
        if ext.as_deref() == Some("mp4") {
            return None;
        }
        let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("download");
        return Some(format!("{stem}.mp4"));
    }
    let ext = ext?;
    if !matches!(ext.as_str(), "m3u8" | "m3u" | "mkv" | "mov") {
        return None;
    }
    let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("download");
    Some(format!("{stem}.{}", output_extension(dest)))
}

/// Expired signed URLs surface as fatal auth errors on pieces. Re-resolving
/// the playlist mints fresh tokens — but only once per run.
fn is_expired_credentials(error: &DownloadError) -> bool {
    matches!(error, DownloadError::Fatal(message) if matches!(message.as_str(), "HTTP 401" | "HTTP 403" | "HTTP 410"))
}

#[derive(Clone, Copy)]
enum MuxOperation {
    Concat,
    RemuxTs,
}

fn remap_mux_result(result: Result<(), RemuxError>) -> Result<(), DownloadError> {
    match result {
        Ok(()) => Ok(()),
        Err(RemuxError::Cancelled) => Err(DownloadError::Cancelled),
        Err(RemuxError::Fatal(message)) => Err(DownloadError::Fatal(message)),
    }
}

async fn run_mux(
    ctx: &JobContext,
    inputs: Vec<PathBuf>,
    output: &Path,
    operation: MuxOperation,
) -> Result<(), DownloadError> {
    let output = output.to_path_buf();
    let cancel = ctx.cancel.clone();
    let result = tokio::task::spawn_blocking(move || {
        let should_cancel = || *cancel.borrow();
        match operation {
            MuxOperation::Concat => mux::concat_ts_files(&inputs, &output, &should_cancel),
            MuxOperation::RemuxTs => mux::remux_ts_files_to_mp4(&inputs, &output, &should_cancel),
        }
    })
    .await
    .map_err(|e| DownloadError::fatal(format!("merge task failed: {e}")))?;
    remap_mux_result(result)
}

/// Assemble MPEG-TS pieces into the selected container. MP4 is remuxed in
/// Rust; an explicit `.ts` destination is a byte-exact concat passthrough.
async fn merge_segments(
    ctx: &JobContext,
    dir: &Path,
    names: &[String],
    output: &Path,
) -> Result<(), DownloadError> {
    let mut inputs = Vec::with_capacity(names.len());
    for name in names {
        inputs.push(segment_path(dir, name).map_err(DownloadError::Fatal)?);
    }
    let operation = if output.extension().and_then(|e| e.to_str()) == Some("ts") {
        MuxOperation::Concat
    } else {
        MuxOperation::RemuxTs
    };
    run_mux(ctx, inputs, output, operation).await
}

fn media_names(plan: &Plan, rendition: usize) -> Vec<String> {
    (0..plan.renditions[rendition].segments.len())
        .map(|index| piece_name(plan, PieceKey { rendition, kind: PieceKind::Media(index) }))
        .collect()
}

/// Ordered fMP4 representation bytes: each init section immediately precedes
/// the first media fragment that uses it; unchanged init sections are not
/// duplicated. The result is a valid fragmented-MP4 byte stream. (Defragmenting
/// it into a progressive MP4 is a possible future enhancement, but players
/// already accept this representation.)
fn fmp4_ordered_inputs(
    root: &Path,
    plan: &Plan,
    rendition: usize,
) -> Result<Vec<PathBuf>, DownloadError> {
    let media = &plan.renditions[rendition];
    let mut inputs = Vec::new();
    let mut active_init = None;
    for (index, segment) in media.segments.iter().enumerate() {
        if segment.init != active_init {
            if let Some(init) = segment.init {
                inputs.push(
                    piece_path(root, plan, PieceKey { rendition, kind: PieceKind::Init(init) })
                        .map_err(DownloadError::Fatal)?,
                );
            }
            active_init = segment.init;
        }
        inputs.push(
            piece_path(root, plan, PieceKey { rendition, kind: PieceKind::Media(index) })
                .map_err(DownloadError::Fatal)?,
        );
    }
    Ok(inputs)
}

async fn concat_fmp4_rendition(
    ctx: &JobContext,
    root: &Path,
    plan: &Plan,
    rendition: usize,
    output: &Path,
) -> Result<(), DownloadError> {
    let inputs = fmp4_ordered_inputs(root, plan, rendition)?;
    run_mux(ctx, inputs, output, MuxOperation::Concat).await
}

/// Render one rendition to MP4 input for the split-audio merger. TS goes
/// through the native remuxer; fMP4 is already MP4 and only needs its ordered
/// representation concat.
async fn render_rendition_to_mp4(
    ctx: &JobContext,
    root: &Path,
    plan: &Plan,
    rendition: usize,
    output: &Path,
) -> Result<(), DownloadError> {
    match plan.renditions[rendition].container {
        Container::Fmp4 => concat_fmp4_rendition(ctx, root, plan, rendition, output).await,
        Container::Ts => {
            let names = media_names(plan, rendition);
            merge_segments(ctx, &rendition_dir(root, rendition), &names, output).await
        }
    }
}

/// Split-audio mux integration seam.
///
/// The download/cache layer intentionally ends here with two complete MP4
/// renderings: `video` (the main rendition) and `audio` (the selected
/// EXT-X-MEDIA rendition). `mux::merge_av_mp4` is wired at this single seam;
/// split audio is fully native.
async fn merge_split_audio_mp4(
    ctx: &JobContext,
    video: &Path,
    audio: &Path,
    output: &Path,
) -> Result<(), DownloadError> {
    let video_path = video.to_path_buf();
    let audio_path = audio.to_path_buf();
    let output_path = output.to_path_buf();
    let cancel = ctx.cancel.clone();
    let result = tokio::task::spawn_blocking(move || {
        let should_cancel = || *cancel.borrow();
        mux::merge_av_mp4(&video_path, &audio_path, &output_path, &should_cancel)
    })
    .await
    .map_err(|e| DownloadError::fatal(format!("HLS split-audio merge task failed: {e}")))?;
    match result {
        Ok(()) => Ok(()),
        Err(MergeError::Cancelled) => Err(DownloadError::Cancelled),
        Err(error) => Err(DownloadError::Fatal(error.to_string())),
    }
}

async fn assemble_output(
    ctx: &JobContext,
    plan: &Plan,
    root: &Path,
    output: &Path,
) -> Result<(), DownloadError> {
    if plan.renditions.len() == 1 {
        return match plan.renditions[0].container {
            Container::Fmp4 => concat_fmp4_rendition(ctx, root, plan, 0, output).await,
            Container::Ts => {
                let names = media_names(plan, 0);
                merge_segments(ctx, root, &names, output).await
            }
        };
    }

    let video_render = root.join("video-render.mp4");
    let audio_render = root.join("audio-render.mp4");
    render_rendition_to_mp4(ctx, root, plan, 0, &video_render).await?;
    render_rendition_to_mp4(ctx, root, plan, 1, &audio_render).await?;
    merge_split_audio_mp4(ctx, &video_render, &audio_render, output).await
}

/// Post-finalize subtitle writes. Every rendition failure is logged and never
/// reclassifies the already completed video as a failure.
async fn write_hls_subtitles(ctx: &JobContext, plan: &Plan, headers: &HttpHeaders, landed: &Path) {
    let Some(subtitles) = &plan.subtitles else {
        return;
    };
    let Some(save_dir) = landed.parent() else {
        return;
    };
    let stem = landed.file_stem().and_then(|stem| stem.to_str()).unwrap_or("download");
    for rendition in &subtitles.renditions {
        match write_hls_subtitle_rendition(
            ctx,
            &plan.source,
            &subtitles.master_url,
            rendition,
            headers,
            save_dir,
            stem,
        )
        .await
        {
            Ok(paths) if !paths.is_empty() => log::info!(
                "download {}: wrote {} subtitle file(s) for rendition '{}'",
                ctx.id,
                paths.len(),
                rendition.name
            ),
            Ok(_) => {}
            Err(error) => log::warn!(
                "download {}: subtitle rendition '{}' failed: {}",
                ctx.id,
                rendition.name,
                error.message()
            ),
        }
    }
}

/// Download an HLS stream to `out_path`. Returns where the file landed.
pub async fn process_hls_stream(
    ctx: &JobContext,
    url: &str,
    out_path: &str,
    headers: HttpHeaders,
) -> Result<PathBuf, DownloadError> {
    let mut dest = PathBuf::from(out_path);
    let _ticker = ctx.progress.start();
    let mut plan = resolve_plan(ctx, url, &headers).await?;
    let dir = hls_session_dir(ctx.id);
    // Signed piece URLs can expire mid-download. Re-resolving the playlist
    // mints fresh tokens — once per run; cache validation decides whether the
    // re-resolved cut still matches the bytes on disk.
    let mut reresolved = false;
    loop {
        match download_pieces(ctx, &plan, &headers, &dir).await {
            Ok(()) => break,
            Err(error) if !reresolved && is_expired_credentials(&error) => {
                reresolved = true;
                log::info!(
                    "download {}: HLS piece URLs rejected ({}), re-resolving the playlist once",
                    ctx.id,
                    error.message()
                );
                plan = resolve_plan(ctx, url, &headers).await?;
            }
            Err(error) => return Err(error),
        }
    }

    ctx.progress.set_merging();
    let force_mp4 = requires_mp4_container(&plan);
    let extension = if force_mp4 { "mp4" } else { output_extension(&dest) };
    let output = dir.join(format!("output.{extension}"));
    assemble_output(ctx, &plan, &dir, &output).await?;
    if let Some(wanted) = container_corrected_name(&dest, force_mp4) {
        dest = ctx.rename(&dest, &wanted);
    }
    let landed = ctx.finalize(&output, &dest).await?;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    write_hls_subtitles(ctx, &plan, &headers, &landed).await;
    Ok(landed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use m3u8_rs::Playlist;
    use std::collections::HashMap;

    #[test]
    fn hls_session_dir_is_per_download() {
        assert_eq!(hls_session_dirname(7), "hls-7.falcondm-temp");
        assert!(hls_session_dir(7).ends_with("downloads_temp/hls-7.falcondm-temp"));
    }

    #[test]
    fn segment_path_rejects_quotes_and_paths() {
        let dir = Path::new("/tmp/falcon-hls");
        assert_eq!(segment_path(dir, "seg_00001.ts").unwrap(), dir.join("seg_00001.ts"));
        assert!(segment_path(dir, "seg_00'01.ts").is_err());
        assert!(segment_path(dir, "../secret.ts").is_err());
        assert!(segment_path(dir, "a/b.ts").is_err());
    }

    fn master(text: &str) -> MasterPlaylist {
        match m3u8_rs::parse_playlist_res(text.as_bytes()).expect("playlist") {
            Playlist::MasterPlaylist(master) => master,
            Playlist::MediaPlaylist(_) => panic!("expected master"),
        }
    }

    fn media(text: &str) -> MediaData {
        parse_hls_media(text.as_bytes()).expect("playlist")
    }

    #[test]
    fn best_variant_ignores_iframe_tracks() {
        assert!(pick_best_variant(&[]).is_none());
        let pl = master(
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nlow.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=2500000\nhigh.m3u8\n\
             #EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=9000000,URI=\"iframe.m3u8\"\n",
        );
        let best = pick_best_variant(&pl.variants).expect("variant");
        assert_eq!(best.bandwidth, 2_500_000);
        assert!(best.uri.contains("high.m3u8"));
    }

    #[test]
    fn split_audio_rendition_is_detected() {
        let split = master(
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"en\",URI=\"audio.m3u8\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=2500000,AUDIO=\"aud\"\nvideo.m3u8\n",
        );
        assert!(pick_audio_rendition(&split, &split.variants[0]).is_some());
        // Same group without a URI = audio is muxed into the variant.
        let muxed = master(
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"en\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=2500000,AUDIO=\"aud\"\nvideo.m3u8\n",
        );
        assert!(pick_audio_rendition(&muxed, &muxed.variants[0]).is_none());
        let wrong_type = master(
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"aud\",NAME=\"subs\",URI=\"subs.m3u8\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=2500000,AUDIO=\"aud\"\nvideo.m3u8\n",
        );
        assert!(pick_audio_rendition(&wrong_type, &wrong_type.variants[0]).is_none());
    }

    #[test]
    fn aes_fmp4_byterange_and_discontinuity_are_native() {
        let plain = media("#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\na.ts\n#EXT-X-ENDLIST\n");
        assert_eq!(unsupported_feature(&plain), None);
        let encrypted = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=AES-128,URI=\"k.key\"\n\
             #EXTINF:4.0,\na.ts\n#EXT-X-ENDLIST\n",
        );
        assert_eq!(unsupported_feature(&encrypted), None);
        let fmp4 = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\"\n\
             #EXTINF:4.0,\na.m4s\n#EXT-X-ENDLIST\n",
        );
        assert_eq!(unsupported_feature(&fmp4), None);
        let ranged = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\n#EXT-X-BYTERANGE:100@0\na.ts\n\
             #EXT-X-ENDLIST\n",
        );
        assert_eq!(unsupported_feature(&ranged), None);
        let discontinuity = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\na.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:4.0,\nb.ts\n#EXT-X-ENDLIST\n",
        );
        assert_eq!(unsupported_feature(&discontinuity), None);
        let sample_aes = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://a\"\n\
             #EXTINF:4.0,\na.ts\n#EXT-X-ENDLIST\n",
        );
        assert_eq!(unsupported_feature(&sample_aes), Some("SAMPLE-AES DRM"));
        let base = Url::parse("https://cdn.example.com/index.m3u8").unwrap();
        let mut registry = KeyRegistry::default();
        assert!(matches!(
            build_media_plan(&sample_aes, &base, None, &mut registry),
            Err(DownloadError::Fatal(ref message)) if message == ERR_HLS_DRM
        ));
        let fairplay_format = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=AES-128,URI=\"skd://a\",KEYFORMAT=\"com.apple.streamingkeydelivery\"\n\
             #EXTINF:4.0,\na.ts\n#EXT-X-ENDLIST\n",
        );
        let mut registry = KeyRegistry::default();
        assert!(matches!(
            build_media_plan(&fairplay_format, &base, None, &mut registry),
            Err(DownloadError::Fatal(ref message)) if message == ERR_HLS_DRM
        ));
    }

    fn fingerprint_for(list: &[&str]) -> String {
        let segments = list
            .iter()
            .map(|url| SegmentPlan {
                piece: Piece { url: Url::parse(url).unwrap(), range: None, encryption: None },
                init: None,
            })
            .collect();
        let rendition = MediaPlan {
            media_url: Url::parse("https://cdn.example.com/index.m3u8").unwrap(),
            container: Container::Ts,
            inits: Vec::new(),
            segments,
            estimated_bytes: None,
        };
        plan_fingerprint(&[rendition], &[])
    }

    #[test]
    fn fingerprint_ignores_query_but_not_paths_order_or_count() {
        let a = fingerprint_for(&[
            "https://cdn.example.com/s/seg0.ts?tok=A",
            "https://cdn.example.com/s/seg1.ts?tok=A",
        ]);
        let rotated = fingerprint_for(&[
            "https://cdn.example.com/s/seg0.ts?tok=B&expires=9",
            "https://cdn.example.com/s/seg1.ts?tok=B&expires=9",
        ]);
        assert_eq!(a, rotated, "signed-URL rotation must not change the fingerprint");
        let recut = fingerprint_for(&[
            "https://cdn.example.com/s/seg0.ts?tok=A",
            "https://cdn.example.com/s/other.ts?tok=A",
        ]);
        assert_ne!(a, recut, "different paths are a different cut");
        let fewer = fingerprint_for(&["https://cdn.example.com/s/seg0.ts?tok=A"]);
        assert_ne!(a, fewer, "segment count matters");
        let swapped = fingerprint_for(&[
            "https://cdn.example.com/s/seg1.ts?tok=A",
            "https://cdn.example.com/s/seg0.ts?tok=A",
        ]);
        assert_ne!(a, swapped, "play order matters");
    }

    #[test]
    fn fingerprint_includes_init_range_and_encryption_facts() {
        let base = Url::parse("https://cdn.example.com/").unwrap();
        let piece =
            |name: &str| Piece { url: base.join(name).unwrap(), range: None, encryption: None };
        let make = |init: Vec<Piece>, segments: Vec<SegmentPlan>| {
            vec![MediaPlan {
                media_url: base.clone(),
                container: Container::Fmp4,
                inits: init,
                segments,
                estimated_bytes: None,
            }]
        };
        let segment = SegmentPlan { piece: piece("seg0.m4s"), init: Some(0) };
        let plain = make(vec![piece("init.mp4")], vec![segment.clone()]);
        let mut ranged_init = piece("init.mp4");
        ranged_init.range = Some(ByteRange { start: 10, end: 99 });
        let ranged = make(vec![ranged_init], vec![segment]);
        assert_ne!(
            plan_fingerprint(&plain, &[]),
            plan_fingerprint(&ranged, &[]),
            "the EXT-X-MAP byte range is part of the resume identity"
        );

        let key_url = Url::parse("https://cdn.example.com/key.bin?token=a").unwrap();
        let mut encrypted_init = piece("init.mp4");
        encrypted_init.encryption = Some(Encryption { key: 0, iv: [7; 16] });
        let encrypted = make(
            vec![encrypted_init],
            vec![SegmentPlan { piece: piece("seg0.m4s"), init: Some(0) }],
        );
        let keys = [KeyPlan { url: key_url }];
        assert_ne!(
            plan_fingerprint(&plain, &[]),
            plan_fingerprint(&encrypted, &keys),
            "encryption changes what cached plaintext means"
        );

        let rotated_keys = [KeyPlan {
            url: Url::parse("https://cdn.example.com/rotated-key.bin?token=a").unwrap(),
        }];
        assert_ne!(
            plan_fingerprint(&encrypted, &keys),
            plan_fingerprint(&encrypted, &rotated_keys),
            "a different key URI is a different encryption identity"
        );
        let mut other_iv_init = piece("init.mp4");
        other_iv_init.encryption = Some(Encryption { key: 0, iv: [8; 16] });
        let other_iv = make(
            vec![other_iv_init],
            vec![SegmentPlan { piece: piece("seg0.m4s"), init: Some(0) }],
        );
        assert_ne!(
            plan_fingerprint(&encrypted, &keys),
            plan_fingerprint(&other_iv, &keys),
            "the CBC IV is part of the plaintext identity"
        );
    }

    #[test]
    fn key_material_fingerprint_changes_with_key_bytes() {
        let one = key_material_fingerprint(&[[0x11; AES_KEY_BYTES]]);
        let two = key_material_fingerprint(&[[0x22; AES_KEY_BYTES]]);
        let repeated = key_material_fingerprint(&[[0x11; AES_KEY_BYTES]]);
        assert_ne!(one, two);
        assert_eq!(one, repeated);
        assert_eq!(one.len(), 64, "SHA-256 hex");
    }

    #[test]
    fn playlist_names_take_the_container_extension() {
        let corrected = |name: &str| container_corrected_name(Path::new(name), false);
        assert_eq!(corrected("/dl/master.m3u8").as_deref(), Some("master.mp4"));
        assert_eq!(corrected("/dl/SHOW.M3U8").as_deref(), Some("SHOW.mp4"));
        assert_eq!(corrected("/dl/a.m3u").as_deref(), Some("a.mp4"));
        assert_eq!(corrected("/dl/video.mkv").as_deref(), Some("video.mp4"));
        assert_eq!(corrected("/dl/video.mov").as_deref(), Some("video.mp4"));
        assert_eq!(corrected("/dl/clip.ts"), None);
        assert_eq!(corrected("/dl/plain"), None);
        assert_eq!(corrected("/dl/movie.mp4"), None);
    }

    #[test]
    fn attempt_progress_rolls_back_exactly_what_it_added() {
        let progress = crate::download::progress::Progress::new(None, None, 1);
        let lock: ProgressLock = Arc::new(std::sync::Mutex::new(()));
        let mut a = AttemptProgress::new(&progress, &lock);
        let mut b = AttemptProgress::new(&progress, &lock);
        a.add(100);
        b.add(50);
        assert_eq!(progress.downloaded(), 150);
        // The failed attempt gives back only its own bytes, never the parallel ones.
        drop(a);
        assert_eq!(progress.downloaded(), 50);
        b.commit();
        assert_eq!(progress.downloaded(), 50, "committed bytes stay counted");
    }

    struct Fixture {
        base: String,
        hits: Arc<std::sync::Mutex<HashMap<String, usize>>>,
    }

    fn requested_range(request: &str, len: usize) -> Option<(usize, usize)> {
        let header = request
            .lines()
            .find(|line| line.get(..6).is_some_and(|name| name.eq_ignore_ascii_case("range:")))?;
        let spec = header.get(6..)?.trim().strip_prefix("bytes=")?;
        let (start, end) = spec.split_once('-')?;
        let start = start.parse::<usize>().ok()?;
        let end = end.parse::<usize>().ok()?.min(len.saturating_sub(1));
        (start <= end && start < len).then_some((start, end))
    }

    /// Static file server. `fail_first` paths answer 503 once. Honors exact
    /// byte ranges so HLS EXT-X-BYTERANGE fixtures exercise 206 validation.
    async fn serve(files: HashMap<String, Vec<u8>>, fail_first: Vec<String>) -> Fixture {
        serve_full(files, fail_first, Vec::new()).await
    }

    /// `serve` plus `fail_always` paths answering 500 on every hit.
    async fn serve_full(
        files: HashMap<String, Vec<u8>>,
        fail_first: Vec<String>,
        fail_always: Vec<String>,
    ) -> Fixture {
        use tokio::io::AsyncReadExt;
        let files = Arc::new(files);
        let fail_first = Arc::new(fail_first);
        let fail_always = Arc::new(fail_always);
        let hits = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits_c = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let (files, fail_first, fail_always, hits) =
                    (files.clone(), fail_first.clone(), fail_always.clone(), hits_c.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .split('?')
                        .next()
                        .unwrap_or("/")
                        .to_string();
                    let seen = {
                        let mut hits = hits.lock().unwrap();
                        let entry = hits.entry(path.clone()).or_insert(0);
                        *entry += 1;
                        *entry
                    };
                    if fail_always.contains(&path) {
                        let _ = sock.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        return;
                    }
                    if seen == 1 && fail_first.contains(&path) {
                        let _ = sock.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        return;
                    }
                    let Some(body) = files.get(&path) else {
                        let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        return;
                    };
                    if let Some((start, end)) = requested_range(&request, body.len()) {
                        let part = &body[start..=end];
                        let head = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n",
                            part.len(),
                            body.len()
                        );
                        let _ = sock.write_all(head.as_bytes()).await;
                        let _ = sock.write_all(part).await;
                        return;
                    }
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(body).await;
                });
            }
        });
        Fixture { base: format!("http://127.0.0.1:{}", addr.port()), hits }
    }

    fn fake_stream() -> HashMap<String, Vec<u8>> {
        let mut files = HashMap::new();
        files.insert(
            "/master.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2500000\nhigh/index.m3u8\n".to_vec(),
        );
        files.insert(
            "/high/index.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\nseg0.ts\n#EXTINF:1.0,\nseg1.ts\n#EXTINF:1.0,\nseg2.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        for i in 0..3 {
            files.insert(format!("/high/seg{i}.ts"), format!("SEGMENT-{i}-DATA").into_bytes());
        }
        files
    }

    fn headers(connections: usize) -> HttpHeaders {
        HttpHeaders { max_connections: connections, ..Default::default() }
    }

    /// Compatibility wrapper for the original single-rendition cache tests.
    async fn download_segments(
        ctx: &JobContext,
        plan: &Plan,
        headers: &HttpHeaders,
        dir: &Path,
    ) -> Result<Vec<String>, DownloadError> {
        download_pieces(ctx, plan, headers, dir).await?;
        Ok(media_names(plan, 0))
    }

    #[tokio::test]
    async fn master_playlist_resolves_best_variant_and_fetches_in_order() {
        let fixture = serve(fake_stream(), vec!["/high/seg1.ts".into()]).await;
        let dir = std::env::temp_dir().join(format!("falcon-hls-{}", uuid::Uuid::new_v4()));
        let (ctx, _tx) = JobContext::for_tests(1);
        let _g = crate::util::allow_loopback_for_tests();
        let plan = resolve_plan(&ctx, &format!("{}/master.m3u8", fixture.base), &headers(3))
            .await
            .unwrap();
        assert_eq!(plan.renditions[0].segments.len(), 3);
        assert!(plan.renditions[0].segments[0].piece.url.as_str().ends_with("/high/seg0.ts"));
        assert_eq!(plan.estimated_bytes, Some(2_500_000 / 8 * 3));

        let names = download_segments(&ctx, &plan, &headers(3), &dir).await.unwrap();
        assert_eq!(names, vec!["seg_00000.ts", "seg_00001.ts", "seg_00002.ts"]);
        let joined: Vec<u8> =
            names.iter().flat_map(|name| std::fs::read(dir.join(name)).unwrap()).collect();
        assert_eq!(joined, b"SEGMENT-0-DATASEGMENT-1-DATASEGMENT-2-DATA");
        // The 503 was retried, and failed bytes were not counted.
        assert_eq!(fixture.hits.lock().unwrap()["/high/seg1.ts"], 2);
        assert_eq!(ctx.progress.downloaded(), joined.len() as u64);
        assert_eq!(ctx.progress.total(), joined.len() as u64);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn resume_skips_complete_segments_and_distrusts_tmp_leftovers() {
        let fixture = serve(fake_stream(), Vec::new()).await;
        let dir = std::env::temp_dir().join(format!("falcon-hls-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (ctx, _tx) = JobContext::for_tests(2);
        let _g = crate::util::allow_loopback_for_tests();
        let plan = resolve_plan(&ctx, &format!("{}/high/index.m3u8", fixture.base), &headers(2))
            .await
            .unwrap();
        // A complete first run leaves the trusted cache: manifest + sidecars.
        download_segments(&ctx, &plan, &headers(2), &dir).await.unwrap();
        // Lose one segment (with its sidecar) and drop a crash leftover instead.
        std::fs::remove_file(dir.join("seg_00001.ts")).unwrap();
        std::fs::remove_file(dir.join("seg_00001.ts.len")).unwrap();
        std::fs::write(dir.join("seg_00001.ts.tmp"), b"SEGM").unwrap();
        let (ctx2, _tx2) = JobContext::for_tests(5);
        download_segments(&ctx2, &plan, &headers(2), &dir).await.unwrap();
        let hits = fixture.hits.lock().unwrap().clone();
        assert_eq!(hits["/high/seg0.ts"], 1, "finished segment must not be re-fetched");
        assert_eq!(hits["/high/seg1.ts"], 2, "the lost segment is fetched exactly once more");
        assert_eq!(hits["/high/seg2.ts"], 1, "finished segment must not be re-fetched");
        assert_eq!(std::fs::read(dir.join("seg_00001.ts")).unwrap(), b"SEGMENT-1-DATA");
        assert!(!dir.join("seg_00001.ts.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn missing_segment_fails_without_leaving_a_trusted_file() {
        let mut files = fake_stream();
        files.remove("/high/seg2.ts");
        let fixture = serve(files, Vec::new()).await;
        let dir = std::env::temp_dir().join(format!("falcon-hls-{}", uuid::Uuid::new_v4()));
        let (ctx, _tx) = JobContext::for_tests(3);
        let _g = crate::util::allow_loopback_for_tests();
        let plan = resolve_plan(&ctx, &format!("{}/high/index.m3u8", fixture.base), &headers(1))
            .await
            .unwrap();
        let result = download_segments(&ctx, &plan, &headers(1), &dir).await;
        assert!(matches!(result, Err(DownloadError::Fatal(_))), "{result:?}");
        assert!(!dir.join("seg_00002.ts").exists());
        assert!(!dir.join("seg_00002.ts.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn live_is_refused_and_aes128_is_native() {
        let mut files = HashMap::new();
        files.insert(
            "/live.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\na.ts\n".to_vec(),
        );
        files.insert(
            "/enc.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\"\n#EXTINF:2.0,\na.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        let fixture = serve(files, Vec::new()).await;
        let (ctx, _tx) = JobContext::for_tests(4);
        let _g = crate::util::allow_loopback_for_tests();
        let live = resolve_plan(&ctx, &format!("{}/live.m3u8", fixture.base), &headers(1)).await;
        assert!(matches!(live, Err(DownloadError::Fatal(ref m)) if m == "ERR_HLS_LIVE"));
        let enc = resolve_plan(&ctx, &format!("{}/enc.m3u8", fixture.base), &headers(1)).await;
        let plan = enc.expect("AES-128 playlists are native");
        assert_eq!(plan.keys.len(), 1);
        assert!(plan.renditions[0].segments[0].piece.encryption.is_some());
    }

    /// Static file server whose contents the test can mutate mid-flight.
    async fn serve_mutable(
        files: HashMap<String, Vec<u8>>,
    ) -> (Fixture, Arc<std::sync::Mutex<HashMap<String, Vec<u8>>>>) {
        use tokio::io::AsyncReadExt;
        let files = Arc::new(std::sync::Mutex::new(files));
        let hits = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits_c = hits.clone();
        let files_c = files.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let (files, hits) = (files_c.clone(), hits_c.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .split('?')
                        .next()
                        .unwrap_or("/")
                        .to_string();
                    {
                        let mut hits = hits.lock().unwrap();
                        *hits.entry(path.clone()).or_insert(0) += 1;
                    }
                    let body = files.lock().unwrap().get(&path).cloned();
                    let Some(body) = body else {
                        let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        return;
                    };
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                });
            }
        });
        (Fixture { base: format!("http://127.0.0.1:{}", addr.port()), hits }, files)
    }

    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("falcon-hls-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A cache written for one playlist must never mix with a re-cut one.
    #[tokio::test]
    async fn mutated_playlist_invalidates_cache() {
        let mut files = HashMap::new();
        files.insert(
            "/index.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\nseg0.ts\n#EXTINF:1.0,\nseg1.ts\n#EXTINF:1.0,\nseg2.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        for i in 0..3 {
            files.insert(format!("/seg{i}.ts"), format!("SEGMENT-{i}-DATA").into_bytes());
        }
        let (fixture, files) = serve_mutable(files).await;
        let dir = fresh_dir("mutate");
        let _g = crate::util::allow_loopback_for_tests();
        let (ctx, _tx) = JobContext::for_tests(20);
        let plan =
            resolve_plan(&ctx, &format!("{}/index.m3u8", fixture.base), &headers(2)).await.unwrap();
        download_segments(&ctx, &plan, &headers(2), &dir).await.unwrap();

        // The CDN re-cuts the stream: new segment names, one segment less.
        {
            let mut files = files.lock().unwrap();
            files.insert(
                "/index.m3u8".to_string(),
                b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\nnew0.ts\n#EXTINF:1.0,\nnew1.ts\n#EXT-X-ENDLIST\n".to_vec(),
            );
            files.insert("/new0.ts".to_string(), b"NEW-0-DATA".to_vec());
            files.insert("/new1.ts".to_string(), b"NEW-1-DATA".to_vec());
        }
        let (ctx2, _tx2) = JobContext::for_tests(21);
        let plan2 = resolve_plan(&ctx2, &format!("{}/index.m3u8", fixture.base), &headers(2))
            .await
            .unwrap();
        let names = download_segments(&ctx2, &plan2, &headers(2), &dir).await.unwrap();
        assert_eq!(names.len(), 2);
        let joined: Vec<u8> =
            names.iter().flat_map(|name| std::fs::read(dir.join(name)).unwrap()).collect();
        assert_eq!(joined, b"NEW-0-DATANEW-1-DATA", "no stale bytes may survive a re-cut");
        assert!(!dir.join("seg_00002.ts").exists(), "orphaned segment of the old cut is removed");
        assert!(!dir.join("seg_00003.ts").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Resume must trust only byte-exact segments; a truncated nonzero file is
    /// fetched again instead of silently corrupting the merge.
    #[tokio::test]
    async fn resume_detects_truncated_segment_and_keeps_valid_cache() {
        let fixture = serve(fake_stream(), Vec::new()).await;
        let dir = fresh_dir("trunc");
        let _g = crate::util::allow_loopback_for_tests();
        let (ctx, _tx) = JobContext::for_tests(22);
        let plan = resolve_plan(&ctx, &format!("{}/high/index.m3u8", fixture.base), &headers(2))
            .await
            .unwrap();
        download_segments(&ctx, &plan, &headers(2), &dir).await.unwrap();
        assert_eq!(fixture.hits.lock().unwrap()["/high/seg0.ts"], 1);

        // Power loss after the rename: the file is nonzero but short.
        let path = dir.join("seg_00001.ts");
        let data = std::fs::read(&path).unwrap();
        std::fs::write(&path, &data[..data.len() - 3]).unwrap();

        let (ctx2, _tx2) = JobContext::for_tests(23);
        download_segments(&ctx2, &plan, &headers(2), &dir).await.unwrap();
        let hits = fixture.hits.lock().unwrap().clone();
        assert_eq!(hits["/high/seg0.ts"], 1, "intact segment stays cached");
        assert_eq!(hits["/high/seg2.ts"], 1, "intact segment stays cached");
        assert_eq!(hits["/high/seg1.ts"], 2, "truncated segment is fetched again");
        assert_eq!(std::fs::read(dir.join("seg_00001.ts")).unwrap(), b"SEGMENT-1-DATA");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn discontinuity_playlist_is_native() {
        let discontinuity = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\na.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:4.0,\nb.ts\n#EXT-X-ENDLIST\n",
        );
        assert_eq!(unsupported_feature(&discontinuity), None);
        let mut files = HashMap::new();
        files.insert(
            "/disc.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\na.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:2.0,\nb.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        let fixture = serve(files, Vec::new()).await;
        let (ctx, _tx) = JobContext::for_tests(24);
        let _g = crate::util::allow_loopback_for_tests();
        let result = resolve_plan(&ctx, &format!("{}/disc.m3u8", fixture.base), &headers(1)).await;
        assert!(result.is_ok(), "discontinuity playlists stay native");
    }

    /// Copies the pre-generated HLS fixture parts (a few seconds of MPEG-TS).
    fn make_hls_parts(src: &Path) -> Option<usize> {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls");
        for name in ["index.m3u8", "part0.ts", "part1.ts", "part2.ts"] {
            std::fs::copy(fixture.join(name), src.join(name)).unwrap();
        }
        Some(3)
    }

    fn load_parts(src: &Path) -> HashMap<String, Vec<u8>> {
        let mut files = HashMap::new();
        for entry in std::fs::read_dir(src).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            files.insert(format!("/{name}"), std::fs::read(entry.path()).unwrap());
        }
        files
    }

    /// Signed-URL server: the token handed out in the playlist expires after
    /// the first segment it protects. The next playlist fetch mints a fresh
    /// token (and optionally re-cut segment names).
    struct Rotating {
        base: String,
        hits: Arc<std::sync::Mutex<HashMap<String, usize>>>,
    }

    async fn serve_signed_segments(segments: Vec<Vec<u8>>, recut: bool) -> Rotating {
        use tokio::io::AsyncReadExt;
        let hits = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
        let playlist_fetches = Arc::new(AtomicU64::new(0));
        let token_a_dead = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits_c = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let (hits, fetches, dead, segments) = (
                    hits_c.clone(),
                    playlist_fetches.clone(),
                    token_a_dead.clone(),
                    segments.clone(),
                );
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let target = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    {
                        let mut hits = hits.lock().unwrap();
                        *hits.entry(target.clone()).or_insert(0) += 1;
                    }
                    let path = target.split('?').next().unwrap_or("/");
                    let token =
                        target.split('?').nth(1).and_then(|q| q.strip_prefix("tok=")).unwrap_or("");
                    let reply = |status: &str, body: &[u8]| {
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes()
                        .into_iter()
                        .chain(body.iter().copied())
                        .collect::<Vec<u8>>()
                    };
                    if path == "/index.m3u8" {
                        let fetch = fetches.fetch_add(1, Ordering::Relaxed) + 1;
                        let (tok, prefix) = if fetch == 1 {
                            ("A", "seg")
                        } else if recut {
                            ("B", "cut")
                        } else {
                            ("B", "seg")
                        };
                        let body = format!(
                            "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\n{prefix}0.ts?tok={tok}\n#EXTINF:1.0,\n{prefix}1.ts?tok={tok}\n#EXTINF:1.0,\n{prefix}2.ts?tok={tok}\n#EXT-X-ENDLIST\n"
                        );
                        let _ = sock.write_all(&reply("200 OK", body.as_bytes())).await;
                        return;
                    }
                    let index = path
                        .trim_start_matches('/')
                        .trim_start_matches("seg")
                        .trim_start_matches("cut")
                        .trim_end_matches(".ts")
                        .to_string();
                    let Ok(index) = index.parse::<usize>() else {
                        let _ = sock.write_all(&reply("404 Not Found", b"")).await;
                        return;
                    };
                    let Some(segment) = segments.get(index) else {
                        let _ = sock.write_all(&reply("404 Not Found", b"")).await;
                        return;
                    };
                    let forbidden = match token {
                        "A" if dead.load(Ordering::Relaxed) => true,
                        "A" => {
                            // The first successful segment burns the token.
                            dead.store(true, Ordering::Relaxed);
                            false
                        }
                        "B" => false,
                        _ => true,
                    };
                    if forbidden {
                        let _ = sock.write_all(&reply("403 Forbidden", b"")).await;
                    } else {
                        let _ = sock.write_all(&reply("200 OK", segment)).await;
                    }
                });
            }
        });
        Rotating { base: format!("http://127.0.0.1:{}", addr.port()), hits }
    }

    fn signed_fixture_parts() -> (PathBuf, Vec<Vec<u8>>) {
        let src = std::env::temp_dir().join(format!("falcon-hls-src-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&src).unwrap();
        assert_eq!(make_hls_parts(&src), Some(3));
        let bytes =
            (0..3).map(|i| std::fs::read(src.join(format!("part{i}.ts"))).unwrap()).collect();
        (src, bytes)
    }

    /// Signed URLs expire mid-download: the engine re-resolves the playlist
    /// once and continues with fresh URLs, reusing the valid cache.
    #[tokio::test]
    async fn expired_signed_urls_reresolve_once_and_keep_cache() {
        let (src, segment) = signed_fixture_parts();
        let fixture = serve_signed_segments(segment, false).await;
        let out_dir = fresh_dir("signed-out");
        let dest = out_dir.join("clip.mp4");
        let id = 910_000 + i64::from(std::process::id());
        remove_artifacts(id);
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(1),
        )
        .await
        .unwrap();
        assert_eq!(landed, dest.canonicalize().unwrap());
        let hits = fixture.hits.lock().unwrap().clone();
        assert_eq!(hits["/seg0.ts?tok=A"], 1, "first segment succeeds before expiry");
        assert_eq!(hits["/seg1.ts?tok=A"], 1, "the expired token is rejected once");
        assert_eq!(hits["/seg1.ts?tok=B"], 1, "fresh token fetches the rest");
        assert_eq!(hits["/seg2.ts?tok=B"], 1);
        assert!(
            !hits.contains_key("/seg0.ts?tok=B"),
            "a cache-valid segment is not fetched again after re-signing"
        );
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(out_dir);
        remove_artifacts(id);
    }

    /// Server whose segments never authorize: every request is a 403, however
    /// fresh the token. Guards the re-resolve loop.
    async fn serve_always_forbidden() -> Rotating {
        use tokio::io::AsyncReadExt;
        let hits = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits_c = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let hits = hits_c.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let target = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    {
                        let mut hits = hits.lock().unwrap();
                        *hits.entry(target.clone()).or_insert(0) += 1;
                    }
                    let path = target.split('?').next().unwrap_or("/");
                    if path == "/index.m3u8" {
                        let body = b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\nseg0.ts\n#EXT-X-ENDLIST\n";
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(head.as_bytes()).await;
                        let _ = sock.write_all(body).await;
                    } else {
                        let _ = sock
                            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .await;
                    }
                });
            }
        });
        Rotating { base: format!("http://127.0.0.1:{}", addr.port()), hits }
    }

    /// One re-resolve per run: permanently forbidden segments end fatal,
    /// not in a loop of playlist fetches.
    #[tokio::test]
    async fn permanently_forbidden_segments_fail_after_one_reresolve() {
        let fixture = serve_always_forbidden().await;
        let out_dir = fresh_dir("forbidden-out");
        let dest = out_dir.join("clip.mp4");
        let id = 940_000 + i64::from(std::process::id());
        remove_artifacts(id);
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let result = process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(1),
        )
        .await;
        assert!(
            matches!(result, Err(DownloadError::Fatal(ref m)) if m == "HTTP 403"),
            "{result:?}"
        );
        let hits = fixture.hits.lock().unwrap().clone();
        assert_eq!(
            hits["/index.m3u8"], 2,
            "exactly one re-resolution: the initial fetch plus one retry"
        );
        let _ = std::fs::remove_dir_all(out_dir);
        remove_artifacts(id);
    }

    /// When the re-resolved playlist is a different cut, the cache cannot be
    /// trusted: everything restarts against the new segments.
    #[tokio::test]
    async fn reresolve_with_recut_playlist_restarts_clean() {
        let (src, segment) = signed_fixture_parts();
        let fixture = serve_signed_segments(segment, true).await;
        let out_dir = fresh_dir("signed-recut");
        let dest = out_dir.join("clip.mp4");
        let id = 920_000 + i64::from(std::process::id());
        remove_artifacts(id);
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(1),
        )
        .await
        .unwrap();
        let hits = fixture.hits.lock().unwrap().clone();
        assert_eq!(hits["/seg0.ts?tok=A"], 1);
        for i in 0..3 {
            assert_eq!(
                hits[&format!("/cut{i}.ts?tok=B")],
                1,
                "re-cut segments are all fetched once"
            );
        }
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(out_dir);
        remove_artifacts(id);
    }

    /// A `.m3u8` output name is corrected to the produced container.
    #[tokio::test]
    async fn m3u8_destination_is_renamed_to_the_container() {
        let src = std::env::temp_dir().join(format!("falcon-hls-src-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&src).unwrap();
        assert_eq!(make_hls_parts(&src), Some(3));
        let fixture = serve(load_parts(&src), Vec::new()).await;
        let out_dir = fresh_dir("rename-out");
        let dest = out_dir.join("master.m3u8");
        let id = 930_000 + i64::from(std::process::id());
        remove_artifacts(id);
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(3),
        )
        .await
        .unwrap();
        assert_eq!(
            landed.file_name().and_then(|n| n.to_str()),
            Some("master.mp4"),
            "the container extension wins over the playlist name"
        );
        let bytes = std::fs::read(&landed).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp", "output must be an MP4 container");
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(out_dir);
        remove_artifacts(id);
    }

    #[tokio::test]
    async fn failed_merge_deletes_partial_output() {
        let dir = fresh_dir("failmerge");
        let name = segment_name(0);
        std::fs::write(dir.join(&name), b"not an mpeg-ts stream").unwrap();
        let (ctx, _tx) = JobContext::for_tests(31);
        let output = dir.join("output.mp4");
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            merge_segments(&ctx, &dir, &[name], &output),
        )
        .await
        .expect("merge must not hang");
        assert!(result.is_err(), "invalid input fails the merge");
        assert!(!output.exists(), "partial output is removed after a failed merge");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn cancelled_merge_leaves_no_partial_output() {
        let dir = fresh_dir("cancelmerge");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls/part0.ts");
        let name = segment_name(0);
        std::fs::copy(source, dir.join(&name)).unwrap();
        let (ctx, tx) = JobContext::for_tests(32);
        tx.send(true).unwrap();
        let output = dir.join("output.mp4");
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            merge_segments(&ctx, &dir, &[name], &output),
        )
        .await
        .expect("cancelled merge must not hang");
        assert!(matches!(result, Err(DownloadError::Cancelled)), "{result:?}");
        assert!(!output.exists(), "a cancelled merge leaves no partial output");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn ts_output_is_concat_passthrough() {
        let dir = fresh_dir("tsmerge");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls");
        let mut names = Vec::new();
        let mut expected = Vec::new();
        for i in 0..2 {
            let bytes = std::fs::read(fixture.join(format!("part{i}.ts"))).unwrap();
            expected.extend_from_slice(&bytes);
            let name = segment_name(i);
            std::fs::write(dir.join(&name), bytes).unwrap();
            names.push(name);
        }
        let (ctx, _tx) = JobContext::for_tests(33);
        let output = dir.join("output.ts");
        merge_segments(&ctx, &dir, &names, &output).await.unwrap();
        assert_eq!(std::fs::read(output).unwrap(), expected);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Real MPEG-TS segments → one playable MP4 without any external tool.
    #[tokio::test]
    async fn end_to_end_remux_produces_an_mp4() {
        let src = std::env::temp_dir().join(format!("falcon-hls-src-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&src).unwrap();
        assert_eq!(make_hls_parts(&src), Some(3));
        let fixture = serve(load_parts(&src), Vec::new()).await;

        let out_dir = std::env::temp_dir().join(format!("falcon-hls-out-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&out_dir).unwrap();
        let dest = out_dir.join("clip.mp4");
        let id = 900_000 + i64::from(std::process::id());
        remove_artifacts(id);
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(3),
        )
        .await
        .unwrap();
        assert_eq!(landed, dest.canonicalize().unwrap());
        let bytes = std::fs::read(&dest).unwrap();
        assert!(bytes.len() > 1000);
        assert_eq!(&bytes[4..8], b"ftyp", "output must be an MP4 container");
        assert!(!hls_session_dir(id).exists(), "segment cache is removed after success");
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(out_dir);
    }

    fn ts_fixture_bytes(count: usize) -> Vec<Vec<u8>> {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls");
        (0..count).map(|i| std::fs::read(fixture.join(format!("part{i}.ts"))).unwrap()).collect()
    }

    fn encrypt_aes128(key: &[u8; 16], iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
        use aes::cipher::BlockModeEncrypt;
        let padding = AES_BLOCK_BYTES - plaintext.len() % AES_BLOCK_BYTES;
        let mut data = plaintext.to_vec();
        data.extend(std::iter::repeat_n(padding as u8, padding));
        let mut enc = cbc::Encryptor::<aes::Aes128>::new(key.into(), iv.into());
        for chunk in data.as_chunks_mut::<AES_BLOCK_BYTES>().0 {
            enc.encrypt_block(chunk.into());
        }
        data
    }

    fn test_out(tag: &str) -> (PathBuf, i64) {
        // Unique per test within the process: two tags of equal string length
        // must never share an HLS session dir through identical ids.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let out = fresh_dir(tag);
        let id =
            950_000 + i64::from(std::process::id()) + NEXT.fetch_add(1, Ordering::Relaxed) as i64;
        (out, id)
    }

    /// AES-128 without IV derives one IV per segment from its media sequence.
    /// The key is fetched once, every cached piece is plaintext, and `.ts`
    /// output is byte-exactly the original MPEG-TS concatenation.
    #[tokio::test]
    async fn aes128_segments_decrypt_byte_exact_and_fetch_key_once() {
        let plain = ts_fixture_bytes(2);
        let key = [0x42u8; 16];
        let mut files = HashMap::new();
        files.insert("/key.bin".to_string(), key.to_vec());
        let playlist = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:7\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n#EXTINF:2.0,\nseg0.ts\n#EXTINF:2.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
        files.insert("/index.m3u8".to_string(), playlist.as_bytes().to_vec());
        for (i, part) in plain.iter().enumerate() {
            let iv = sequence_iv(7 + i as u64);
            files.insert(format!("/seg{i}.ts"), encrypt_aes128(&key, &iv, part));
        }
        let fixture = serve(files, Vec::new()).await;
        let (out_dir, id) = test_out("aes-out");
        remove_artifacts(id);
        let dest = out_dir.join("clip.ts");
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(3),
        )
        .await
        .unwrap();
        let expected: Vec<u8> = plain.concat();
        assert_eq!(std::fs::read(&landed).unwrap(), expected);
        assert_eq!(fixture.hits.lock().unwrap()["/key.bin"], 1, "one key fetch for two segments");
        remove_artifacts(id);
        let _ = std::fs::remove_dir_all(out_dir);
    }

    #[test]
    fn explicit_iv_parses_as_big_endian_and_sequence_iv_is_used() {
        assert_eq!(parse_iv("0x0000000000000000000000000000002a").unwrap(), sequence_iv(42));
        assert_eq!(parse_iv("0X2a").unwrap(), sequence_iv(42));
        assert!(parse_iv("2a").is_err());
        assert!(parse_iv("0xzz").is_err());
    }

    /// Ranges are exact HTTP Range requests. The final segment deliberately
    /// omits `@offset`, exercising RFC continuation after the previous range.
    #[tokio::test]
    async fn byterange_segments_download_exact_subranges_in_order() {
        let plain = ts_fixture_bytes(3);
        let whole = plain.concat();
        let lengths: Vec<usize> = plain.iter().map(Vec::len).collect();
        let starts = [0, lengths[0], lengths[0] + lengths[1]];
        let playlist = format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:{}@{}\nall.ts\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:{}@{}\nall.ts\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:{}\nall.ts\n#EXT-X-ENDLIST\n",
            lengths[0], starts[0], lengths[1], starts[1], lengths[2]
        );
        let mut files = HashMap::new();
        files.insert("/index.m3u8".to_string(), playlist.into_bytes());
        files.insert("/all.ts".to_string(), whole.clone());
        let fixture = serve(files, Vec::new()).await;
        let (out_dir, id) = test_out("range-out");
        remove_artifacts(id);
        let dest = out_dir.join("clip.ts");
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let plan =
            resolve_plan(&ctx, &format!("{}/index.m3u8", fixture.base), &headers(2)).await.unwrap();
        assert_eq!(plan.estimated_bytes, Some(whole.len() as u64));
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(2),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&landed).unwrap(), whole);
        remove_artifacts(id);
        let _ = std::fs::remove_dir_all(out_dir);
    }

    /// fMP4 output is already MP4: init + media fragments, byte-for-byte.
    /// Even a `.ts` request is corrected because TS bytes would be a lie.
    #[tokio::test]
    async fn fmp4_playlist_writes_init_plus_fragments_as_final_mp4() {
        let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/dash");
        let init = std::fs::read(fixture_dir.join("vinit.mp4")).unwrap();
        let fragments: Vec<Vec<u8>> = ["v1.m4s", "v2.m4s"]
            .iter()
            .map(|name| std::fs::read(fixture_dir.join(name)).unwrap())
            .collect();
        let playlist = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:2.0,\nfrag1.m4s\n#EXTINF:2.0,\nfrag2.m4s\n#EXT-X-ENDLIST\n";
        let mut files = HashMap::new();
        files.insert("/index.m3u8".to_string(), playlist.as_bytes().to_vec());
        files.insert("/init.mp4".to_string(), init.clone());
        files.insert("/frag1.m4s".to_string(), fragments[0].clone());
        files.insert("/frag2.m4s".to_string(), fragments[1].clone());
        let fixture = serve(files, Vec::new()).await;
        let (out_dir, id) = test_out("fmp4-out");
        remove_artifacts(id);
        let requested = out_dir.join("clip.ts");
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            requested.to_str().unwrap(),
            headers(3),
        )
        .await
        .unwrap();
        assert_eq!(landed.file_name().and_then(|n| n.to_str()), Some("clip.mp4"));
        let expected: Vec<u8> = init.into_iter().chain(fragments.concat()).collect();
        assert_eq!(std::fs::read(&landed).unwrap(), expected);
        remove_artifacts(id);
        let _ = std::fs::remove_dir_all(out_dir);
    }

    #[test]
    fn aes_key_rotation_plans_distinct_key_cache_entries() {
        let playlist = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:3\n\
             #EXT-X-KEY:METHOD=AES-128,URI=\"one.key\"\n#EXTINF:4.0,\na.ts\n\
             #EXT-X-KEY:METHOD=AES-128,URI=\"two.key\"\n#EXTINF:4.0,\nb.ts\n#EXT-X-ENDLIST\n",
        );
        let base = Url::parse("https://cdn.example.com/media/index.m3u8").unwrap();
        let mut registry = KeyRegistry::default();
        let plan = build_media_plan(&playlist, &base, None, &mut registry).unwrap();
        assert_eq!(registry.keys.len(), 2);
        assert_eq!(plan.segments[0].piece.encryption.as_ref().unwrap().key, 0);
        assert_eq!(plan.segments[1].piece.encryption.as_ref().unwrap().key, 1);
        assert_eq!(plan.segments[0].piece.encryption.as_ref().unwrap().iv, sequence_iv(3));
        assert_eq!(plan.segments[1].piece.encryption.as_ref().unwrap().iv, sequence_iv(4));
    }

    /// A key declared before EXT-X-MAP encrypts the init section too. The cached
    /// init artifact must therefore be plaintext, not the served ciphertext.
    #[tokio::test]
    async fn encrypted_fmp4_init_is_decrypted_before_caching() {
        let key = [0xabu8; 16];
        let iv = [0xcdu8; 16];
        let iv_hex = "0xcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
        let init_plain = b"ftypisobm000fake-init";
        let frag_plain = b"moofmdatfake-fragment";
        let playlist = format!(
            "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:1\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV={iv_hex}\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:1.0,\nfrag.m4s\n#EXT-X-ENDLIST\n"
        );
        let mut files = HashMap::new();
        files.insert("/index.m3u8".to_string(), playlist.into_bytes());
        files.insert("/key.bin".to_string(), key.to_vec());
        files.insert("/init.mp4".to_string(), encrypt_aes128(&key, &iv, init_plain));
        files.insert("/frag.m4s".to_string(), encrypt_aes128(&key, &iv, frag_plain));
        let fixture = serve(files, Vec::new()).await;
        let dir = fresh_dir("encrypted-fmp4");
        let (ctx, _tx) = JobContext::for_tests(77);
        let _g = crate::util::allow_loopback_for_tests();
        let plan =
            resolve_plan(&ctx, &format!("{}/index.m3u8", fixture.base), &headers(2)).await.unwrap();
        download_pieces(&ctx, &plan, &headers(2), &dir).await.unwrap();
        assert_eq!(std::fs::read(dir.join("init_000.bin")).unwrap(), init_plain);
        assert_eq!(std::fs::read(dir.join("seg_00000.m4s")).unwrap(), frag_plain);
        assert_eq!(fixture.hits.lock().unwrap()["/key.bin"], 1);
        assert!(!dir.join("keys").exists(), "key bytes are never persisted");

        // Simulate both a crash after one piece and a pre-upgrade plaintext key cache.
        // Resume must ignore/delete the stale key file and refetch the key instead.
        std::fs::remove_file(dir.join("seg_00000.m4s")).unwrap();
        std::fs::remove_file(dir.join("seg_00000.m4s.len")).unwrap();
        std::fs::create_dir_all(dir.join("keys")).unwrap();
        std::fs::write(dir.join("keys/key_000.bin"), [0x99u8; AES_KEY_BYTES]).unwrap();
        std::fs::write(dir.join("key-cache.bin"), [0x99u8; AES_KEY_BYTES]).unwrap();
        let (ctx_restart, _tx_restart) = JobContext::for_tests(177);
        download_pieces(&ctx_restart, &plan, &headers(2), &dir).await.unwrap();
        assert_eq!(std::fs::read(dir.join("seg_00000.m4s")).unwrap(), frag_plain);
        assert!(!dir.join("keys").exists(), "legacy plaintext key cache is deleted");
        assert!(!dir.join("key-cache.bin").exists(), "legacy single-file key cache is deleted");
        let hits = fixture.hits.lock().unwrap();
        assert_eq!(hits["/key.bin"], 2, "resume refetches key material");
        assert_eq!(hits["/init.mp4"], 1, "trusted init cache is reused");
        assert_eq!(hits["/frag.m4s"], 2, "only the missing fragment is fetched again");
        drop(hits);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The playlist shape and even the key URI can stay byte-identical while the
    /// key server rotates the key bytes. Plaintext pieces encrypted under the old
    /// key are a different media artifact and must be dropped, even though their
    /// length sidecars still match.
    #[tokio::test]
    async fn rotated_key_bytes_invalidates_plaintext_piece_cache() {
        let playlist = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n#EXTINF:1.0,\nseg0.ts\n#EXTINF:1.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
        let encrypted_files = |key: &[u8; AES_KEY_BYTES], fill: u8| {
            let mut files = HashMap::new();
            files.insert("/index.m3u8".to_string(), playlist.as_bytes().to_vec());
            files.insert("/key.bin".to_string(), key.to_vec());
            for index in 0..2u64 {
                let plain = vec![fill + index as u8; 37];
                files.insert(
                    format!("/seg{index}.ts"),
                    encrypt_aes128(key, &sequence_iv(index), &plain),
                );
            }
            files
        };
        let first_key = [0x11; AES_KEY_BYTES];
        let rotated_key = [0x22; AES_KEY_BYTES];
        let (fixture, files) = serve_mutable(encrypted_files(&first_key, b'O')).await;
        let dir = fresh_dir("key-rotation");
        let (ctx, _tx) = JobContext::for_tests(79);
        let _g = crate::util::allow_loopback_for_tests();
        let plan =
            resolve_plan(&ctx, &format!("{}/index.m3u8", fixture.base), &headers(2)).await.unwrap();
        download_pieces(&ctx, &plan, &headers(2), &dir).await.unwrap();
        assert_eq!(std::fs::read(dir.join("seg_00000.ts")).unwrap(), vec![b'O'; 37]);
        assert_eq!(std::fs::read(dir.join("seg_00001.ts")).unwrap(), vec![b'O' + 1; 37]);

        *files.lock().unwrap() = encrypted_files(&rotated_key, b'N');
        let (ctx_restart, _tx_restart) = JobContext::for_tests(179);
        let plan2 =
            resolve_plan(&ctx_restart, &format!("{}/index.m3u8", fixture.base), &headers(2))
                .await
                .unwrap();
        assert_eq!(
            plan.fingerprint, plan2.fingerprint,
            "the fixture changes only key bytes, not playlist addressing"
        );
        download_pieces(&ctx_restart, &plan2, &headers(2), &dir).await.unwrap();
        assert_eq!(std::fs::read(dir.join("seg_00000.ts")).unwrap(), vec![b'N'; 37]);
        assert_eq!(std::fs::read(dir.join("seg_00001.ts")).unwrap(), vec![b'N' + 1; 37]);
        let hits = fixture.hits.lock().unwrap();
        assert_eq!(hits["/key.bin"], 2, "key bytes are refetched on resume");
        assert_eq!(hits["/seg0.ts"], 2, "old-key plaintext is not reused");
        assert_eq!(hits["/seg1.ts"], 2, "old-key plaintext is not reused");
        drop(hits);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Key rotation commonly mints a new key URI too. Even when the key bytes happen
    /// to be identical, that URI is part of the playlist identity and invalidates pieces.
    #[tokio::test]
    async fn rotated_key_uri_invalidates_piece_cache() {
        let key = [0x33; AES_KEY_BYTES];
        let playlist = |key_name: &str| {
            format!(
                "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-KEY:METHOD=AES-128,URI=\"{key_name}\"\n#EXTINF:1.0,\nseg0.ts\n#EXT-X-ENDLIST\n"
            )
        };
        let build_files = |key_name: &str| {
            let mut files = HashMap::new();
            files.insert("/index.m3u8".to_string(), playlist(key_name).into_bytes());
            files.insert("/key-a.bin".to_string(), key.to_vec());
            files.insert("/key-b.bin".to_string(), key.to_vec());
            files
                .insert("/seg0.ts".to_string(), encrypt_aes128(&key, &sequence_iv(0), &[b'K'; 21]));
            files
        };
        let (fixture, files) = serve_mutable(build_files("key-a.bin")).await;
        let dir = fresh_dir("key-uri-rotation");
        let (ctx, _tx) = JobContext::for_tests(80);
        let _g = crate::util::allow_loopback_for_tests();
        let plan =
            resolve_plan(&ctx, &format!("{}/index.m3u8", fixture.base), &headers(1)).await.unwrap();
        download_pieces(&ctx, &plan, &headers(1), &dir).await.unwrap();
        assert_eq!(fixture.hits.lock().unwrap()["/seg0.ts"], 1);

        files.lock().unwrap().insert("/index.m3u8".to_string(), playlist("key-b.bin").into_bytes());
        let (ctx_restart, _tx_restart) = JobContext::for_tests(181);
        let plan2 =
            resolve_plan(&ctx_restart, &format!("{}/index.m3u8", fixture.base), &headers(1))
                .await
                .unwrap();
        assert_ne!(plan.fingerprint, plan2.fingerprint);
        download_pieces(&ctx_restart, &plan2, &headers(1), &dir).await.unwrap();
        let hits = fixture.hits.lock().unwrap();
        assert_eq!(hits["/seg0.ts"], 2, "a different key URI misses the piece cache");
        assert_eq!(hits["/key-b.bin"], 1);
        drop(hits);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// DEFAULT=YES wins the rendition choice and both playlists use the same
    /// cache engine, with audio isolated in its own namespace.
    #[tokio::test]
    async fn split_audio_master_downloads_video_and_default_audio_rendition() {
        let mut files = HashMap::new();
        files.insert(
            "/master.m3u8".to_string(),
            br#"#EXTM3U
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="backup",URI="backup.m3u8"
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="main",DEFAULT=YES,URI="audio.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=800000,AUDIO="aud"
video.m3u8
"#
            .to_vec(),
        );
        files.insert(
            "/video.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nv0.ts\n#EXTINF:1,\nv1.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        files.insert(
            "/audio.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\na0.ts\n#EXTINF:1,\na1.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        files.insert(
            "/backup.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nb0.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        for (name, bytes) in
            [("v0.ts", b"V0".as_slice()), ("v1.ts", b"V1"), ("a0.ts", b"A0"), ("a1.ts", b"A1")]
        {
            files.insert(format!("/{name}"), bytes.to_vec());
        }
        let fixture = serve(files, Vec::new()).await;
        let dir = fresh_dir("split-audio");
        let (ctx, _tx) = JobContext::for_tests(78);
        let _g = crate::util::allow_loopback_for_tests();
        let plan = resolve_plan(&ctx, &format!("{}/master.m3u8", fixture.base), &headers(4))
            .await
            .unwrap();
        assert_eq!(plan.renditions.len(), 2);
        download_pieces(&ctx, &plan, &headers(4), &dir).await.unwrap();
        assert_eq!(std::fs::read(dir.join("seg_00000.ts")).unwrap(), b"V0");
        assert_eq!(std::fs::read(dir.join("audio/seg_00000.ts")).unwrap(), b"A0");
        assert_eq!(std::fs::read(dir.join("audio/seg_00001.ts")).unwrap(), b"A1");
        let hits = fixture.hits.lock().unwrap();
        assert_eq!(
            hits.get("/backup.m3u8"),
            None,
            "DEFAULT=YES selects only the default rendition"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// TS split renditions are independently remuxed and then merged into one
    /// progressive MP4 through the shared mux engine.
    #[tokio::test]
    async fn split_audio_ts_end_to_end_merges_video_and_audio() {
        let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls");
        let mut files = HashMap::new();
        files.insert(
            "/master.m3u8".to_string(),
            br#"#EXTM3U
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="main",DEFAULT=YES,URI="audio.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=800000,AUDIO="aud"
video.m3u8
"#
            .to_vec(),
        );
        files.insert(
            "/video.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nv.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        files.insert(
            "/audio.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\na.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        files
            .insert("/v.ts".to_string(), std::fs::read(fixture_dir.join("video-only.ts")).unwrap());
        files
            .insert("/a.ts".to_string(), std::fs::read(fixture_dir.join("audio-only.ts")).unwrap());
        let fixture = serve(files, Vec::new()).await;
        let (out_dir, id) = test_out("split-ts-out");
        remove_artifacts(id);
        let dest = out_dir.join("split.mp4");
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/master.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(4),
        )
        .await
        .unwrap();
        let bytes = std::fs::read(&landed).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        assert!(bytes.len() > 10_000, "merged output is unexpectedly small");
        remove_artifacts(id);
        let _ = std::fs::remove_dir_all(out_dir);
    }

    /// Fragmented MP4 split renditions use the byte-concat fast path for both
    /// roles and still reach the shared progressive MP4 merger.
    #[tokio::test]
    async fn split_audio_fmp4_end_to_end_merges_video_and_audio() {
        let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/dash");
        let mut files = HashMap::new();
        files.insert(
            "/master.m3u8".to_string(),
            br#"#EXTM3U
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="main",DEFAULT=YES,URI="audio.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=800000,AUDIO="aud"
video.m3u8
"#
            .to_vec(),
        );
        files.insert(
            "/video.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n#EXT-X-MAP:URI=\"vinit.mp4\"\n#EXTINF:2,\nv1.m4s\n#EXTINF:2,\nv2.m4s\n#EXT-X-ENDLIST\n".to_vec(),
        );
        files.insert(
            "/audio.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n#EXT-X-MAP:URI=\"ainit.mp4\"\n#EXTINF:2,\na0.m4s\n#EXT-X-ENDLIST\n".to_vec(),
        );
        for name in ["vinit.mp4", "v1.m4s", "v2.m4s", "ainit.mp4", "a0.m4s"] {
            files.insert(format!("/{name}"), std::fs::read(fixture_dir.join(name)).unwrap());
        }
        let fixture = serve(files, Vec::new()).await;
        let (out_dir, id) = test_out("split-fmp4-out");
        remove_artifacts(id);
        let dest = out_dir.join("split.mp4");
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/master.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(4),
        )
        .await
        .unwrap();
        assert_eq!(&std::fs::read(&landed).unwrap()[4..8], b"ftyp");
        remove_artifacts(id);
        let _ = std::fs::remove_dir_all(out_dir);
    }

    fn rendition(
        name: &str,
        lang: Option<&str>,
        default: bool,
        autoselect: bool,
        forced: bool,
    ) -> HlsSubtitleRendition {
        HlsSubtitleRendition {
            group_id: "subs".into(),
            language: lang.map(str::to_string),
            name: name.into(),
            uri: format!("subs/{name}.m3u8"),
            is_default: default,
            is_autoselect: autoselect,
            is_forced: forced,
        }
    }

    #[test]
    fn subtitle_selection_prefers_default_then_autoselect_and_always_keeps_forced() {
        let en = rendition("en", Some("en"), true, true, false);
        let tr = rendition("tr", Some("tr"), false, true, false);
        let fr = rendition("fr", Some("fr"), false, false, false);
        // DEFAULT=YES wins over AUTOSELECT.
        let picked = select_subtitle_renditions(&[en.clone(), tr.clone()]);
        assert_eq!(picked, vec![en.clone()]);
        // Without a default the first AUTOSELECT=YES is the pick.
        let picked = select_subtitle_renditions(&[fr.clone(), tr.clone()]);
        assert_eq!(picked, vec![tr.clone()]);
        // Neither default nor autoselect selects nothing.
        assert_eq!(select_subtitle_renditions(std::slice::from_ref(&fr)), Vec::new());
        // FORCED=YES renditions always ride along, in master order.
        let forced = rendition("forced", Some("fr"), false, false, true);
        let picked = select_subtitle_renditions(&[forced.clone(), tr.clone()]);
        assert_eq!(picked, vec![forced.clone(), tr.clone()]);
        // A rendition that is both default and forced is not duplicated.
        let both = rendition("both", Some("en"), true, true, true);
        assert_eq!(select_subtitle_renditions(std::slice::from_ref(&both)), vec![both]);
    }

    /// Master playlist with a TYPE=SUBTITLES group plus the media playlists of
    /// three renditions: en is DEFAULT, tr is plain, fr is FORCED.
    fn subtitle_stream_files() -> HashMap<String, Vec<u8>> {
        let mut files = HashMap::new();
        files.insert(
            "/master.m3u8".to_string(),
            br#"#EXTM3U
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",LANGUAGE="en",NAME="English",DEFAULT=YES,AUTOSELECT=YES,URI="subs/en.m3u8"
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",LANGUAGE="tr",NAME="Turkish",AUTOSELECT=YES,URI="subs/tr.m3u8"
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",LANGUAGE="fr",NAME="French Forced",FORCED=YES,URI="subs/fr.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=800000,SUBTITLES="subs"
video.m3u8
"#
            .to_vec(),
        );
        files.insert(
            "/video.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\npart0.ts\n#EXTINF:2,\npart1.ts\n#EXTINF:2,\npart2.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        let sub_playlist = |stem: &str| {
            format!(
                "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.000,\n{stem}0.vtt\n#EXTINF:2.000,\n{stem}1.vtt\n#EXT-X-ENDLIST\n"
            )
        };
        let cue = |mpegts: u64, text: &str| {
            format!(
                "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:{mpegts}\n\n00:00:00.000 --> 00:00:01.000\n{text}\n"
            )
        };
        for (lang, first, second) in
            [("en", "one", "two"), ("tr", "bir", "iki"), ("fr", "un", "deux")]
        {
            files.insert(format!("/subs/{lang}.m3u8"), sub_playlist(lang).into_bytes());
            files.insert(format!("/subs/{lang}0.vtt"), cue(900_000, first).into_bytes());
            files.insert(format!("/subs/{lang}1.vtt"), cue(1_080_000, second).into_bytes());
        }
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls");
        for i in 0..3 {
            files.insert(
                format!("/part{i}.ts"),
                std::fs::read(fixture.join(format!("part{i}.ts"))).unwrap(),
            );
        }
        files
    }

    /// DEFAULT and FORCED renditions land as corrected .vtt/.srt pairs next to
    /// the finished video; the plain AUTOSELECT-only rendition stays unfetched.
    #[tokio::test]
    async fn master_subtitles_land_next_to_the_video_with_mapped_timestamps() {
        let fixture = serve(subtitle_stream_files(), Vec::new()).await;
        let (out_dir, id) = test_out("subs-out");
        remove_artifacts(id);
        let dest = out_dir.join("clip.mp4");
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/master.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(3),
        )
        .await
        .unwrap();
        assert_eq!(landed.file_name().and_then(|n| n.to_str()), Some("clip.mp4"));
        assert_eq!(&std::fs::read(&landed).unwrap()[4..8], b"ftyp");

        // X-TIMESTAMP-MAP correction: the second segment's MPEGTS clock puts
        // its cue at 2–3 s in the concatenated timeline.
        let vtt = std::fs::read_to_string(out_dir.join("clip.en.vtt")).unwrap();
        assert!(vtt.starts_with("WEBVTT"), "{vtt}");
        assert!(vtt.contains("00:00:00.000 --> 00:00:01.000\none"), "{vtt}");
        assert!(vtt.contains("00:00:02.000 --> 00:00:03.000\ntwo"), "{vtt}");
        assert!(!vtt.contains("X-TIMESTAMP-MAP"), "{vtt}");
        let srt = std::fs::read_to_string(out_dir.join("clip.en.srt")).unwrap();
        assert!(srt.contains("00:00:00,000 --> 00:00:01,000\none"), "{srt}");
        assert!(srt.contains("00:00:02,000 --> 00:00:03,000\ntwo"), "{srt}");

        // FORCED=YES is always included even without DEFAULT/AUTOSELECT.
        assert!(out_dir.join("clip.fr.vtt").exists());
        assert!(out_dir.join("clip.fr.srt").exists());
        let forced = std::fs::read_to_string(out_dir.join("clip.fr.vtt")).unwrap();
        assert!(forced.contains("un"), "{forced}");

        // tr is neither DEFAULT nor the first AUTOSELECT nor FORCED.
        assert!(!out_dir.join("clip.tr.vtt").exists());
        assert!(!out_dir.join("clip.tr.srt").exists());
        let hits = fixture.hits.lock().unwrap();
        assert_eq!(hits.get("/subs/tr.m3u8"), None, "unselected rendition is never fetched");
        assert_eq!(hits["/subs/en.m3u8"], 1);
        assert_eq!(hits["/subs/fr.m3u8"], 1);
        drop(hits);
        remove_artifacts(id);
        let _ = std::fs::remove_dir_all(out_dir);
    }

    /// A subtitle server that 500s every request must not turn the completed
    /// video into a failure: the media lands and no subtitle file appears.
    #[tokio::test]
    async fn failing_subtitle_server_still_completes_the_video() {
        let fixture =
            serve_full(subtitle_stream_files(), Vec::new(), vec!["/subs/en.m3u8".into()]).await;
        let (out_dir, id) = test_out("subs-fail-out");
        remove_artifacts(id);
        let dest = out_dir.join("clip.mp4");
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/master.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(3),
        )
        .await
        .unwrap();
        assert_eq!(&std::fs::read(&landed).unwrap()[4..8], b"ftyp", "video still completes");
        assert!(!out_dir.join("clip.en.vtt").exists());
        assert!(!out_dir.join("clip.en.srt").exists());
        // The FORCED rendition is independent of the broken one and still lands.
        assert!(out_dir.join("clip.fr.vtt").exists());
        let hits = fixture.hits.lock().unwrap();
        assert!(hits["/subs/en.m3u8"] >= 1, "the failing playlist was attempted");
        drop(hits);
        remove_artifacts(id);
        let _ = std::fs::remove_dir_all(out_dir);
    }
}
