//! DASH (MPD) engine for static, unencrypted fragmented media.
//!
//! Segments are fetched in parallel into a per-download session dir, each written
//! to `*.tmp` and renamed only when complete. A fingerprinted cache manifest
//! prevents bytes from a changed representation plan being trusted on resume.
//!
//! Every representation is assembled as init segment + media segments in MPD
//! order. That byte concatenation is the fragmented-MP4 representation stream;
//! the final handoff is isolated in one function so the muxer can consume the
//! same assembly without touching the download or resume machinery.

use crate::download::http_client::{open, parse_content_range, HttpHeaders, Opened, RangeRequest};
use crate::download::job::JobContext;
use crate::download::limiter::{cancelled, sleep_or_cancel};
use crate::download::DownloadError;
use crate::util::{split_falcon_format, validate_fetch_url_async};
use dash_mpd_core::{
    AdaptationSet, BaseURL, Initialization, Period, Representation, SegmentBase, SegmentList,
    SegmentTemplate, MPD,
};
use futures::stream::{self, StreamExt};
use reqwest::header::CONTENT_ENCODING;
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

const MAX_MPD_BYTES: usize = 16 * 1024 * 1024;
const MAX_INDEX_BYTES: usize = 16 * 1024 * 1024;
const MAX_SEGMENT_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_SEGMENTS_PER_REPRESENTATION: usize = 100_000;
const SEGMENT_ATTEMPTS: u32 = 4;
const ERR_DASH_LIVE: &str = "ERR_DASH_LIVE";
const ERR_DASH_DRM: &str = "ERR_DASH_DRM";

pub(crate) fn dash_session_dirname(download_id: i64) -> String {
    format!("dash-{download_id}.falcondm-temp")
}

pub(crate) fn dash_session_dir(download_id: i64) -> PathBuf {
    crate::util::app_data_dir().join("downloads_temp").join(dash_session_dirname(download_id))
}

/// Remove the segment cache of a download (remove, or retry of a failed row).
pub fn remove_artifacts(download_id: i64) {
    let _ = std::fs::remove_dir_all(dash_session_dir(download_id));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    end: u64,
}

impl ByteRange {
    fn len(self) -> u64 {
        self.end - self.start + 1
    }
}

fn parse_byte_range(raw: &str) -> Result<ByteRange, DownloadError> {
    let trimmed = raw.trim();
    let Some((start, end)) = trimmed.split_once('-') else {
        return Err(DownloadError::fatal(format!("Invalid DASH byte range: {raw}")));
    };
    let start = start
        .trim()
        .parse::<u64>()
        .map_err(|_| DownloadError::fatal(format!("Invalid DASH byte range: {raw}")))?;
    let end = end
        .trim()
        .parse::<u64>()
        .map_err(|_| DownloadError::fatal(format!("Invalid DASH byte range: {raw}")))?;
    if end < start || end == u64::MAX {
        return Err(DownloadError::fatal(format!("Invalid DASH byte range: {raw}")));
    }
    Ok(ByteRange { start, end })
}

/// Substitute the DASH template identifiers used in media and initialization
/// URLs. Unknown or malformed identifiers are left untouched, matching the
/// forgiving behavior of players faced with real-world manifests.
fn render_template(
    template: &str,
    representation_id: &str,
    bandwidth: u64,
    number: u64,
    time: u64,
) -> String {
    fn width(spec: &str) -> Option<usize> {
        let digits = spec.strip_suffix('d')?;
        if digits.is_empty() {
            return Some(0);
        }
        if digits.len() > 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    }

    fn padded(value: u64, width: Option<usize>) -> String {
        match width.filter(|width| *width > 0) {
            Some(width) => format!("{value:0width$}"),
            None => value.to_string(),
        }
    }

    fn substitute(
        token: &str,
        representation_id: &str,
        bandwidth: u64,
        number: u64,
        time: u64,
    ) -> Option<String> {
        let (name, format) = match token.split_once('%') {
            Some((name, spec)) => (name, width(spec)?),
            None => (token, 0),
        };
        let format = (format > 0).then_some(format);
        Some(match name {
            "RepresentationID" if format.is_none() => representation_id.to_string(),
            "Number" => padded(number, format),
            "Time" => padded(time, format),
            "Bandwidth" => padded(bandwidth, format),
            _ => return None,
        })
    }

    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(pos) = rest.find('$') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        if let Some(stripped) = after.strip_prefix('$') {
            out.push('$');
            rest = stripped;
            continue;
        }
        let Some(end) = after.find('$') else {
            out.push('$');
            out.push_str(after);
            return out;
        };
        let token = &after[..end];
        match substitute(token, representation_id, bandwidth, number, time) {
            Some(value) => out.push_str(&value),
            None => {
                out.push('$');
                out.push_str(token);
                out.push('$');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SegmentPoint {
    number: u64,
    time: u64,
}

fn cap_segment_count(count: u128) -> Result<u64, DownloadError> {
    if count == 0 {
        return Err(DownloadError::fatal("No DASH segments found"));
    }
    if count > MAX_SEGMENTS_PER_REPRESENTATION as u128 {
        return Err(DownloadError::fatal(format!(
            "DASH representation contains too many segments: {count}"
        )));
    }
    Ok(count as u64)
}

fn apply_end_number(
    start_number: u64,
    count: u64,
    end_number: Option<u64>,
) -> Result<u64, DownloadError> {
    let Some(end) = end_number else {
        return Ok(count);
    };
    if end < start_number {
        return Err(DownloadError::fatal("DASH endNumber is before startNumber"));
    }
    Ok(count.min(end - start_number + 1))
}

/// Expand `SegmentTemplate@duration` without a SegmentTimeline. The required
/// count is ceil(period seconds × timescale / duration).
fn expand_duration_segments(
    template: &SegmentTemplate,
    period_duration: Duration,
) -> Result<Vec<SegmentPoint>, DownloadError> {
    let timescale = template.timescale.unwrap_or(1);
    if timescale == 0 {
        return Err(DownloadError::fatal("DASH timescale must not be zero"));
    }
    let Some(duration) = template.duration.filter(|duration| *duration > 0.0) else {
        return Err(DownloadError::fatal("DASH SegmentTemplate duration is missing"));
    };
    let count = (period_duration.as_secs_f64() * timescale as f64 / duration).ceil();
    if !count.is_finite() || count > MAX_SEGMENTS_PER_REPRESENTATION as f64 {
        return Err(DownloadError::fatal("DASH representation contains too many segments"));
    }
    let start = template.startNumber.unwrap_or(1);
    let count = apply_end_number(start, count as u64, template.endNumber)?;
    let count = cap_segment_count(u128::from(count))?;
    let mut points = Vec::with_capacity(count as usize);
    for index in 0..count {
        let number = start
            .checked_add(index)
            .ok_or_else(|| DownloadError::fatal("DASH segment number overflow"))?;
        // ISO 23009-1 table 20: without a timeline the time is the segment start
        // counted from the first segment; presentationTimeOffset is not a URL offset.
        let time = (index as f64 * duration).round();
        if !time.is_finite() || time > u64::MAX as f64 {
            return Err(DownloadError::fatal("DASH segment time overflow"));
        }
        points.push(SegmentPoint { number, time: time as u64 });
    }
    Ok(points)
}

/// Exact ceil((period_end_time - segment_start) / segment_duration), with the
/// period end represented in timescale units as a rational number.
fn negative_repeat_count(
    start_time: u64,
    segment_duration: u64,
    period_duration: Duration,
    timescale: u64,
    presentation_time_offset: u64,
) -> Result<u128, DownloadError> {
    const NANOS_PER_SECOND: u128 = 1_000_000_000;
    let nanos = u128::from(period_duration.as_secs()) * NANOS_PER_SECOND
        + u128::from(period_duration.subsec_nanos());
    let numerator = nanos
        .checked_mul(u128::from(timescale))
        .ok_or_else(|| DownloadError::fatal("DASH period duration overflow"))?;
    let denominator = NANOS_PER_SECOND;
    let end = u128::from(presentation_time_offset) * denominator + numerator;
    let start = u128::from(start_time) * denominator;
    if end <= start {
        return Ok(0);
    }
    let step = u128::from(segment_duration) * denominator;
    Ok((end - start).div_ceil(step))
}

/// Expand a SegmentTimeline. Positive `r` counts additional repetitions
/// (`r=5` → six segments); a negative final `r` repeats until the period end.
fn expand_timeline_segments(
    template: &SegmentTemplate,
    period_duration: Duration,
) -> Result<Vec<SegmentPoint>, DownloadError> {
    let Some(timeline) = template.SegmentTimeline.as_ref() else {
        return Err(DownloadError::fatal("DASH SegmentTimeline is missing"));
    };
    if timeline.segments.is_empty() {
        return Err(DownloadError::fatal("DASH SegmentTimeline is empty"));
    }
    let timescale = template.timescale.unwrap_or(1);
    if timescale == 0 {
        return Err(DownloadError::fatal("DASH timescale must not be zero"));
    }
    let pto = template.presentationTimeOffset.unwrap_or(0);
    let mut number = template.startNumber.unwrap_or(1);
    let mut time = 0u64;
    let mut points = Vec::new();
    for (index, entry) in timeline.segments.iter().enumerate() {
        if entry.d == 0 {
            return Err(DownloadError::fatal("DASH SegmentTimeline entry duration is zero"));
        }
        if let Some(t) = entry.t {
            time = t;
        }
        if let Some(n) = entry.n {
            number = n;
        }
        let count = match entry.r.unwrap_or(0) {
            r if r < 0 => {
                if index + 1 != timeline.segments.len() {
                    return Err(DownloadError::fatal(
                        "DASH negative SegmentTimeline repeat is not on the last entry",
                    ));
                }
                negative_repeat_count(time, entry.d, period_duration, timescale, pto)?
            }
            r => u128::from(r as u64) + 1,
        };
        let count = cap_segment_count(count)?;
        for _ in 0..count {
            if template.endNumber.is_some_and(|end| number > end) {
                break;
            }
            if points.len() >= MAX_SEGMENTS_PER_REPRESENTATION {
                return Err(DownloadError::fatal("DASH representation contains too many segments"));
            }
            points.push(SegmentPoint { number, time });
            number = number
                .checked_add(1)
                .ok_or_else(|| DownloadError::fatal("DASH segment number overflow"))?;
            time = time
                .checked_add(entry.d)
                .ok_or_else(|| DownloadError::fatal("DASH segment time overflow"))?;
        }
    }
    if points.is_empty() {
        return Err(DownloadError::fatal("No DASH segments found"));
    }
    Ok(points)
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// Parse the media references of every top-level `sidx` box in an index range.
/// `box_end` is the absolute one-past-the-end position of the containing box;
/// `first_offset` is relative to that position by ISOBMFF definition.
fn parse_sidx_box(
    body: &[u8],
    box_end: u64,
    ranges: &mut Vec<ByteRange>,
) -> Result<(), DownloadError> {
    if body.len() < 4 {
        return Err(DownloadError::fatal("Truncated DASH segment index"));
    }
    let version = body[0];
    let mut at = 4usize;
    let read32 = |at: &mut usize| -> Result<u32, DownloadError> {
        let value = read_u32(body, *at)
            .ok_or_else(|| DownloadError::fatal("Truncated DASH segment index"))?;
        *at += 4;
        Ok(value)
    };
    let _reference_id = read32(&mut at)?;
    let _timescale = read32(&mut at)?;
    let first_offset = if version == 0 {
        let _earliest_presentation_time = read32(&mut at)?;
        u64::from(read32(&mut at)?)
    } else {
        if body.len() < at + 16 {
            return Err(DownloadError::fatal("Truncated DASH segment index"));
        }
        let _earliest_presentation_time = read_u64(body, at).unwrap_or(0);
        at += 8;
        let offset = read_u64(body, at)
            .ok_or_else(|| DownloadError::fatal("Truncated DASH segment index"))?;
        at += 8;
        offset
    };
    if body.len() < at + 4 {
        return Err(DownloadError::fatal("Truncated DASH segment index"));
    }
    let reserved_and_count =
        read_u32(body, at).ok_or_else(|| DownloadError::fatal("Truncated DASH segment index"))?;
    at += 4;
    let reference_count = (reserved_and_count & 0xffff) as usize;
    if body.len() < at + reference_count * 12 {
        return Err(DownloadError::fatal("Truncated DASH segment index"));
    }
    let mut cursor = box_end
        .checked_add(first_offset)
        .ok_or_else(|| DownloadError::fatal("DASH segment index offset overflow"))?;
    for _ in 0..reference_count {
        let reference = read32(&mut at)?;
        let reference_type = reference >> 31;
        let size = u64::from(reference & 0x7fff_ffff);
        let _subsegment_duration = read32(&mut at)?;
        let _sap = read32(&mut at)?;
        if size == 0 || size > MAX_SEGMENT_BYTES {
            return Err(DownloadError::fatal("Invalid DASH indexed segment size"));
        }
        let end = cursor
            .checked_add(size - 1)
            .ok_or_else(|| DownloadError::fatal("DASH segment range overflow"))?;
        // reference_type 1 is another index, not media bytes. It is skipped while
        // the cursor still advances so following media references stay aligned.
        if reference_type == 0 {
            ranges.push(ByteRange { start: cursor, end });
        }
        cursor = end
            .checked_add(1)
            .ok_or_else(|| DownloadError::fatal("DASH segment range overflow"))?;
    }
    Ok(())
}

fn parse_sidx_segments(
    index: &[u8],
    index_range: ByteRange,
) -> Result<Vec<ByteRange>, DownloadError> {
    let mut ranges = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= index.len() {
        let type_start = pos + 4;
        let mut size = u64::from(
            read_u32(index, pos).ok_or_else(|| DownloadError::fatal("Bad DASH segment index"))?,
        );
        let mut header = 8usize;
        if size == 1 {
            size = read_u64(index, pos + 8)
                .ok_or_else(|| DownloadError::fatal("Bad DASH segment index"))?;
            header = 16;
        } else if size == 0 {
            size = (index.len() - pos) as u64;
        }
        if size < header as u64 || pos + size as usize > index.len() {
            return Err(DownloadError::fatal("Truncated DASH segment index box"));
        }
        let end = pos + size as usize;
        if index.get(type_start..type_start + 4) == Some(b"sidx") {
            let box_end = index_range
                .start
                .checked_add(end as u64)
                .ok_or_else(|| DownloadError::fatal("DASH segment index offset overflow"))?;
            parse_sidx_box(&index[pos + header..end], box_end, &mut ranges)?;
        }
        pos = end;
    }
    if ranges.is_empty() {
        return Err(DownloadError::fatal("DASH segment index has no media references"));
    }
    if ranges.len() > MAX_SEGMENTS_PER_REPRESENTATION {
        return Err(DownloadError::fatal("DASH representation contains too many segments"));
    }
    Ok(ranges)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MediaKind {
    Video,
    Audio,
}

impl MediaKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }
}

#[derive(Clone, Debug)]
struct Piece {
    url: Url,
    range: Option<ByteRange>,
}

#[derive(Clone, Debug)]
struct RepresentationPlan {
    id: String,
    kind: MediaKind,
    mime_type: Option<String>,
    codecs: Option<String>,
    bandwidth: u64,
    lang: Option<String>,
    init: Option<Piece>,
    segments: Vec<Piece>,
    timescale: u64,
    estimated_bytes: Option<u64>,
}

struct Plan {
    /// Manifest URL the user gave us: cookies are only sent to its origin.
    source: Url,
    /// Final manifest URL after redirects. BaseURL resolution starts here.
    manifest_url: Url,
    representations: Vec<RepresentationPlan>,
    primary: usize,
    estimated_bytes: Option<u64>,
    period_seconds: Option<f64>,
    /// FNV-1a over the ordered query-stripped piece locators plus the facts
    /// that define addressing (representation ids, count, init range, timescale).
    fingerprint: String,
}

fn check_content_encoding(response: &Response) -> Result<(), DownloadError> {
    let encoded = response
        .headers()
        .get(CONTENT_ENCODING)
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
                _ => Err(DownloadError::fatal("DASH byte-range response is invalid")),
            };
        }
        if status.is_success() {
            return Err(DownloadError::fatal("DASH byte-range request was not honored"));
        }
        return Err(DownloadError::from_status(status.as_u16()));
    }
    if status.is_success() {
        Ok(())
    } else {
        Err(DownloadError::from_status(status.as_u16()))
    }
}

