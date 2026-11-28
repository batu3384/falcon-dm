//! Format-selector subset parser, validator and evaluator.
//!
//! Supported grammar (anything else is a hard parse error — fail closed):
//!
//! ```text
//! chain   := entry ('/' entry)*        — fallback order, first match wins
//! entry   := term ('+' term)?          — `+` merges a video and an audio stream
//! term    := name filter*
//! name    := bestvideo | bv | bestaudio | ba | best | b | bv* | ba* | b*
//! filter  := '[' (height OP N | ext=VALUE) ']'   OP := <= | < | >= | > | =
//! ```
//!
//! Semantics cover every preset the app and the
//! extension emit, with three deliberate native adaptations:
//!
//! 1. **Strict height cap.** When any entry of the chain carries an explicit
//!    height upper bound, that cap applies chain-wide: a fallback entry without
//!    its own filter (the classic trailing `/bv*+ba/b`) may still only pick at or
//!    below the cap. The user chose 720p — the download must never come back
//!    1080p. Entries with their own bound keep the tighter of their own.
//! 2. **MP4-only merges.** The native muxer writes MP4, so a merge pair is
//!    only viable with an MP4 video stream and MP4 (AAC) audio. A merge entry
//!    that can only resolve to WebM/Opus input is treated as unfulfillable and
//!    the chain falls through — typically to a progressive MP4 — instead of
//!    failing the download. When no MP4-compatible pair and no progressive
//!    format exist at all, selection fails with `ERR_YT_FORMAT_UNAVAILABLE`.
//! 3. **Progressive merge short-circuit.** When the video term of a merge
//!    (`bv*` can do this) resolves to a format that already carries audio, the
//!    entry degrades to that single progressive file — merging in a second
//!    audio track would be meaningless.

use crate::download::youtube::{audio_rank, video_rank, CodecFamily, YoutubeFormat};

/// Audio container extensions the enqueue logic recognizes for audio-only picks.
pub fn is_audio_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "m4a" | "mp3" | "opus" | "ogg" | "flac" | "wav" | "aac" | "weba"
    )
}