/// Bounded GET used for the MPD and in-band segment indexes. Unlike media
/// pieces these are control data, so holding them in memory is correct.
async fn fetch_bytes(
    ctx: &JobContext,
    source: &Url,
    target: &Url,
    headers: &HttpHeaders,
    range: Option<ByteRange>,
    max_bytes: usize,
    what: &'static str,
) -> Result<(Vec<u8>, Url), DownloadError> {
    if range.is_some_and(|range| range.len() > max_bytes as u64) {
        return Err(DownloadError::fatal(format!("{what} is too large")));
    }
    let range_request = range.map_or_else(RangeRequest::default, |range| RangeRequest {
        start: Some(range.start),
        end: Some(range.end),
        if_range: None,
    });
    let mut cancel = ctx.cancel.clone();
    let work = async {
        let Opened { response, final_url, .. } =
            open(source, target, headers, &range_request, None).await?;
        validate_response(&response, range)?;
        check_content_encoding(&response)?;
        if response.content_length().is_some_and(|len| len > max_bytes as u64) {
            return Err(DownloadError::fatal(format!("{what} is too large")));
        }
        let mut body = Vec::new();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk =
                chunk.map_err(|e| DownloadError::transient(format!("{what} read failed: {e}")))?;
            if body.len() + chunk.len() > max_bytes {
                return Err(DownloadError::fatal(format!("{what} is too large")));
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

fn choose_base_url(urls: &[BaseURL]) -> Option<&BaseURL> {
    let mut best: Option<&BaseURL> = None;
    for url in urls.iter().filter(|url| !url.base.trim().is_empty()) {
        let replace =
            best.is_none_or(|current| url.priority.unwrap_or(1) < current.priority.unwrap_or(1));
        if replace {
            best = Some(url);
        }
    }
    best
}

fn checked_http_url(url: Url, what: &str) -> Result<Url, DownloadError> {
    if matches!(url.scheme(), "http" | "https") {
        Ok(url)
    } else {
        Err(DownloadError::fatal(format!("Unsupported DASH {what} URL scheme")))
    }
}

/// Resolve nested BaseURLs in MPD → Period → AdaptationSet → Representation
/// order. RFC 3986 joining is intentional and starts at the final post-redirect
/// MPD URL, not the URL originally typed by the user.
fn resolve_base_url(
    manifest: &Url,
    mpd: &MPD,
    period: &Period,
    adaptation: &AdaptationSet,
    representation: &Representation,
) -> Result<Url, DownloadError> {
    let mut base = manifest.clone();
    for level in [
        mpd.base_url.as_slice(),
        period.BaseURL.as_slice(),
        adaptation.BaseURL.as_slice(),
        representation.BaseURL.as_slice(),
    ] {
        if let Some(selected) = choose_base_url(level) {
            base = base
                .join(selected.base.trim())
                .map_err(|e| DownloadError::fatal(format!("Invalid DASH BaseURL: {e}")))?;
            base = checked_http_url(base, "BaseURL")?;
        }
    }
    Ok(base)
}

fn resolve_reference(base: &Url, reference: &str) -> Result<Url, DownloadError> {
    let url = base
        .join(reference.trim())
        .map_err(|e| DownloadError::fatal(format!("Invalid DASH segment URL: {e}")))?;
    checked_http_url(url, "segment")
}

fn initialization_piece(
    initialization: &Initialization,
    base: &Url,
) -> Result<Option<Piece>, DownloadError> {
    let range = initialization.range.as_deref().map(parse_byte_range).transpose()?;
    let source =
        initialization.sourceURL.as_deref().map(str::trim).filter(|source| !source.is_empty());
    match (source, range) {
        (None, None) => Ok(None),
        (source, range) => {
            let url = match source {
                Some(source) => resolve_reference(base, source)?,
                // A range without sourceURL addresses bytes of the representation resource.
                None => base.clone(),
            };
            Ok(Some(Piece { url, range }))
        }
    }
}

fn template_initialization_piece(
    template: &SegmentTemplate,
    base: &Url,
    representation_id: &str,
    bandwidth: u64,
) -> Result<Option<Piece>, DownloadError> {
    if let Some(raw) =
        template.initialization.as_deref().map(str::trim).filter(|raw| !raw.is_empty())
    {
        let start = template.startNumber.unwrap_or(1);
        let rendered = render_template(raw, representation_id, bandwidth, start, 0);
        return Ok(Some(Piece { url: resolve_reference(base, &rendered)?, range: None }));
    }
    match template.Initialization.as_ref() {
        Some(initialization) => initialization_piece(initialization, base),
        None => Ok(None),
    }
}

fn expand_template_pieces(
    template: &SegmentTemplate,
    base: &Url,
    representation_id: &str,
    bandwidth: u64,
    period_duration: Duration,
) -> Result<(Option<Piece>, Vec<Piece>, u64), DownloadError> {
    let timescale = template.timescale.unwrap_or(1);
    if timescale == 0 {
        return Err(DownloadError::fatal("DASH timescale must not be zero"));
    }
    let media =
        template.media.as_deref().map(str::trim).filter(|media| !media.is_empty()).ok_or_else(
            || DownloadError::fatal("DASH SegmentTemplate media template is missing"),
        )?;
    let points = if template.SegmentTimeline.is_some() {
        expand_timeline_segments(template, period_duration)?
    } else {
        expand_duration_segments(template, period_duration)?
    };
    let init = template_initialization_piece(template, base, representation_id, bandwidth)?;
    let mut segments = Vec::with_capacity(points.len());
    for point in points {
        let rendered =
            render_template(media, representation_id, bandwidth, point.number, point.time);
        segments.push(Piece { url: resolve_reference(base, &rendered)?, range: None });
    }
    Ok((init, segments, timescale))
}

fn expand_list_pieces(
    list: &SegmentList,
    base: &Url,
) -> Result<(Option<Piece>, Vec<Piece>, u64), DownloadError> {
    let init = match list.Initialization.as_ref() {
        Some(initialization) => initialization_piece(initialization, base)?,
        None => None,
    };
    let mut segments = Vec::new();
    for segment in &list.segment_urls {
        let range = segment.mediaRange.as_deref().map(parse_byte_range).transpose()?;
        let reference = segment.media.as_deref().map(str::trim).filter(|media| !media.is_empty());
        if reference.is_none() && range.is_none() {
            // A SegmentURL can alternatively carry an index URL; it is not media.
            continue;
        }
        let url = match reference {
            Some(reference) => resolve_reference(base, reference)?,
            None => base.clone(),
        };
        if range.is_some_and(|range| range.len() > MAX_SEGMENT_BYTES) {
            return Err(DownloadError::fatal("DASH segment is too large"));
        }
        segments.push(Piece { url, range });
    }
    if segments.len() > MAX_SEGMENTS_PER_REPRESENTATION {
        return Err(DownloadError::fatal("DASH representation contains too many segments"));
    }
    if segments.is_empty() {
        return Err(DownloadError::fatal("No DASH segments found"));
    }
    Ok((init, segments, list.timescale.unwrap_or(1)))
}

async fn expand_base_pieces(
    ctx: &JobContext,
    source: &Url,
    headers: &HttpHeaders,
    resource: &Url,
    base: &SegmentBase,
) -> Result<(Option<Piece>, Vec<Piece>, u64), DownloadError> {
    let init = match base.Initialization.as_ref() {
        Some(initialization) => initialization_piece(initialization, resource)?,
        None => None,
    };
    let Some(index_range) = base.indexRange.as_deref().map(parse_byte_range).transpose()? else {
        return Err(DownloadError::fatal("DASH SegmentBase without indexRange is not supported"));
    };
    if index_range.len() > MAX_INDEX_BYTES as u64 {
        return Err(DownloadError::fatal("DASH segment index is too large"));
    }
    let index_piece = Piece { url: resource.clone(), range: Some(index_range) };
    let (index, _) = fetch_bytes(
        ctx,
        source,
        &index_piece.url,
        headers,
        index_piece.range,
        MAX_INDEX_BYTES,
        "DASH segment index",
    )
    .await?;
    let ranges = parse_sidx_segments(&index, index_range)?;
    let segments = ranges
        .into_iter()
        .map(|range| Piece { url: resource.clone(), range: Some(range) })
        .collect();
    Ok((init, segments, base.timescale.unwrap_or(1)))
}

enum Segmenting<'a> {
    Template(&'a SegmentTemplate),
    List(&'a SegmentList),
    Base(&'a SegmentBase),
}

fn level_segmenting<'a>(
    base: Option<&'a SegmentBase>,
    list: Option<&'a SegmentList>,
    template: Option<&'a SegmentTemplate>,
) -> Option<Segmenting<'a>> {
    if let Some(template) = template {
        Some(Segmenting::Template(template))
    } else if let Some(list) = list {
        Some(Segmenting::List(list))
    } else {
        base.map(Segmenting::Base)
    }
}

fn effective_segmenting<'a>(
    period: &'a Period,
    adaptation: &'a AdaptationSet,
    representation: &'a Representation,
) -> Option<Segmenting<'a>> {
    level_segmenting(
        representation.SegmentBase.as_ref(),
        representation.SegmentList.as_ref(),
        representation.SegmentTemplate.as_ref(),
    )
    .or_else(|| {
        level_segmenting(
            adaptation.SegmentBase.as_ref(),
            adaptation.SegmentList.as_ref(),
            adaptation.SegmentTemplate.as_ref(),
        )
    })
    .or_else(|| {
        level_segmenting(
            period.SegmentBase.as_ref(),
            period.SegmentList.as_ref(),
            period.SegmentTemplate.as_ref(),
        )
    })
}

fn codecs_include(codecs: &str, video: bool) -> bool {
    const VIDEO: &[&str] = &[
        "avc1", "avc2", "avc3", "avc4", "hvc1", "hev1", "vp8", "vp9", "vp09", "av01", "dvhe",
        "dvh1",
    ];
    const AUDIO: &[&str] = &[
        "mp4a", "ac-3", "ec-3", "opus", "vorbis", "flac", "alac", "mp3", "dtsc", "dtse", "dtsl",
        "dtsx",
    ];
    codecs.split(',').map(str::trim).any(|codec| {
        let codec = codec.to_ascii_lowercase();
        if video {
            VIDEO.iter().any(|prefix| codec.starts_with(prefix))
        } else {
            AUDIO.iter().any(|prefix| codec.starts_with(prefix))
        }
    })
}

fn kind_from_attributes(
    content_type: Option<&str>,
    mime_type: Option<&str>,
    codecs: Option<&str>,
) -> Option<MediaKind> {
    if let Some(kind) = content_type.map(str::trim).map(str::to_ascii_lowercase) {
        if kind == "video" {
            return Some(MediaKind::Video);
        }
        if kind == "audio" {
            return Some(MediaKind::Audio);
        }
    }
    if let Some(mime) = mime_type
        .and_then(|mime| mime.split(';').next())
        .map(str::trim)
        .map(str::to_ascii_lowercase)
    {
        if mime.starts_with("video/") {
            return Some(MediaKind::Video);
        }
        if mime.starts_with("audio/") {
            return Some(MediaKind::Audio);
        }
    }
    if let Some(codecs) = codecs {
        // A muxed video adaptation can carry both codec families; video is the
        // primary kind because it is the deliverable representation.
        if codecs_include(codecs, true) {
            return Some(MediaKind::Video);
        }
        if codecs_include(codecs, false) {
            return Some(MediaKind::Audio);
        }
    }
    None
}

fn adaptation_kind(adaptation: &AdaptationSet) -> Option<MediaKind> {
    if let Some(kind) = kind_from_attributes(
        adaptation.contentType.as_deref(),
        adaptation.mimeType.as_deref(),
        adaptation.codecs.as_deref(),
    ) {
        return Some(kind);
    }
    for representation in &adaptation.representations {
        if let Some(kind) = kind_from_attributes(
            representation.contentType.as_deref().or(adaptation.contentType.as_deref()),
            representation.mimeType.as_deref().or(adaptation.mimeType.as_deref()),
            representation.codecs.as_deref().or(adaptation.codecs.as_deref()),
        ) {
            return Some(kind);
        }
    }
    if adaptation.width.is_some() || adaptation.height.is_some() {
        return Some(MediaKind::Video);
    }
    None
}

/// Rank by bandwidth first, then width and height as required for video.
fn representation_rank(
    adaptation: &AdaptationSet,
    representation: &Representation,
) -> (u64, u64, u64) {
    (
        representation.bandwidth.unwrap_or(0),
        representation.width.or(adaptation.width).unwrap_or(0),
        representation.height.or(adaptation.height).unwrap_or(0),
    )
}

fn best_representation(adaptation: &AdaptationSet) -> Option<(usize, (u64, u64, u64))> {
    let mut best: Option<(usize, (u64, u64, u64))> = None;
    for (index, representation) in adaptation.representations.iter().enumerate() {
        let rank = representation_rank(adaptation, representation);
        if best.is_none_or(|(_, best_rank)| rank > best_rank) {
            best = Some((index, rank));
        }
    }
    best
}

struct AdaptationChoice<'a> {
    adaptation: &'a AdaptationSet,
    representation_index: usize,
    kind: MediaKind,
    rank: (u64, u64, u64),
    main: bool,
}

fn adaptation_choices(period: &Period) -> Vec<AdaptationChoice<'_>> {
    let mut choices = Vec::new();
    for adaptation in &period.adaptations {
        let Some(kind) = adaptation_kind(adaptation) else {
            continue;
        };
        let Some((representation_index, rank)) = best_representation(adaptation) else {
            continue;
        };
        let main = adaptation.Role.iter().any(|role| {
            role.value.as_deref().is_some_and(|value| value.eq_ignore_ascii_case("main"))
        });
        choices.push(AdaptationChoice { adaptation, representation_index, kind, rank, main });
    }
    choices
}

fn choose_adaptation<'a>(
    choices: &'a [AdaptationChoice<'a>],
    kind: MediaKind,
) -> Option<&'a AdaptationChoice<'a>> {
    let mut best: Option<&AdaptationChoice<'_>> = None;
    for choice in choices.iter().filter(|choice| choice.kind == kind) {
        let key = (choice.main, choice.rank);
        if best.is_none_or(|current| key > (current.main, current.rank)) {
            best = Some(choice);
        }
    }
    best
}

fn period_starts(mpd: &MPD) -> Vec<Option<Duration>> {
    let mut next = Some(Duration::ZERO);
    let mut starts = Vec::with_capacity(mpd.periods.len());
    for period in &mpd.periods {
        let start = period.start.or(next);
        starts.push(start);
        next = match (start, period.duration) {
            (Some(start), Some(duration)) => start.checked_add(duration),
            _ => None,
        };
    }
    starts
}

fn duration_between(start: Option<Duration>, end: Option<Duration>) -> Option<Duration> {
    end.zip(start).and_then(|(end, start)| end.checked_sub(start))
}

fn period_duration_at(mpd: &MPD, starts: &[Option<Duration>], index: usize) -> Option<Duration> {
    let period = &mpd.periods[index];
    if period.duration.is_some() {
        return period.duration;
    }
    if index + 1 < starts.len() {
        if let Some(duration) = duration_between(starts[index], starts[index + 1]) {
            return Some(duration);
        }
    }
    if index + 1 == mpd.periods.len() {
        return duration_between(starts[index], mpd.mediaPresentationDuration);
    }
    None
}

/// Pick the longest period. Server-side ad insertion produces multi-period
/// MPDs whose full semantic model is outside this engine; choosing the longest
/// period keeps the main feature rather than concatenating unrelated ads.
fn select_longest_period(mpd: &MPD) -> Option<(usize, Option<Duration>)> {
    if mpd.periods.is_empty() {
        return None;
    }
    let starts = period_starts(mpd);
    let mut best = 0usize;
    let mut best_duration = period_duration_at(mpd, &starts, 0);
    for index in 1..mpd.periods.len() {
        let duration = period_duration_at(mpd, &starts, index);
        let replace = match (duration, best_duration) {
            (Some(candidate), Some(current)) => candidate > current,
            (Some(_), None) => true,
            _ => false,
        };
        if replace {
            best = index;
            best_duration = duration;
        }
    }
    Some((best, best_duration))
}