/// Default quality ceiling applied when the user did not pick one.
pub const DEFAULT_VIDEO_FORMAT: &str =
    "bestvideo[height<=1080][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=1080]+bestaudio/best[height<=1080]/bv*+ba/b";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeightOp {
    Le,
    Lt,
    Ge,
    Gt,
    Eq,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Filter {
    Height(HeightOp, u32),
    Ext(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TermKind {
    /// `bestvideo`/`bv`: video-only formats.
    VideoOnly,
    /// `bestaudio`/`ba`: audio-only formats.
    AudioOnly,
    /// `best`/`b`: formats carrying both video and audio.
    Progressive,
    /// `bv*`: any format with a video stream.
    AnyVideo,
    /// `ba*`: any format with an audio stream.
    AnyAudio,
    /// `b*`: any format at all.
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Term {
    kind: TermKind,
    filters: Vec<Filter>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    video: Term,
    audio: Option<Term>,
}

/// A parsed, validated selector chain with its derived strict height cap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chain {
    entries: Vec<Entry>,
    /// Smallest explicit height upper bound anywhere in the chain.
    cap: Option<u32>,
}

/// What a chain entry resolved to against one extraction's format list.
#[derive(Clone, Debug, PartialEq)]
pub enum Selection {
    /// One file carries everything the user gets: progressive media, an
    /// audio-only track, or a bare video-only stream the selector asked for.
    Single(Box<YoutubeFormat>),
    /// Adaptive pair destined for the native MP4 muxer. Boxed: the variant
    /// would otherwise double the enum size for the rarest case.
    Merge { video: Box<YoutubeFormat>, audio: Box<YoutubeFormat> },
}

impl Selection {
    pub fn is_audio_only(&self) -> bool {
        matches!(self, Self::Single(format) if format.is_audio_only())
    }

    /// The stream that decides the output container/filename.
    pub fn primary(&self) -> &YoutubeFormat {
        match self {
            Self::Single(format) => format,
            Self::Merge { video, .. } => video,
        }
    }
}

const NAMES: &[(&str, TermKind)] = &[
    ("bestvideo", TermKind::VideoOnly),
    ("bestaudio", TermKind::AudioOnly),
    ("best", TermKind::Progressive),
    ("bv*", TermKind::AnyVideo),
    ("ba*", TermKind::AnyAudio),
    ("b*", TermKind::Any),
    ("bv", TermKind::VideoOnly),
    ("ba", TermKind::AudioOnly),
    ("b", TermKind::Progressive),
];

fn parse_term(input: &str) -> Result<Term, String> {
    let lower = input.trim().to_ascii_lowercase();
    let Some((name, kind)) = NAMES.iter().find(|(name, _)| lower.starts_with(name)) else {
        return Err(format!("unsupported selector term '{input}'"));
    };
    let mut rest = &lower[name.len()..];
    let mut filters = Vec::new();
    while !rest.is_empty() {
        if !rest.starts_with('[') {
            return Err(format!("unsupported selector syntax in '{input}'"));
        }
        let Some(end) = rest.find(']') else {
            return Err(format!("unterminated filter in '{input}'"));
        };
        let body = &rest[1..end];
        filters.push(parse_filter(body).ok_or_else(|| format!("unsupported filter '[{body}]'"))?);
        rest = &rest[end + 1..];
    }
    Ok(Term { kind: *kind, filters })
}

fn parse_filter(body: &str) -> Option<Filter> {
    if let Some(rest) = body.strip_prefix("height") {
        for (op_text, op) in [
            ("<=", HeightOp::Le),
            (">=", HeightOp::Ge),
            ("<", HeightOp::Lt),
            (">", HeightOp::Gt),
            ("=", HeightOp::Eq),
        ] {
            if let Some(value) = rest.strip_prefix(op_text) {
                return value.parse::<u32>().ok().map(|n| Filter::Height(op, n));
            }
        }
        return None;
    }
    if let Some(ext) = body.strip_prefix("ext=") {
        if ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        return Some(Filter::Ext(ext.to_string()));
    }
    None
}

/// Parse and validate a selector. Anything outside the subset is an error —
/// silently misinterpreting a quality choice is worse than refusing it.
pub fn parse_chain(input: &str) -> Result<Chain, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("empty format selector".into());
    }
    let mut entries = Vec::new();
    for entry_text in trimmed.split('/') {
        let entry_text = entry_text.trim();
        if entry_text.is_empty() {
            return Err("empty fallback entry in selector".into());
        }
        let terms: Vec<&str> = entry_text.split('+').collect();
        if terms.len() > 2 {
            return Err(format!("multi-way merges are not supported: '{entry_text}'"));
        }
        let video = parse_term(terms[0])?;
        let audio = match terms.get(1) {
            Some(text) => Some(parse_term(text)?),
            None => None,
        };
        entries.push(Entry { video, audio });
    }
    let cap = entries
        .iter()
        .flat_map(|entry| {
            let mut terms = vec![&entry.video];
            if let Some(audio) = &entry.audio {
                terms.push(audio);
            }
            terms
        })
        .flat_map(|term| term.filters.iter())
        .filter_map(|filter| match filter {
            Filter::Height(HeightOp::Le, n) => Some(*n),
            Filter::Height(HeightOp::Lt, n) => Some(n.saturating_sub(1)),
            Filter::Height(HeightOp::Eq, n) => Some(*n),
            _ => None,
        })
        .min();
    Ok(Chain { entries, cap })
}

fn height_passes(height: Option<u32>, op: HeightOp, value: u32) -> bool {
    // Audio-only formats carry no height; height filters are a video concept
    // and must not silently discard audio tracks.
    let Some(height) = height else {
        return true;
    };
    match op {
        HeightOp::Le => height <= value,
        HeightOp::Lt => height < value,
        HeightOp::Ge => height >= value,
        HeightOp::Gt => height > value,
        HeightOp::Eq => height == value,
    }
}

fn ext_passes(format: &YoutubeFormat, ext: &str) -> bool {
    if format.container == ext {
        return true;
    }
    // `m4a` is the selector spelling of an audio-only MP4 (AAC) track.
    ext == "m4a" && format.is_audio_only() && format.container == "mp4"
}

fn term_matches(format: &YoutubeFormat, term: &Term) -> bool {
    let kind_matches = match term.kind {
        TermKind::VideoOnly => format.is_video_only(),
        TermKind::AudioOnly => format.is_audio_only(),
        TermKind::Progressive => format.progressive,
        TermKind::AnyVideo => format.has_video,
        TermKind::AnyAudio => format.has_audio,
        TermKind::Any => true,
    };
    kind_matches
        && term.filters.iter().all(|filter| match filter {
            Filter::Height(op, value) => height_passes(format.height, *op, *value),
            Filter::Ext(ext) => ext_passes(format, ext),
        })
}

/// Effective cap for one entry: its own explicit upper bound when present,
/// otherwise the strict chain-wide cap. Only formats with video are capped.
fn capped<'a>(
    formats: impl Iterator<Item = &'a YoutubeFormat>,
    entry_cap: Option<u32>,
) -> impl Iterator<Item = &'a YoutubeFormat> {
    formats.filter(move |format| match (entry_cap, format.height.filter(|_| format.has_video)) {
        (Some(cap), Some(height)) => height <= cap,
        _ => true,
    })
}