fn has_content_protection(mpd: &MPD) -> bool {
    if !mpd.ContentProtection.is_empty() {
        return true;
    }
    mpd.periods.iter().any(|period| {
        !period.ContentProtection.is_empty()
            || period.adaptations.iter().any(|adaptation| {
                !adaptation.ContentProtection.is_empty()
                    || adaptation
                        .representations
                        .iter()
                        .any(|representation| !representation.ContentProtection.is_empty())
            })
    })
}

fn reject_unsupported_type(mpd: &MPD) -> Result<(), DownloadError> {
    let kind = mpd.mpdtype.as_deref().map(str::to_ascii_lowercase);
    match kind.as_deref() {
        None | Some("static") => Ok(()),
        Some("dynamic") => Err(DownloadError::fatal(ERR_DASH_LIVE)),
        Some(other) => {
            Err(DownloadError::fatal(format!("Unsupported DASH presentation type: {other}")))
        }
    }
}

fn estimate_representation_bytes(
    init: Option<&Piece>,
    segments: &[Piece],
    bandwidth: u64,
    period_seconds: Option<f64>,
) -> Option<u64> {
    let mut exact = 0u64;
    let mut all_ranged = true;
    let mut count = 0usize;
    for piece in init.into_iter().chain(segments.iter()) {
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
    let seconds = period_seconds.filter(|seconds| *seconds > 0.0)?;
    (bandwidth > 0).then(|| (bandwidth as f64 / 8.0 * seconds) as u64)
}

async fn build_representation_plan(
    ctx: &JobContext,
    source: &Url,
    headers: &HttpHeaders,
    mpd: &MPD,
    manifest_url: &Url,
    period: &Period,
    period_duration: Option<Duration>,
    choice: &AdaptationChoice<'_>,
) -> Result<RepresentationPlan, DownloadError> {
    let adaptation = choice.adaptation;
    let representation = adaptation
        .representations
        .get(choice.representation_index)
        .ok_or_else(|| DownloadError::fatal("DASH representation is missing"))?;
    let base = resolve_base_url(manifest_url, mpd, period, adaptation, representation)?;
    let fallback_id = format!("representation-{}", choice.representation_index);
    let id = representation
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or(&fallback_id)
        .to_string();
    let bandwidth = representation.bandwidth.unwrap_or(0);
    let segmenting = effective_segmenting(period, adaptation, representation)
        .ok_or_else(|| DownloadError::fatal("DASH representation has no segment addressing"))?;
    let (init, segments, timescale) = match segmenting {
        Segmenting::Template(template) => {
            let duration = period_duration
                .ok_or_else(|| DownloadError::fatal("DASH period duration is missing"))?;
            expand_template_pieces(template, &base, &id, bandwidth, duration)?
        }
        Segmenting::List(list) => expand_list_pieces(list, &base)?,
        Segmenting::Base(base_element) => {
            expand_base_pieces(ctx, source, headers, &base, base_element).await?
        }
    };
    if segments.is_empty() {
        return Err(DownloadError::fatal("No DASH segments found"));
    }
    if segments.len() > MAX_SEGMENTS_PER_REPRESENTATION {
        return Err(DownloadError::fatal("DASH representation contains too many segments"));
    }
    let period_seconds = period_duration.map(|duration| duration.as_secs_f64());
    let estimated_bytes =
        estimate_representation_bytes(init.as_ref(), &segments, bandwidth, period_seconds);
    Ok(RepresentationPlan {
        id,
        kind: choice.kind,
        mime_type: representation.mimeType.clone().or_else(|| adaptation.mimeType.clone()),
        codecs: representation.codecs.clone().or_else(|| adaptation.codecs.clone()),
        bandwidth,
        lang: representation.lang.clone().or_else(|| adaptation.lang.clone()),
        init,
        segments,
        timescale,
        estimated_bytes,
    })
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

fn hash_piece(hash: &mut u64, piece: &Piece) {
    let mut normalized = piece.url.clone();
    // Signed URL rotation must not invalidate the cache; a changed path or range must.
    normalized.set_query(None);
    normalized.set_fragment(None);
    hash_text(hash, normalized.as_str());
    match piece.range {
        Some(range) => {
            hash_u64(hash, 1);
            hash_u64(hash, range.start);
            hash_u64(hash, range.end);
        }
        None => hash_u64(hash, 0),
    }
}

fn plan_fingerprint(representations: &[RepresentationPlan]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    hash_text(&mut hash, "falcon-dash-plan-v1");
    for representation in representations {
        hash_text(&mut hash, &representation.id);
        hash_text(&mut hash, representation.kind.as_str());
        hash_text(&mut hash, representation.mime_type.as_deref().unwrap_or(""));
        hash_text(&mut hash, representation.codecs.as_deref().unwrap_or(""));
        hash_text(&mut hash, representation.lang.as_deref().unwrap_or(""));
        hash_u64(&mut hash, representation.bandwidth);
        hash_u64(&mut hash, representation.timescale);
        match representation.init.as_ref() {
            Some(init) => {
                hash_u64(&mut hash, 1);
                hash_piece(&mut hash, init);
            }
            None => hash_u64(&mut hash, 0),
        }
        hash_u64(&mut hash, representation.segments.len() as u64);
        for segment in &representation.segments {
            hash_piece(&mut hash, segment);
        }
    }
    format!("{hash:016x}")
}

async fn resolve_plan(
    ctx: &JobContext,
    url: &str,
    headers: &HttpHeaders,
) -> Result<Plan, DownloadError> {
    let source = validate_fetch_url_async(url).await?;
    let (bytes, manifest_url) =
        fetch_bytes(ctx, &source, &source, headers, None, MAX_MPD_BYTES, "DASH manifest").await?;
    let xml = std::str::from_utf8(&bytes)
        .map_err(|_| DownloadError::fatal("DASH manifest is not valid UTF-8"))?;
    let mpd = dash_mpd_core::parse(xml)
        .map_err(|e| DownloadError::fatal(format!("Failed to parse DASH manifest: {e}")))?;
    reject_unsupported_type(&mpd)?;
    if has_content_protection(&mpd) {
        return Err(DownloadError::fatal(ERR_DASH_DRM));
    }
    let Some((period_index, period_duration)) = select_longest_period(&mpd) else {
        return Err(DownloadError::fatal("DASH manifest has no periods"));
    };
    if mpd.periods.len() > 1 {
        log::info!(
            "download {}: multi-period DASH manifest; using the longest period only (server-side ad periods are not concatenated)",
            ctx.id
        );
    }
    let period = &mpd.periods[period_index];
    let choices = adaptation_choices(period);
    let video = choose_adaptation(&choices, MediaKind::Video);
    let audio = choose_adaptation(&choices, MediaKind::Audio);
    let mut selected = Vec::new();
    if let Some(video) = video {
        selected.push(video);
    }
    if let Some(audio) = audio {
        let video_has_audio = video.is_some_and(|choice| {
            let representation = &choice.adaptation.representations[choice.representation_index];
            representation
                .codecs
                .as_deref()
                .or(choice.adaptation.codecs.as_deref())
                .is_some_and(|codecs| codecs_include(codecs, false))
        });
        if !video_has_audio {
            selected.push(audio);
        }
    }
    if selected.is_empty() {
        return Err(DownloadError::fatal("No audio or video DASH representations found"));
    }

    let mut representations = Vec::with_capacity(selected.len());
    for choice in selected {
        representations.push(
            build_representation_plan(
                ctx,
                &source,
                headers,
                &mpd,
                &manifest_url,
                period,
                period_duration,
                choice,
            )
            .await?,
        );
    }
    let fingerprint = plan_fingerprint(&representations);
    let estimated_bytes = representations
        .iter()
        .try_fold(0u64, |total, representation| total.checked_add(representation.estimated_bytes?));
    let primary = representations
        .iter()
        .position(|representation| representation.kind == MediaKind::Video)
        .unwrap_or(0);
    Ok(Plan {
        source,
        manifest_url,
        representations,
        primary,
        estimated_bytes,
        period_seconds: period_duration.map(|duration| duration.as_secs_f64()),
        fingerprint,
    })
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PieceKind {
    Init,
    Media(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PieceKey {
    representation: usize,
    kind: PieceKind,
}

fn piece_name(key: PieceKey) -> String {
    match key.kind {
        PieceKind::Init => format!("rep{}_init.bin", key.representation),
        PieceKind::Media(index) => format!("rep{}_seg_{index:05}.bin", key.representation),
    }
}

fn piece_for(plan: &Plan, key: PieceKey) -> Result<&Piece, DownloadError> {
    let representation = plan
        .representations
        .get(key.representation)
        .ok_or_else(|| DownloadError::fatal("DASH piece plan is invalid"))?;
    match key.kind {
        PieceKind::Init => representation
            .init
            .as_ref()
            .ok_or_else(|| DownloadError::fatal("DASH piece plan is invalid")),
        PieceKind::Media(index) => representation
            .segments
            .get(index)
            .ok_or_else(|| DownloadError::fatal("DASH piece plan is invalid")),
    }
}

fn piece_keys(plan: &Plan) -> Vec<PieceKey> {
    let mut keys = Vec::new();
    for (representation, plan) in plan.representations.iter().enumerate() {
        if plan.init.is_some() {
            keys.push(PieceKey { representation, kind: PieceKind::Init });
        }
        keys.extend(
            (0..plan.segments.len())
                .map(|index| PieceKey { representation, kind: PieceKind::Media(index) }),
        );
    }
    keys
}

fn io_error(context: &str, error: &std::io::Error) -> DownloadError {
    if error.raw_os_error() == Some(28) {
        DownloadError::fatal("ERR_DISK_FULL")
    } else {
        DownloadError::fatal(format!("{context}: {error}"))
    }
}

/// One attempt: stream a piece into `tmp`, validating exact byte-range responses.
async fn fetch_piece_once(
    ctx: &JobContext,
    source: &Url,
    piece: &Piece,
    headers: &HttpHeaders,
    client: &mut HostClient,
    tmp: &Path,
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
        return Err(DownloadError::fatal("DASH segment is too large"));
    }
    let mut file = tokio::fs::File::create(tmp)
        .await
        .map_err(|e| io_error("could not create DASH segment file", &e))?;
    let mut counted = AttemptProgress::new(&ctx.progress, progress_lock);
    let mut written = 0u64;
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk
            .map_err(|e| DownloadError::transient(format!("DASH segment read failed: {e}")))?;
        written += chunk.len() as u64;
        if written > byte_limit {
            return Err(DownloadError::fatal("DASH segment is too large"));
        }
        file.write_all(&chunk).await.map_err(|e| io_error("DASH segment write failed", &e))?;
        counted.add(chunk.len() as u64);
        ctx.limiter.acquire(chunk.len() as u64, &ctx.cancel).await?;
    }
    file.flush().await.map_err(|e| io_error("DASH segment flush failed", &e))?;
    // Resume trusts this file across restarts: it must be on disk, not in cache.
    file.sync_all().await.map_err(|e| io_error("DASH segment sync failed", &e))?;
    if written == 0 {
        return Err(DownloadError::transient("DASH segment is empty"));
    }
    if response_length.is_some_and(|len| len != written)
        || required_length.is_some_and(|len| len != written)
    {
        return Err(DownloadError::transient("DASH segment is incomplete"));
    }
    counted.commit();
    Ok(written)
}

async fn write_len_sidecar(dir: &Path, key: PieceKey, bytes: u64) {
    let sidecar = dir.join(format!("{}.len", piece_name(key)));
    let written = async {
        let mut file = tokio::fs::File::create(&sidecar).await.ok()?;
        file.write_all(bytes.to_string().as_bytes()).await.ok()?;
        file.sync_all().await.ok()
    };
    if written.await.is_none() {
        log::warn!("could not write the length sidecar of DASH piece {}", piece_name(key));
    }
}

async fn fetch_piece(
    ctx: &JobContext,
    plan: &Plan,
    headers: &HttpHeaders,
    client: &mut HostClient,
    key: PieceKey,
    dir: &Path,
    progress_lock: &ProgressLock,
) -> Result<u64, DownloadError> {
    let name = piece_name(key);
    let path = dir.join(&name);
    let tmp = dir.join(format!("{name}.tmp"));
    let piece = piece_for(plan, key)?.clone();
    let mut cancel = ctx.cancel.clone();
    let mut attempt = 0u32;
    loop {
        ctx.check_cancel()?;
        let result = tokio::select! {
            result = fetch_piece_once(ctx, &plan.source, &piece, headers, client, &tmp, progress_lock) => result,
            _ = cancelled(&mut cancel) => Err(DownloadError::Cancelled),
        };
        match result {
            Ok(bytes) => {
                tokio::fs::rename(&tmp, &path)
                    .await
                    .map_err(|e| io_error("DASH segment rename failed", &e))?;
                // The sidecar lands after the segment: a crash in between just
                // re-downloads it, never trusts a partial file.
                write_len_sidecar(dir, key, bytes).await;
                return Ok(bytes);
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                attempt += 1;
                match error {
                    DownloadError::Transient(_) if attempt < SEGMENT_ATTEMPTS => {
                        sleep_or_cancel(Duration::from_millis(300u64 << attempt), &ctx.cancel)
                            .await?;
                    }
                    DownloadError::Transient(message) => {
                        return Err(DownloadError::Transient(format!(
                            "DASH piece {name} failed after retries: {message}"
                        )));
                    }
                    other => return Err(other),
                }
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct CacheManifest {
    version: u32,
    manifest: String,
    fingerprint: String,
    representations: usize,
    pieces: usize,
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("manifest.json")
}

async fn write_manifest(dir: &Path, plan: &Plan, pieces: usize) -> Result<(), DownloadError> {
    let manifest = CacheManifest {
        version: 1,
        manifest: plan.manifest_url.to_string(),
        fingerprint: plan.fingerprint.clone(),
        representations: plan.representations.len(),
        pieces,
    };
    let body = serde_json::to_vec(&manifest)
        .map_err(|e| DownloadError::fatal(format!("could not encode DASH cache manifest: {e}")))?;
    let tmp = dir.join("manifest.json.tmp");
    tokio::fs::write(&tmp, body)
        .await
        .map_err(|e| io_error("could not write DASH cache manifest", &e))?;
    tokio::fs::rename(&tmp, manifest_path(dir))
        .await
        .map_err(|e| io_error("could not write DASH cache manifest", &e))?;
    Ok(())
}

/// A cached piece is trusted only when its byte length matches the sidecar
/// written right after it completed — truncated nonzero files are corruption.
async fn trusted_piece_len(dir: &Path, key: PieceKey) -> Option<u64> {
    let name = piece_name(key);
    let meta = tokio::fs::metadata(dir.join(&name)).await.ok()?;
    if meta.len() == 0 {
        return None;
    }
    let sidecar = tokio::fs::read(dir.join(format!("{name}.len"))).await.ok()?;
    let recorded = std::str::from_utf8(&sidecar).ok()?.trim().parse::<u64>().ok()?;
    (recorded == meta.len()).then_some(meta.len())
}

/// Drop anything a previous run left when representation addressing no longer
/// matches: mixing pieces of two different plans produces a broken stream.
async fn validate_cache(dir: &Path, plan: &Plan, pieces: usize) -> Result<(), DownloadError> {
    let cached: Option<CacheManifest> = tokio::fs::read(manifest_path(dir))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let matches = cached.as_ref().is_some_and(|manifest| {
        manifest.version == 1
            && manifest.fingerprint == plan.fingerprint
            && manifest.representations == plan.representations.len()
            && manifest.pieces == pieces
    });
    if matches {
        return Ok(());
    }
    if cached.is_some() {
        log::info!("DASH manifest plan changed since the cache was written — restarting clean");
    }
    let _ = tokio::fs::remove_dir_all(dir).await;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| io_error("could not create DASH temp folder", &e))?;
    write_manifest(dir, plan, pieces).await
}

/// Fetch every missing init/media piece of all selected representations.
async fn download_pieces(
    ctx: &JobContext,
    plan: &Plan,
    headers: &HttpHeaders,
    dir: &Path,
) -> Result<(), DownloadError> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| io_error("could not create DASH temp folder", &e))?;
    let keys = piece_keys(plan);
    validate_cache(dir, plan, keys.len()).await?;
    let count = keys.len();
    let mut have_bytes = 0u64;
    let mut missing = Vec::new();
    for key in keys {
        if let Some(bytes) = trusted_piece_len(dir, key).await {
            have_bytes += bytes;
        } else {
            missing.push(key);
            let name = piece_name(key);
            let _ = tokio::fs::remove_file(dir.join(&name)).await;
            let _ = tokio::fs::remove_file(dir.join(format!("{name}.len"))).await;
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
        async move {
            let mut client: HostClient = None;
            loop {
                let Some(key) = crate::util::lock_or_recover(&queue).pop_front() else {
                    return Ok::<(), DownloadError>(());
                };
                let bytes =
                    fetch_piece(ctx, plan, headers, &mut client, key, dir, &progress_lock).await?;
                let pieces = done_pieces.fetch_add(1, Ordering::Relaxed) + 1;
                let total_bytes = done_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
                // Sizes seen so far project the total better than peak bandwidth.
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

fn is_expired_credentials(error: &DownloadError) -> bool {
    matches!(error, DownloadError::Fatal(message) if matches!(message.as_str(), "HTTP 401" | "HTTP 403" | "HTTP 410"))
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RepresentationMetadata {
    pub(crate) id: String,
    pub(crate) kind: MediaKind,
    pub(crate) mime_type: Option<String>,
    pub(crate) codecs: Option<String>,
    pub(crate) bandwidth: u64,
    pub(crate) lang: Option<String>,
    pub(crate) init_present: bool,
    pub(crate) segment_count: usize,
}

impl RepresentationMetadata {
    fn from_plan(plan: &RepresentationPlan) -> Self {
        Self {
            id: plan.id.clone(),
            kind: plan.kind,
            mime_type: plan.mime_type.clone(),
            codecs: plan.codecs.clone(),
            bandwidth: plan.bandwidth,
            lang: plan.lang.clone(),
            init_present: plan.init.is_some(),
            segment_count: plan.segments.len(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AssembledRepresentation {
    pub(crate) metadata: RepresentationMetadata,
    pub(crate) path: PathBuf,
}

/// Everything the muxer needs: concatenated per-representation files plus the
/// manifest facts required to interpret and pair them.
#[derive(Clone, Debug)]
pub(crate) struct DashAssembly {
    pub(crate) representations: Vec<AssembledRepresentation>,
    pub(crate) primary: usize,
    pub(crate) mux_manifest: PathBuf,
}

#[derive(Serialize)]
struct MuxManifestEntry {
    #[serde(flatten)]
    metadata: RepresentationMetadata,
    file: String,
}

#[derive(Serialize)]
struct MuxManifest {
    version: u32,
    primary: usize,
    period_seconds: Option<f64>,
    representations: Vec<MuxManifestEntry>,
}

fn mux_manifest_path(dir: &Path) -> PathBuf {
    dir.join("mux-manifest.json")
}

async fn write_mux_manifest(
    dir: &Path,
    primary: usize,
    period_seconds: Option<f64>,
    representations: &[AssembledRepresentation],
) -> Result<PathBuf, DownloadError> {
    let manifest = MuxManifest {
        version: 1,
        primary,
        period_seconds,
        representations: representations
            .iter()
            .map(|representation| MuxManifestEntry {
                metadata: representation.metadata.clone(),
                file: representation.path.to_string_lossy().into_owned(),
            })
            .collect(),
    };
    let body = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| DownloadError::fatal(format!("could not encode DASH mux manifest: {e}")))?;
    let path = mux_manifest_path(dir);
    let tmp = dir.join("mux-manifest.json.tmp");
    tokio::fs::write(&tmp, body)
        .await
        .map_err(|e| io_error("could not write DASH mux manifest", &e))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| io_error("could not write DASH mux manifest", &e))?;
    Ok(path)
}

async fn append_file(
    ctx: &JobContext,
    input: &Path,
    output: &mut tokio::fs::File,
) -> Result<(), DownloadError> {
    let mut input = tokio::fs::File::open(input)
        .await
        .map_err(|e| io_error("could not open cached DASH piece", &e))?;
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        ctx.check_cancel()?;
        let read = input
            .read(&mut buffer)
            .await
            .map_err(|e| io_error("could not read cached DASH piece", &e))?;
        if read == 0 {
            return Ok(());
        }
        output
            .write_all(&buffer[..read])
            .await
            .map_err(|e| io_error("could not assemble DASH representation", &e))?;
    }
}

fn extension_for_mime(mime_type: Option<&str>) -> &'static str {
    let Some(mime) = mime_type
        .and_then(|mime| mime.split(';').next())
        .map(str::trim)
        .map(str::to_ascii_lowercase)
    else {
        return "mp4";
    };
    match mime.as_str() {
        "video/mp4" => "mp4",
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => "m4a",
        "video/webm" | "audio/webm" => "webm",
        "video/x-matroska" => "mkv",
        "audio/x-matroska" => "mka",
        "audio/ogg" => "ogg",
        _ => "mp4",
    }
}

fn concat_filename(index: usize, kind: MediaKind, mime_type: Option<&str>) -> String {
    format!("rep{index}.{}.{}", kind.as_str(), extension_for_mime(mime_type))
}

/// Concatenate init + media pieces byte-wise for every selected representation
/// and write the machine-readable mux manifest next to them.
async fn assemble_representations(
    ctx: &JobContext,
    plan: &Plan,
    dir: &Path,
) -> Result<DashAssembly, DownloadError> {
    let mut representations = Vec::with_capacity(plan.representations.len());
    for (index, representation) in plan.representations.iter().enumerate() {
        let metadata = RepresentationMetadata::from_plan(representation);
        let filename =
            concat_filename(index, representation.kind, representation.mime_type.as_deref());
        let output = dir.join(filename);
        let tmp = dir.join(format!(
            "{}.tmp",
            output.file_name().and_then(|n| n.to_str()).unwrap_or("dash-output")
        ));
        let _ = tokio::fs::remove_file(&output).await;
        let _ = tokio::fs::remove_file(&tmp).await;
        let mut file = tokio::fs::File::create(&tmp)
            .await
            .map_err(|e| io_error("could not create DASH output", &e))?;
        if representation.init.is_some() {
            append_file(
                ctx,
                &dir.join(piece_name(PieceKey { representation: index, kind: PieceKind::Init })),
                &mut file,
            )
            .await?;
        }
        for segment in 0..representation.segments.len() {
            append_file(
                ctx,
                &dir.join(piece_name(PieceKey {
                    representation: index,
                    kind: PieceKind::Media(segment),
                })),
                &mut file,
            )
            .await?;
        }
        file.flush().await.map_err(|e| io_error("could not finish DASH output", &e))?;
        file.sync_all().await.map_err(|e| io_error("could not sync DASH output", &e))?;
        drop(file);
        tokio::fs::rename(&tmp, &output)
            .await
            .map_err(|e| io_error("could not finish DASH output", &e))?;
        representations.push(AssembledRepresentation { metadata, path: output });
    }
    let mux_manifest =
        write_mux_manifest(dir, plan.primary, plan.period_seconds, &representations).await?;
    Ok(DashAssembly { representations, primary: plan.primary, mux_manifest })
}

/// A name ending in a manifest extension must not hold media bytes:
/// `movie.mpd` becomes `movie.mp4` or `movie.m4a` from the representation MIME.
fn container_corrected_name(dest: &Path, mime_type: Option<&str>) -> Option<String> {
    let ext = dest.extension().and_then(|e| e.to_str())?.to_ascii_lowercase();
    if ext != "mpd" {
        return None;
    }
    let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("download");
    Some(format!("{stem}.{}", extension_for_mime(mime_type)))
}

fn audio_companion_target(landed: &Path, mime_type: Option<&str>) -> Option<PathBuf> {
    let stem = landed.file_stem()?.to_str()?;
    Some(landed.parent()?.join(format!("{stem}.audio.{}", extension_for_mime(mime_type))))
}

/// Keep the temporary split-audio concatenation next to the delivered video.
/// This deliberately uses the finished primary name: if `video.mp4` landed as
/// `video (1).mp4`, its companion cannot collide with another download's audio.
async fn place_audio_companion(
    audio: &AssembledRepresentation,
    landed: &Path,
) -> Result<PathBuf, DownloadError> {
    let Some(target) = audio_companion_target(landed, audio.metadata.mime_type.as_deref()) else {
        return Err(DownloadError::fatal("DASH audio output filename is invalid"));
    };
    let source = audio.path.clone();
    tokio::task::spawn_blocking(move || {
        crate::util::move_file_unique(&source, &target, &std::collections::HashSet::new())
    })
    .await
    .map_err(|e| DownloadError::fatal(format!("DASH audio output worker failed: {e}")))?
    .map_err(DownloadError::fatal)
}

/// Deliver the assembled representations through the single-file job pipeline.
async fn finalize_assembly(
    ctx: &JobContext,
    assembly: &DashAssembly,
    dest: &Path,
) -> Result<PathBuf, DownloadError> {
    // muxer integration point: consume `assembly` here and hand one muxed temp file to `ctx.finalize` instead of the primary concatenation plus companion.
    ctx.progress.set_merging();
    if assembly.representations.len() > 1 {
        log::debug!(
            "download {}: DASH mux metadata is at {}",
            ctx.id,
            assembly.mux_manifest.display()
        );
    }
    let primary = assembly
        .representations
        .get(assembly.primary)
        .ok_or_else(|| DownloadError::fatal("DASH primary representation is missing"))?;
    let mut dest = dest.to_path_buf();
    if let Some(wanted) = container_corrected_name(&dest, primary.metadata.mime_type.as_deref()) {
        dest = ctx.rename(&dest, &wanted);
    }
    let landed = ctx.finalize(&primary.path, &dest).await?;
    if let Some(audio) = assembly
        .representations
        .iter()
        .enumerate()
        .find(|(index, representation)| {
            *index != assembly.primary && representation.metadata.kind == MediaKind::Audio
        })
        .map(|(_, representation)| representation)
    {
        if let Err(error) = place_audio_companion(audio, &landed).await {
            // The primary deliverable is already safely finalized. Losing the
            // temporary companion must not turn that successful download into
            // a failed row; the muxer path will replace this transitional copy.
            log::warn!(
                "download {}: could not keep DASH audio companion: {}",
                ctx.id,
                error.message()
            );
        }
    }
    Ok(landed)
}

/// Download a DASH presentation to `out_path`. Returns where the file landed.
pub async fn process_dash_stream(
    ctx: &JobContext,
    url: &str,
    out_path: &str,
    headers: HttpHeaders,
) -> Result<PathBuf, DownloadError> {
    let (clean_url, _) = split_falcon_format(url);
    let dest = PathBuf::from(out_path);
    let _ticker = ctx.progress.start();
    let mut plan = resolve_plan(ctx, &clean_url, &headers).await?;
    let dir = dash_session_dir(ctx.id);
    // Signed segment URLs can expire mid-download. Re-resolving the manifest
    // mints fresh tokens once; query-insensitive fingerprints keep valid cache.
    let mut reresolved = false;
    loop {
        match download_pieces(ctx, &plan, &headers, &dir).await {
            Ok(()) => break,
            Err(error) if !reresolved && is_expired_credentials(&error) => {
                reresolved = true;
                log::info!(
                    "download {}: DASH segment URLs rejected ({}), re-resolving the manifest once",
                    ctx.id,
                    error.message()
                );
                plan = resolve_plan(ctx, &clean_url, &headers).await?;
            }
            Err(error) => return Err(error),
        }
    }

    let assembly = assemble_representations(ctx, &plan, &dir).await?;
    let landed = finalize_assembly(ctx, &assembly, &dest).await?;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    Ok(landed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn template(xml: &str) -> SegmentTemplate {
        let mpd = format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT10S"><Period duration="PT10S"><AdaptationSet contentType="video" mimeType="video/mp4">{xml}<Representation id="v" bandwidth="1000"/></AdaptationSet></Period></MPD>"#
        );
        let parsed = dash_mpd_core::parse(&mpd).unwrap();
        parsed.periods[0].adaptations[0].SegmentTemplate.clone().unwrap()
    }

    #[test]
    fn template_substitution_supports_width_time_bandwidth_and_escape() {
        assert_eq!(
            render_template(
                "$RepresentationID$/seg-$Number%05d$-$Time$-$Bandwidth$-$$-tail.m4s",
                "video-1",
                250_000,
                7,
                12_345,
            ),
            "video-1/seg-00007-12345-250000-$-tail.m4s"
        );
        assert_eq!(render_template("plain.m4s", "v", 1, 2, 3), "plain.m4s");
    }

    #[test]
    fn byte_range_parsing_validates_order() {
        assert_eq!(parse_byte_range("0-99").unwrap(), ByteRange { start: 0, end: 99 });
        assert_eq!(parse_byte_range(" 100-199 ").unwrap().len(), 100);
        assert!(parse_byte_range("199-100").is_err());
        assert!(parse_byte_range("x-100").is_err());
        assert!(parse_byte_range("100").is_err());
    }

    #[test]
    fn duration_template_counts_ceil_and_starts_at_start_number() {
        let tpl = template(
            r#"<SegmentTemplate timescale="1000" duration="2000" startNumber="7" media="seg-$Number%05d$.m4s" initialization="init.mp4"/>"#,
        );
        let points = expand_duration_segments(&tpl, Duration::from_secs(10)).unwrap();
        assert_eq!(points.len(), 5);
        assert_eq!(points[0], SegmentPoint { number: 7, time: 0 });
        assert_eq!(points[4], SegmentPoint { number: 11, time: 8000 });

        let tpl = template(
            r#"<SegmentTemplate timescale="1000" duration="3000" startNumber="1" media="seg$Number$.m4s"/>"#,
        );
        let points = expand_duration_segments(&tpl, Duration::from_secs(10)).unwrap();
        assert_eq!(points.len(), 4, "ceil(10*1000/3000)");
    }

    #[test]
    fn timeline_repeat_counts_additional_segments_and_negative_repeat_ends_at_period() {
        let positive = template(
            r#"<SegmentTemplate timescale="1000" startNumber="1" media="s$Number$.m4s"><SegmentTimeline><S t="0" d="2000" r="5"/></SegmentTimeline></SegmentTemplate>"#,
        );
        let points = expand_timeline_segments(&positive, Duration::from_secs(20)).unwrap();
        assert_eq!(points.len(), 6, "r=5 means the first segment plus five repetitions");
        assert_eq!(points[5].time, 10_000);

        let negative = template(
            r#"<SegmentTemplate timescale="1000" startNumber="1" media="s$Number$.m4s"><SegmentTimeline><S t="0" d="2000" r="-1"/></SegmentTimeline></SegmentTemplate>"#,
        );
        let points = expand_timeline_segments(&negative, Duration::from_secs(10)).unwrap();
        assert_eq!(points.len(), 5);
        assert_eq!(
            points.iter().map(|p| p.time).collect::<Vec<_>>(),
            vec![0, 2000, 4000, 6000, 8000]
        );
    }

    #[test]
    fn timeline_explicit_times_reset_the_clock() {
        let tpl = template(
            r#"<SegmentTemplate timescale="1000" startNumber="1" media="s$Time$.m4s"><SegmentTimeline><S t="100" d="2000" r="1"/><S t="5000" d="1000" r="1"/></SegmentTimeline></SegmentTemplate>"#,
        );
        let points = expand_timeline_segments(&tpl, Duration::from_secs(10)).unwrap();
        assert_eq!(points.iter().map(|p| p.time).collect::<Vec<_>>(), vec![100, 2100, 5000, 6000]);
    }

    fn sidx_box(sizes: &[u32], first_offset: u64) -> Vec<u8> {
        let payload_len = 4 + 4 + 4 + 4 + 4 + 2 + 2 + sizes.len() * 12;
        let mut out = Vec::new();
        out.extend_from_slice(&(8 + payload_len as u32).to_be_bytes());
        out.extend_from_slice(b"sidx");
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(&1000u32.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(first_offset as u32).to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&(sizes.len() as u16).to_be_bytes());
        for size in sizes {
            out.extend_from_slice(&size.to_be_bytes());
            out.extend_from_slice(&1000u32.to_be_bytes());
            out.extend_from_slice(&0x9000_0000u32.to_be_bytes());
        }
        out
    }

    #[test]
    fn sidx_references_become_absolute_byte_ranges() {
        let init_len = 16u64;
        let sidx = sidx_box(&[20, 25], 3);
        let range = ByteRange { start: init_len, end: init_len + sidx.len() as u64 - 1 };
        let ranges = parse_sidx_segments(&sidx, range).unwrap();
        let media0 = range.end + 1 + 3;
        assert_eq!(
            ranges,
            vec![
                ByteRange { start: media0, end: media0 + 19 },
                ByteRange { start: media0 + 20, end: media0 + 44 },
            ]
        );
    }

    #[test]
    fn session_dir_is_per_download() {
        assert_eq!(dash_session_dirname(7), "dash-7.falcondm-temp");
        assert!(dash_session_dir(7).ends_with("downloads_temp/dash-7.falcondm-temp"));
    }

    #[test]
    fn audio_companion_follows_the_finished_primary_name() {
        assert_eq!(
            audio_companion_target(Path::new("/tmp/movie (1).mp4"), Some("audio/mp4")),
            Some(PathBuf::from("/tmp/movie (1).audio.m4a"))
        );
    }

    fn headers(connections: usize) -> HttpHeaders {
        HttpHeaders { max_connections: connections, ..Default::default() }
    }

    fn test_id() -> i64 {
        (uuid::Uuid::new_v4().as_u128() & 0x7fff_ffff_ffff_ffff) as i64
    }

    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("falcon-dash-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct Fixture {
        base: String,
        hits: Arc<std::sync::Mutex<HashMap<String, usize>>>,
    }

    /// Static file server with exact inclusive Range support. `fail_first`
    /// paths answer 503 once so retry behavior is observable in hit counts.
    async fn serve(files: HashMap<String, Vec<u8>>, fail_first: Vec<String>) -> Fixture {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let files = Arc::new(files);
        let fail_first = Arc::new(fail_first);
        let hits = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits_c = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let (files, fail_first, hits) = (files.clone(), fail_first.clone(), hits_c.clone());
                tokio::spawn(async move {
                    let mut request_bytes = Vec::new();
                    let mut chunk = [0u8; 8192];
                    loop {
                        let Ok(n) = sock.read(&mut chunk).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        request_bytes.extend_from_slice(&chunk[..n]);
                        if request_bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                        if request_bytes.len() > 64 * 1024 {
                            return;
                        }
                    }
                    let request = String::from_utf8_lossy(&request_bytes).to_string();
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .split('?')
                        .next()
                        .unwrap_or("/")
                        .to_string();
                    let range_header = request.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("range").then(|| value.trim().to_string())
                    });
                    let mut ranged = None;
                    if let Some(value) = range_header.as_deref() {
                        ranged = (|| {
                            let spec = value.strip_prefix("bytes=")?;
                            let (start, end) = spec.split_once('-')?;
                            let start = start.parse::<usize>().ok()?;
                            let end = end.parse::<usize>().ok()?;
                            (start <= end).then_some((start, end))
                        })();
                    }
                    let hit_key = ranged
                        .map(|(start, end)| format!("{path}@{start}-{end}"))
                        .unwrap_or_else(|| path.clone());
                    let seen = {
                        let mut hits = hits.lock().unwrap();
                        let entry = hits.entry(hit_key).or_insert(0);
                        *entry += 1;
                        *entry
                    };
                    if seen == 1 && fail_first.contains(&path) {
                        let _ = sock.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        return;
                    }
                    let Some(body) = files.get(&path) else {
                        let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        return;
                    };
                    if let Some((start, end)) = ranged {
                        if end >= body.len() {
                            let _ = sock.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                            return;
                        }
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
                    if range_header.is_some() {
                        let _ = sock.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
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

    fn one_file_manifest(body: &str) -> HashMap<String, Vec<u8>> {
        HashMap::from([("/index.mpd".to_string(), body.as_bytes().to_vec())])
    }

    #[test]
    fn negative_repeat_respects_presentation_time_offset() {
        let tpl = template(
            r#"<SegmentTemplate timescale="1000" presentationTimeOffset="1000" startNumber="1" media="s$Time$.m4s"><SegmentTimeline><S t="1000" d="2000" r="-1"/></SegmentTimeline></SegmentTemplate>"#,
        );
        let points = expand_timeline_segments(&tpl, Duration::from_secs(10)).unwrap();
        assert_eq!(points.len(), 5);
        assert_eq!(points[0].time, 1000);
        assert_eq!(points[4].time, 9000);
    }

    fn fingerprint_representation(
        id: &str,
        timescale: u64,
        init_range: Option<ByteRange>,
        segments: usize,
        query: &str,
    ) -> RepresentationPlan {
        let url =
            |name: &str| Url::parse(&format!("https://cdn.example.com/{name}?{query}")).unwrap();
        RepresentationPlan {
            id: id.into(),
            kind: MediaKind::Video,
            mime_type: Some("video/mp4".into()),
            codecs: Some("avc1.64001f".into()),
            bandwidth: 1_000_000,
            lang: None,
            init: Some(Piece { url: url("init.mp4"), range: init_range }),
            segments: (0..segments)
                .map(|index| Piece { url: url(&format!("seg{index}.m4s")), range: None })
                .collect(),
            timescale,
            estimated_bytes: None,
        }
    }

    #[test]
    fn fingerprint_covers_plan_facts_and_ignores_signed_queries() {
        let range = Some(ByteRange { start: 0, end: 99 });
        let base = plan_fingerprint(&[fingerprint_representation("v1", 1000, range, 3, "tok=A")]);
        let rotated = plan_fingerprint(&[fingerprint_representation(
            "v1",
            1000,
            range,
            3,
            "tok=B&expires=9",
        )]);
        assert_eq!(base, rotated, "signed-URL rotation must not invalidate the cache");
        assert_ne!(
            base,
            plan_fingerprint(&[fingerprint_representation("v2", 1000, range, 3, "tok=A")]),
            "representation id matters"
        );
        assert_ne!(
            base,
            plan_fingerprint(&[fingerprint_representation("v1", 9000, range, 3, "tok=A")]),
            "timescale matters"
        );
        assert_ne!(
            base,
            plan_fingerprint(&[fingerprint_representation(
                "v1",
                1000,
                Some(ByteRange { start: 0, end: 100 }),
                3,
                "tok=A"
            )]),
            "init range matters"
        );
        assert_ne!(
            base,
            plan_fingerprint(&[fingerprint_representation("v1", 1000, range, 2, "tok=A")]),
            "segment count matters"
        );
    }

    #[tokio::test]
    async fn template_plan_selects_tiebreak_video_and_resolves_nested_baseurls() {
        let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT10S">
            <BaseURL>cdn/</BaseURL>
            <Period duration="PT10S"><BaseURL>p/</BaseURL>
              <AdaptationSet contentType="video" mimeType="video/mp4" codecs="avc1.64001f"><BaseURL>a/</BaseURL>
                <SegmentTemplate timescale="1000" duration="2000" startNumber="7" media="seg-$Number%05d$.m4s" initialization="init-$RepresentationID$.mp4"/>
                <Representation id="vlow" bandwidth="800000" width="640" height="360"><BaseURL>low/</BaseURL></Representation>
                <Representation id="vhigh" bandwidth="800000" width="1280" height="720"><BaseURL>high/</BaseURL></Representation>
              </AdaptationSet>
              <AdaptationSet contentType="audio" mimeType="audio/mp4" codecs="mp4a.40.2" lang="tr"><BaseURL>a/</BaseURL>
                <SegmentTemplate timescale="1000" duration="5000" startNumber="1" media="aud-$Time$.m4s" initialization="ainit.mp4"/>
                <Representation id="a1" bandwidth="96000"/>
              </AdaptationSet>
            </Period></MPD>"#;
        let mut files = one_file_manifest(mpd);
        let body = files["/index.mpd"].clone();
        files.insert("/root/index.mpd".into(), body);
        let fixture = serve(files, Vec::new()).await;
        let (ctx, _tx) = JobContext::for_tests(1);
        let _g = crate::util::allow_loopback_for_tests();
        let plan = resolve_plan(&ctx, &format!("{}/root/index.mpd", fixture.base), &headers(2))
            .await
            .unwrap();
        assert_eq!(plan.representations.len(), 2);
        let video = &plan.representations[0];
        assert_eq!(video.id, "vhigh", "equal bandwidth uses the width/height tiebreak");
        assert_eq!(video.segments.len(), 5);
        assert!(video
            .init
            .as_ref()
            .unwrap()
            .url
            .as_str()
            .ends_with("/root/cdn/p/a/high/init-vhigh.mp4"));
        assert!(video.segments[0].url.as_str().ends_with("/root/cdn/p/a/high/seg-00007.m4s"));
        assert!(video.segments[4].url.as_str().ends_with("/root/cdn/p/a/high/seg-00011.m4s"));
        let audio = &plan.representations[1];
        assert_eq!(audio.lang.as_deref(), Some("tr"));
        assert!(audio.segments[0].url.as_str().ends_with("/root/cdn/p/a/aud-0.m4s"));
        assert!(audio.segments[1].url.as_str().ends_with("/root/cdn/p/a/aud-5000.m4s"));
    }

    #[tokio::test]
    async fn segment_list_supports_initialization_media_and_byte_ranges() {
        let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT4S">
          <Period duration="PT4S"><AdaptationSet contentType="video" mimeType="video/mp4">
            <Representation id="v" bandwidth="1000"><BaseURL>media/</BaseURL>
              <SegmentList timescale="1000">
                <Initialization sourceURL="init.mp4" range="2-9"/>
                <SegmentURL media="one.m4s"/>
                <SegmentURL media="big.bin" mediaRange="100-119"/>
              </SegmentList>
            </Representation>
          </AdaptationSet></Period></MPD>"#;
        let fixture = serve(one_file_manifest(mpd), Vec::new()).await;
        let (ctx, _tx) = JobContext::for_tests(2);
        let _g = crate::util::allow_loopback_for_tests();
        let plan =
            resolve_plan(&ctx, &format!("{}/index.mpd", fixture.base), &headers(1)).await.unwrap();
        let rep = &plan.representations[0];
        assert_eq!(rep.init.as_ref().unwrap().range, Some(ByteRange { start: 2, end: 9 }));
        assert!(rep.init.as_ref().unwrap().url.as_str().ends_with("/media/init.mp4"));
        assert_eq!(rep.segments.len(), 2);
        assert_eq!(rep.segments[0].range, None);
        assert_eq!(rep.segments[1].range, Some(ByteRange { start: 100, end: 119 }));
    }

    #[tokio::test]
    async fn adaptive_download_assembles_video_audio_and_mux_metadata() {
        let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT4S">
          <BaseURL>media/</BaseURL><Period duration="PT4S">
            <AdaptationSet contentType="video" mimeType="video/mp4" codecs="avc1.64001f">
              <SegmentTemplate timescale="1000" duration="2000" startNumber="1" media="v$Number$.m4s" initialization="vinit.mp4"/>
              <Representation id="v1" bandwidth="800000" width="640" height="360"/>
              <Representation id="v2" bandwidth="800000" width="1280" height="720"/>
            </AdaptationSet>
            <AdaptationSet contentType="audio" mimeType="audio/mp4" codecs="mp4a.40.2" lang="tr">
              <SegmentTemplate timescale="1000" duration="2000" startNumber="1" media="a$Time$.m4s" initialization="ainit.mp4"/>
              <Representation id="a1" bandwidth="96000"/>
            </AdaptationSet>
          </Period></MPD>"#;
        let mut files = one_file_manifest(mpd);
        files.insert("/media/vinit.mp4".into(), b"VINIT".to_vec());
        files.insert("/media/v1.m4s".into(), b"V1".to_vec());
        files.insert("/media/v2.m4s".into(), b"V2".to_vec());
        files.insert("/media/ainit.mp4".into(), b"AINIT".to_vec());
        files.insert("/media/a0.m4s".into(), b"A0".to_vec());
        files.insert("/media/a2000.m4s".into(), b"A2".to_vec());
        let fixture = serve(files, Vec::new()).await;
        let (ctx, _tx) = JobContext::for_tests(3);
        let _g = crate::util::allow_loopback_for_tests();
        let plan =
            resolve_plan(&ctx, &format!("{}/index.mpd", fixture.base), &headers(4)).await.unwrap();
        let dir = fresh_dir("assembly");
        download_pieces(&ctx, &plan, &headers(4), &dir).await.unwrap();
        let assembly = assemble_representations(&ctx, &plan, &dir).await.unwrap();
        assert_eq!(std::fs::read(&assembly.representations[0].path).unwrap(), b"VINITV1V2");
        assert_eq!(std::fs::read(&assembly.representations[1].path).unwrap(), b"AINITA0A2");
        assert!(assembly.mux_manifest.exists());
        let mux: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&assembly.mux_manifest).unwrap()).unwrap();
        assert_eq!(mux["primary"], 0);
        assert_eq!(mux["representations"][0]["kind"], "video");
        assert_eq!(mux["representations"][0]["mime_type"], "video/mp4");
        assert_eq!(mux["representations"][0]["init_present"], true);
        assert_eq!(mux["representations"][0]["segment_count"], 2);
        assert_eq!(mux["representations"][1]["kind"], "audio");
        assert_eq!(mux["representations"][1]["lang"], "tr");
        assert_eq!(mux["representations"][1]["codecs"], "mp4a.40.2");
        assert_eq!(ctx.progress.downloaded(), (5 + 2 + 2 + 5 + 2 + 2) as u64);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn resume_skips_complete_pieces_and_retries_transient_failures() {
        let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT3S">
          <Period duration="PT3S"><AdaptationSet contentType="video" mimeType="video/mp4">
            <SegmentTemplate timescale="1000" duration="1000" startNumber="1" media="v$Number$.m4s" initialization="vinit.mp4"/>
            <Representation id="v" bandwidth="1000"/>
          </AdaptationSet></Period></MPD>"#;
        let mut files = one_file_manifest(mpd);
        files.insert("/vinit.mp4".into(), b"INIT".to_vec());
        files.insert("/v1.m4s".into(), b"ONE".to_vec());
        files.insert("/v2.m4s".into(), b"TWO".to_vec());
        files.insert("/v3.m4s".into(), b"THREE".to_vec());
        let fixture = serve(files, vec!["/v2.m4s".into()]).await;
        let (ctx, _tx) = JobContext::for_tests(4);
        let _g = crate::util::allow_loopback_for_tests();
        let plan =
            resolve_plan(&ctx, &format!("{}/index.mpd", fixture.base), &headers(3)).await.unwrap();
        let dir = fresh_dir("resume");
        download_pieces(&ctx, &plan, &headers(3), &dir).await.unwrap();
        assert_eq!(fixture.hits.lock().unwrap()["/v2.m4s"], 2, "503 is retried");

        std::fs::remove_file(dir.join("rep0_seg_00001.bin")).unwrap();
        std::fs::remove_file(dir.join("rep0_seg_00001.bin.len")).unwrap();
        std::fs::write(dir.join("rep0_seg_00001.bin.tmp"), b"partial").unwrap();
        let (ctx2, _tx2) = JobContext::for_tests(5);
        download_pieces(&ctx2, &plan, &headers(3), &dir).await.unwrap();
        let hits = fixture.hits.lock().unwrap().clone();
        assert_eq!(hits["/vinit.mp4"], 1, "complete init is not refetched");
        assert_eq!(hits["/v1.m4s"], 1, "complete segment is not refetched");
        assert_eq!(hits["/v3.m4s"], 1, "complete segment is not refetched");
        assert_eq!(hits["/v2.m4s"], 3, "only the lost segment is fetched once more");
        assert_eq!(std::fs::read(dir.join("rep0_seg_00001.bin")).unwrap(), b"TWO");
        assert!(!dir.join("rep0_seg_00001.bin.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn segment_base_media() -> (Vec<u8>, ByteRange, Vec<ByteRange>, Vec<u8>) {
        let init = b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00isomiso2".to_vec();
        let media0 = b"MEDIA-SEGMENT-ZERO".to_vec();
        let media1 = b"MEDIA-SEGMENT-ONE--LONGER".to_vec();
        let sidx = sidx_box(&[media0.len() as u32, media1.len() as u32], 0);
        let mut file = init.clone();
        let index_start = file.len() as u64;
        file.extend_from_slice(&sidx);
        let index_end = file.len() as u64 - 1;
        let media0_start = file.len() as u64;
        file.extend_from_slice(&media0);
        let media0_end = file.len() as u64 - 1;
        let media1_start = file.len() as u64;
        file.extend_from_slice(&media1);
        let media1_end = file.len() as u64 - 1;
        let expected = [init, media0, media1].concat();
        (
            file,
            ByteRange { start: index_start, end: index_end },
            vec![
                ByteRange { start: media0_start, end: media0_end },
                ByteRange { start: media1_start, end: media1_end },
            ],
            expected,
        )
    }

    #[tokio::test]
    async fn segment_base_downloads_indexed_byte_ranges_and_finalizes_concat() {
        let (media, index_range, media_ranges, expected) = segment_base_media();
        let mpd = format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT4S"><Period duration="PT4S"><AdaptationSet contentType="video" mimeType="video/mp4"><Representation id="v" bandwidth="1000"><BaseURL>single.mp4</BaseURL><SegmentBase indexRange="{}-{}"><Initialization range="0-23"/></SegmentBase></Representation></AdaptationSet></Period></MPD>"#,
            index_range.start, index_range.end
        );
        let mut files = one_file_manifest(&mpd);
        files.insert("/single.mp4".into(), media);
        let fixture = serve(files, Vec::new()).await;
        let out_dir = fresh_dir("segment-base-out");
        let dest = out_dir.join("single.mpd");
        let id = test_id();
        remove_artifacts(id);
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_dash_stream(
            &ctx,
            &format!("{}/index.mpd", fixture.base),
            dest.to_str().unwrap(),
            headers(3),
        )
        .await
        .unwrap();
        assert_eq!(landed.file_name().unwrap(), "single.mp4");
        assert_eq!(std::fs::read(&landed).unwrap(), expected);
        let hits = fixture.hits.lock().unwrap().clone();
        assert_eq!(hits[&format!("/single.mp4@{}-{}", index_range.start, index_range.end)], 1);
        assert_eq!(hits["/single.mp4@0-23"], 1);
        for range in media_ranges {
            assert_eq!(hits[&format!("/single.mp4@{}-{}", range.start, range.end)], 1);
        }
        std::fs::remove_dir_all(out_dir).unwrap();
        remove_artifacts(id);
    }

    #[tokio::test]
    async fn dynamic_and_drm_manifests_get_dedicated_error_codes() {
        let dynamic = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"><Period duration="PT2S"/></MPD>"#;
        let fixture = serve(one_file_manifest(dynamic), Vec::new()).await;
        let (ctx, _tx) = JobContext::for_tests(6);
        let _g = crate::util::allow_loopback_for_tests();
        let result = resolve_plan(&ctx, &format!("{}/index.mpd", fixture.base), &headers(1)).await;
        assert!(matches!(result, Err(DownloadError::Fatal(ref m)) if m == ERR_DASH_LIVE));

        let drm = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static"><Period duration="PT2S"><AdaptationSet contentType="video"><ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/><Representation id="v" bandwidth="1"/></AdaptationSet></Period></MPD>"#;
        let fixture = serve(one_file_manifest(drm), Vec::new()).await;
        let result = resolve_plan(&ctx, &format!("{}/index.mpd", fixture.base), &headers(1)).await;
        assert!(matches!(result, Err(DownloadError::Fatal(ref m)) if m == ERR_DASH_DRM));
    }

    #[tokio::test]
    async fn longest_period_is_selected_in_multi_period_manifest() {
        let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT12S">
          <Period duration="PT2S"><AdaptationSet contentType="video" mimeType="video/mp4"><BaseURL>short/</BaseURL><SegmentTemplate timescale="1000" duration="1000" media="s$Number$.m4s"/><Representation id="v" bandwidth="1"/></AdaptationSet></Period>
          <Period duration="PT10S"><AdaptationSet contentType="video" mimeType="video/mp4"><BaseURL>long/</BaseURL><SegmentTemplate timescale="1000" duration="1000" media="s$Number$.m4s"/><Representation id="v" bandwidth="1"/></AdaptationSet></Period>
        </MPD>"#;
        let fixture = serve(one_file_manifest(mpd), Vec::new()).await;
        let (ctx, _tx) = JobContext::for_tests(7);
        let _g = crate::util::allow_loopback_for_tests();
        let plan =
            resolve_plan(&ctx, &format!("{}/index.mpd", fixture.base), &headers(1)).await.unwrap();
        assert_eq!(plan.representations[0].segments.len(), 10);
        assert!(plan.representations[0].segments[0].url.as_str().ends_with("/long/s1.m4s"));
    }
}