fn entry_own_cap(term: &Term) -> Option<u32> {
    term.filters
        .iter()
        .filter_map(|filter| match filter {
            Filter::Height(HeightOp::Le, n) => Some(*n),
            Filter::Height(HeightOp::Lt, n) => Some(n.saturating_sub(1)),
            Filter::Height(HeightOp::Eq, n) => Some(*n),
            _ => None,
        })
        .min()
}

fn best<'a>(
    formats: impl Iterator<Item = &'a YoutubeFormat>,
    video: bool,
) -> Option<&'a YoutubeFormat> {
    if video {
        formats.max_by_key(|format| video_rank(format))
    } else {
        formats.max_by_key(|format| audio_rank(format))
    }
}

fn pick_term<'a>(
    formats: &'a [YoutubeFormat],
    term: &Term,
    cap: Option<u32>,
    video_ranked: bool,
) -> Option<&'a YoutubeFormat> {
    let cap = entry_own_cap(term).or(cap);
    best(capped(formats.iter().filter(|format| term_matches(format, term)), cap), video_ranked)
}

/// The native muxer reads MP4 inputs only: video must be an MP4 fragment and
/// audio an MP4/AAC track. Opus or WebM input makes the entry unfulfillable.
fn merge_compatible_video(format: &YoutubeFormat) -> bool {
    format.container == "mp4" && format.is_video_only()
}

fn merge_compatible_audio(format: &YoutubeFormat) -> bool {
    format.container == "mp4"
        && format.is_audio_only()
        && !matches!(format.codec_family, CodecFamily::Opus)
}

fn eval_entry(
    formats: &[YoutubeFormat],
    entry: &Entry,
    chain_cap: Option<u32>,
) -> Option<Selection> {
    let video_is_video = !matches!(entry.video.kind, TermKind::AudioOnly | TermKind::AnyAudio);
    let picked_video = pick_term(formats, &entry.video, chain_cap, video_is_video)?;
    let Some(audio_term) = &entry.audio else {
        return Some(Selection::Single(Box::new(picked_video.clone())));
    };
    // A progressive pick already carries its audio; the merge half is moot.
    if picked_video.progressive || picked_video.is_audio_only() {
        return Some(Selection::Single(Box::new(picked_video.clone())));
    }
    if !merge_compatible_video(picked_video) {
        return None;
    }
    let audio_cap = entry_own_cap(audio_term).or(chain_cap);
    let audio = best(
        capped(
            formats.iter().filter(|format| {
                term_matches(format, audio_term) && merge_compatible_audio(format)
            }),
            audio_cap,
        ),
        false,
    )?;
    Some(Selection::Merge { video: Box::new(picked_video.clone()), audio: Box::new(audio.clone()) })
}

/// Resolve the chain against one extraction. `None` means no entry could be
/// fulfilled — the caller reports `ERR_YT_FORMAT_UNAVAILABLE`.
pub fn select(formats: &[YoutubeFormat], chain: &Chain) -> Option<Selection> {
    for entry in &chain.entries {
        if let Some(selection) = eval_entry(formats, entry, chain.cap) {
            return Some(selection);
        }
    }
    None
}

/// Parse + select in one step for callers holding a raw selector string.
pub fn select_str(formats: &[YoutubeFormat], selector: &str) -> Result<Option<Selection>, String> {
    parse_chain(selector).map(|chain| select(formats, &chain))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(
        itag: u32,
        container: &str,
        codec: CodecFamily,
        height: Option<u32>,
        bitrate: u64,
    ) -> YoutubeFormat {
        let has_video = height.is_some();
        YoutubeFormat {
            itag,
            url: format!("https://rr1---sn-fixture.googlevideo.com/videoplayback?itag={itag}"),
            mime_type: String::new(),
            container: container.into(),
            codec_family: codec,
            codecs: None,
            width: height.map(|h| h * 16 / 9),
            height,
            bitrate: Some(bitrate),
            content_length: Some(bitrate * 10),
            audio_quality: None,
            progressive: false,
            has_video,
            has_audio: !has_video,
            quality_label: None,
            fps: Some(30.0),
            was_ciphered: false,
        }
    }

    fn progressive(itag: u32, height: u32) -> YoutubeFormat {
        let mut f = format(itag, "mp4", CodecFamily::H264, Some(height), 400_000);
        f.progressive = true;
        f.has_audio = true;
        f
    }

    /// Realistic YouTube ladder: progressive 360p, adaptive MP4 (avc) + WebM
    /// (vp9) at every height, m4a AAC + webm opus audio.
    fn ladder() -> Vec<YoutubeFormat> {
        vec![
            progressive(18, 360),
            format(160, "mp4", CodecFamily::H264, Some(144), 100_000),
            format(134, "mp4", CodecFamily::H264, Some(360), 600_000),
            format(136, "mp4", CodecFamily::H264, Some(720), 2_000_000),
            format(137, "mp4", CodecFamily::H264, Some(1080), 4_300_000),
            format(248, "webm", CodecFamily::Vp9, Some(1080), 2_600_000),
            format(399, "mp4", CodecFamily::Av1, Some(1080), 2_400_000),
            format(140, "mp4", CodecFamily::Aac, None, 128_000),
            format(251, "webm", CodecFamily::Opus, None, 132_000),
        ]
    }

    fn parse(input: &str) -> Chain {
        parse_chain(input).unwrap_or_else(|error| panic!("'{input}' must parse: {error}"))
    }

    fn picked(input: &str, formats: &[YoutubeFormat]) -> Selection {
        select(formats, &parse(input)).unwrap_or_else(|| panic!("'{input}' must select"))
    }

    #[test]
    fn every_preset_the_app_and_extension_emit_parses() {
        for preset in [
            DEFAULT_VIDEO_FORMAT,
            "best",
            "bestvideo[height<=2160]+bestaudio/best[height<=2160]",
            "bestvideo[height<=1440]+bestaudio/best[height<=1440]",
            "bestvideo[height<=720][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=720]+bestaudio/best[height<=720]",
            "bestvideo[height<=480][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=480]+bestaudio/best[height<=480]",
            "bestaudio[ext=m4a]/bestaudio",
            // extension overlay presets for arbitrary heights
            "bestvideo[height<=1080]+bestaudio/best[height<=1080]/bv*+ba/b",
            "bestvideo[height<=720]+bestaudio/best[height<=720]/bv*+ba/b",
            "bestvideo[height<=480]+bestaudio/best[height<=480]/bv*+ba/b",
            "bestvideo[height<=360]+bestaudio/best[height<=360]/bv*+ba/b",
        ] {
            parse(preset);
        }
    }

    #[test]
    fn junk_outside_the_subset_is_rejected() {
        for bad in [
            "",
            "bestvideo[height<=720",
            "worstvideo",
            "bestvideo[width<=720]",
            "bestvideo[height<=abc]",
            "bestvideo+bestaudio+bestvideo",
            "bestvideo[format_id=137]",
            "137+140",
            "bestvideo/",
            "/bestvideo",
            "bestvideo[height!=720]",
        ] {
            assert!(parse_chain(bad).is_err(), "'{bad}' must not parse");
        }
    }

    #[test]
    fn default_chain_picks_capped_h264_mp4_plus_aac() {
        let selection = picked(DEFAULT_VIDEO_FORMAT, &ladder());
        let Selection::Merge { video, audio } = selection else {
            panic!("default must merge: {selection:?}");
        };
        assert_eq!(video.itag, 137, "avc beats av1 at equal height");
        assert_eq!(audio.itag, 140, "opus can never merge — aac wins in merge context");
    }

    #[test]
    fn height_cap_is_strict_across_fallback_entries() {
        // No MP4 video-only within 720: mp4 video-only starts at 1080, webm has
        // 720, progressive is 360. An uncapped trailing bv*+ba/b would escape to
        // 1080p; the strict cap must fall back to lower/progressive instead.
        let mut formats = vec![
            progressive(18, 360),
            format(137, "mp4", CodecFamily::H264, Some(1080), 4_300_000),
            format(247, "webm", CodecFamily::Vp9, Some(720), 1_500_000),
            format(140, "mp4", CodecFamily::Aac, None, 128_000),
        ];
        let chain = "bestvideo[height<=720]+bestaudio/best[height<=720]/bv*+ba/b";
        let selection = picked(chain, &formats);
        assert_eq!(
            selection,
            Selection::Single(Box::new(formats[0].clone())),
            "strict cap falls back to the progressive 360p, never to 1080p"
        );
        // Same proof for the explicit-quality frontend chain.
        formats.push(format(136, "mp4", CodecFamily::H264, Some(720), 2_000_000));
        let selection = picked(
            "bestvideo[height<=720][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=720]+bestaudio/best[height<=720]",
            &formats,
        );
        let Selection::Merge { video, .. } = selection else {
            panic!("720p mp4 exists now — must merge: {selection:?}");
        };
        assert_eq!(video.itag, 136);
    }

    #[test]
    fn uncapped_chain_may_reach_the_top() {
        let selection = picked("bv*+ba/b", &ladder());
        let Selection::Merge { video, .. } = selection else {
            panic!("uncapped bv*+ba must merge: {selection:?}");
        };
        assert_eq!(video.height, Some(1080));
    }

    #[test]
    fn audio_chain_prefers_m4a_then_any_audio() {
        let ladder = ladder();
        let selection = picked("bestaudio[ext=m4a]/bestaudio", &ladder);
        assert_eq!(selection.primary().itag, 140);
        assert!(selection.is_audio_only());
        // Without any m4a the fallback may legally land on opus — a plain file
        // download needs no merge-compatible container.
        let opus_only: Vec<_> = ladder.iter().filter(|f| f.itag == 251).cloned().collect();
        let selection = picked("bestaudio[ext=m4a]/bestaudio", &opus_only);
        assert_eq!(selection.primary().itag, 251);
    }

    #[test]
    fn unmergeable_pair_falls_through_to_progressive() {
        // Only webm video + opus audio: nothing can enter the MP4 muxer.
        let formats = vec![
            progressive(18, 360),
            format(248, "webm", CodecFamily::Vp9, Some(1080), 2_600_000),
            format(251, "webm", CodecFamily::Opus, None, 132_000),
        ];
        let selection = picked("bv*+ba/b", &formats);
        assert_eq!(selection, Selection::Single(Box::new(formats[0].clone())));
        // Without progressive the chain exhausts: caller reports unavailable.
        let no_prog = &formats[1..];
        assert!(select(no_prog, &parse("bv*+ba/b")).is_none());
    }

    #[test]
    fn opus_only_audio_is_a_fatal_format_unavailable_not_a_merge_attempt() {
        let formats = vec![
            format(137, "mp4", CodecFamily::H264, Some(1080), 4_300_000),
            format(251, "webm", CodecFamily::Opus, None, 132_000),
        ];
        assert!(select(&formats, &parse("bestvideo+bestaudio")).is_none());
    }

    #[test]
    fn best_picks_the_best_progressive_only() {
        let selection = picked("best", &ladder());
        assert_eq!(selection, Selection::Single(Box::new(progressive(18, 360))));
    }

    #[test]
    fn ext_filter_distinguishes_containers() {
        let selection = picked("bestvideo[ext=webm]+bestaudio[ext=m4a]/best", &ladder());
        // webm video cannot merge — the entry is unfulfillable, falls to best.
        assert_eq!(selection, Selection::Single(Box::new(progressive(18, 360))));
        // mp4 audio ext filter selects the aac track specifically.
        let selection = picked("bestaudio[ext=m4a]", &ladder());
        assert_eq!(selection.primary().itag, 140);
    }

    #[test]
    fn entry_with_own_cap_keeps_it_when_chain_cap_differs() {
        // First entry tighter than second: own cap wins per entry.
        let formats = vec![
            format(136, "mp4", CodecFamily::H264, Some(720), 2_000_000),
            format(134, "mp4", CodecFamily::H264, Some(360), 600_000),
            format(140, "mp4", CodecFamily::Aac, None, 128_000),
        ];
        let selection =
            picked("bestvideo[height<=360]+bestaudio/bestvideo[height<=720]+bestaudio", &formats);
        let Selection::Merge { video, .. } = selection else {
            panic!("must merge: {selection:?}");
        };
        assert_eq!(video.height, Some(360), "first entry wins with its own tighter cap");
    }

    #[test]
    fn bv_star_in_merge_degrades_to_progressive_single() {
        // No video-only formats at all: bv* can only resolve to progressive.
        let formats =
            vec![progressive(18, 360), format(140, "mp4", CodecFamily::Aac, None, 128_000)];
        let selection = picked("bv*+ba/b", &formats);
        assert_eq!(selection, Selection::Single(Box::new(progressive(18, 360))));
    }

    #[test]
    fn height_operators_parse_and_compare() {
        let chain = parse("bestvideo[height<1080]+bestaudio");
        assert_eq!(chain.cap, Some(1079));
        let chain = parse("bestvideo[height=720]+bestaudio");
        assert_eq!(chain.cap, Some(720));
        let formats = ladder();
        let selection = picked("bestvideo[height<1080][ext=mp4]+bestaudio[ext=m4a]", &formats);
        let Selection::Merge { video, .. } = selection else {
            panic!("must merge: {selection:?}");
        };
        assert_eq!(video.height, Some(720));
    }
}
