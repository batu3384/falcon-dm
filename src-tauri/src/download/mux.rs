//! Streaming MPEG-TS remuxing without external tools.
//!
//! The MP4 path is deliberately two-pass over the segment files: the first
//! pass measures the sample tables with zero media payloads in memory, the
//! second pass writes payload bytes directly to disk. The demuxer's event
//! queue is drained after every read in both passes, so memory scales with
//! the MP4 sample tables rather than with the media byte size.
//!
//! The writer boundary is byte-oriented rather than TS-specific: a future
//! fragmented-MP4 input only needs to produce the same length-prefixed sample
//! stream, codec configuration and timing metadata for DASH audio+video merge
//! support.

use shiguredo_mp4::boxes::{
    AudioSampleEntryFields, Avc1Box, AvccBox, ElstBox, ElstEntry, EsdsBox, Hvc1Box, HvccBox,
    HvccNalUintArray, Mp4aBox, SampleEntry, VisualSampleEntryFields,
};
use shiguredo_mp4::descriptors::{
    DecoderConfigDescriptor, DecoderSpecificInfo, EsDescriptor, SlConfigDescriptor,
};
use shiguredo_mp4::mux::{
    EditList, Mp4FileMuxer, Mp4FileMuxerOptions, Sample as Mp4Sample, TrackMetadata,
};
use shiguredo_mp4::{FixedPointNumber, TrackKind, Uint};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use transmux::{CodecConfig, DemuxEvent, StreamingTsDemux, TrackId, TrackSpec};

const IO_CHUNK_BYTES: usize = 1024 * 1024;
/// ffmpeg's traditional movie timescale. Using it as the edit-list input unit
/// keeps A/V lead offsets close to the oracle; shiguredo converts them to the
/// output movie timescale it chooses.
const EDIT_TIMESCALE: u32 = 90_000;
/// Edit lists are absent from the sizing pass. This small reserve covers the
/// two `edts` subtrees and final ftyp brand growth while wasting only a few
/// hundred bytes as a `free` box.
const EDIT_LIST_RESERVE_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemuxError {
    Cancelled,
    Fatal(String),
}

impl RemuxError {
    fn fatal(message: impl Into<String>) -> Self {
        Self::Fatal(message.into())
    }
}

struct PartialOutput<'a> {
    path: &'a Path,
    keep: bool,
}

impl<'a> PartialOutput<'a> {
    fn new(path: &'a Path) -> Self {
        let _ = std::fs::remove_file(path);
        Self { path, keep: false }
    }

    fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for PartialOutput<'_> {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(self.path);
        }
    }
}

fn check_cancel(should_cancel: &dyn Fn() -> bool) -> Result<(), RemuxError> {
    if should_cancel() {
        Err(RemuxError::Cancelled)
    } else {
        Ok(())
    }
}

fn open_input(path: &Path) -> Result<std::fs::File, RemuxError> {
    std::fs::File::open(path)
        .map_err(|e| RemuxError::fatal(format!("could not open {}: {e}", path.display())))
}

/// Byte-concatenate MPEG-TS segments for an explicit `.ts` destination.
pub(crate) fn concat_ts_files(
    inputs: &[PathBuf],
    output: &Path,
    should_cancel: &dyn Fn() -> bool,
) -> Result<(), RemuxError> {
    let cleanup = PartialOutput::new(output);
    let mut writer = std::fs::File::create(output)
        .map_err(|e| RemuxError::fatal(format!("could not create {}: {e}", output.display())))?;
    let mut buf = vec![0u8; IO_CHUNK_BYTES];
    for input in inputs {
        let mut reader = open_input(input)?;
        loop {
            check_cancel(should_cancel)?;
            let n = reader.read(&mut buf).map_err(|e| {
                RemuxError::fatal(format!("could not read {}: {e}", input.display()))
            })?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n]).map_err(|e| {
                RemuxError::fatal(format!("could not write {}: {e}", output.display()))
            })?;
        }
    }
    writer
        .flush()
        .map_err(|e| RemuxError::fatal(format!("could not write {}: {e}", output.display())))?;
    cleanup.keep();
    Ok(())
}

fn visual_fields(width: u16, height: u16) -> VisualSampleEntryFields {
    VisualSampleEntryFields {
        data_reference_index: VisualSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
        width,
        height,
        horizresolution: VisualSampleEntryFields::DEFAULT_HORIZRESOLUTION,
        vertresolution: VisualSampleEntryFields::DEFAULT_VERTRESOLUTION,
        frame_count: VisualSampleEntryFields::DEFAULT_FRAME_COUNT,
        compressorname: VisualSampleEntryFields::NULL_COMPRESSORNAME,
        depth: VisualSampleEntryFields::DEFAULT_DEPTH,
    }
}

fn avc_entry(config: &transmux::AVCConfigurationBox, width: u16, height: u16) -> SampleEntry {
    let record = &config.config;
    SampleEntry::Avc1(Avc1Box {
        visual: visual_fields(width, height),
        avcc_box: AvccBox {
            avc_profile_indication: record.profile_indication,
            profile_compatibility: record.profile_compatibility,
            avc_level_indication: record.level_indication,
            length_size_minus_one: Uint::new(record.length_size_minus_one),
            sps_list: record.sps.iter().map(|sps| sps.0.clone()).collect(),
            pps_list: record.pps.iter().map(|pps| pps.0.clone()).collect(),
            chroma_format: record.chroma_format.map(Uint::new),
            bit_depth_luma_minus8: record.bit_depth_luma_minus8.map(Uint::new),
            bit_depth_chroma_minus8: record.bit_depth_chroma_minus8.map(Uint::new),
            sps_ext_list: record.sps_ext.iter().map(|sps| sps.0.clone()).collect(),
        },
        unknown_boxes: Vec::new(),
    })
}

fn hevc_entry(config: &transmux::HEVCConfigurationBox, width: u16, height: u16) -> SampleEntry {
    let record = &config.config;
    SampleEntry::Hvc1(Hvc1Box {
        visual: visual_fields(width, height),
        hvcc_box: HvccBox {
            general_profile_space: Uint::new(record.general_profile_space),
            general_tier_flag: Uint::new(u8::from(record.general_tier_flag)),
            general_profile_idc: Uint::new(record.general_profile_idc),
            general_profile_compatibility_flags: record.general_profile_compatibility_flags,
            general_constraint_indicator_flags: Uint::new(
                record.general_constraint_indicator_flags,
            ),
            general_level_idc: record.general_level_idc,
            min_spatial_segmentation_idc: Uint::new(record.min_spatial_segmentation_idc),
            parallelism_type: Uint::new(record.parallelism_type),
            chroma_format_idc: Uint::new(record.chroma_format_idc),
            bit_depth_luma_minus8: Uint::new(record.bit_depth_luma_minus8),
            bit_depth_chroma_minus8: Uint::new(record.bit_depth_chroma_minus8),
            avg_frame_rate: record.avg_frame_rate,
            constant_frame_rate: Uint::new(record.constant_frame_rate),
            num_temporal_layers: Uint::new(record.num_temporal_layers),
            temporal_id_nested: Uint::new(u8::from(record.temporal_id_nested)),
            length_size_minus_one: Uint::new(record.length_size_minus_one),
            nalu_arrays: record
                .arrays
                .iter()
                .map(|array| HvccNalUintArray {
                    array_completeness: Uint::new(u8::from(array.array_completeness)),
                    nal_unit_type: Uint::new(array.nal_unit_type),
                    nalus: array.nalus.iter().map(|nalu| nalu.0.clone()).collect(),
                })
                .collect(),
        },
        unknown_boxes: Vec::new(),
    })
}

fn aac_entry(
    esds: &transmux::EsdsBox,
    channel_count: u16,
    sample_rate: u32,
    sample_size: u16,
) -> Result<SampleEntry, RemuxError> {
    let decoder_config = esds
        .es_descriptor
        .decoder_config
        .as_ref()
        .ok_or_else(|| RemuxError::fatal("AAC track has no decoder configuration"))?;
    let specific = decoder_config
        .decoder_specific_info
        .as_ref()
        .ok_or_else(|| RemuxError::fatal("AAC track has no AudioSpecificConfig"))?;
    let sample_rate_integer = u16::try_from(sample_rate).map_err(|_| {
        RemuxError::fatal(format!("AAC sample rate {sample_rate} does not fit an mp4a entry"))
    })?;
    Ok(SampleEntry::Mp4a(Mp4aBox {
        audio: AudioSampleEntryFields {
            data_reference_index: AudioSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
            channelcount: channel_count,
            samplesize: sample_size,
            samplerate: FixedPointNumber::new(sample_rate_integer, 0u16),
        },
        esds_box: EsdsBox {
            es: EsDescriptor {
                es_id: esds.es_descriptor.es_id.max(EsDescriptor::MIN_ES_ID),
                stream_priority: EsDescriptor::LOWEST_STREAM_PRIORITY,
                depends_on_es_id: None,
                url_string: None,
                ocr_es_id: None,
                dec_config_descr: DecoderConfigDescriptor {
                    object_type_indication:
                        DecoderConfigDescriptor::OBJECT_TYPE_INDICATION_AUDIO_ISO_IEC_14496_3,
                    stream_type: DecoderConfigDescriptor::STREAM_TYPE_AUDIO,
                    up_stream: DecoderConfigDescriptor::UP_STREAM_FALSE,
                    buffer_size_db: Uint::new(0),
                    max_bitrate: decoder_config.max_bitrate,
                    avg_bitrate: decoder_config.avg_bitrate,
                    dec_specific_info: Some(DecoderSpecificInfo { payload: specific.data.clone() }),
                },
                sl_config_descr: SlConfigDescriptor,
            },
        },
        unknown_boxes: Vec::new(),
    }))
}

fn classify_spec(spec: &TrackSpec) -> Result<Option<(TrackKind, SampleEntry)>, RemuxError> {
    match &spec.config {
        CodecConfig::Avc { config, width, height } => {
            Ok(Some((TrackKind::Video, avc_entry(config, *width, *height))))
        }
        CodecConfig::Hevc { config, width, height } => {
            Ok(Some((TrackKind::Video, hevc_entry(config, *width, *height))))
        }
        CodecConfig::Aac { esds, channel_count, sample_rate, sample_size } => {
            aac_entry(esds, *channel_count, *sample_rate, *sample_size)
                .map(|entry| Some((TrackKind::Audio, entry)))
        }
        CodecConfig::Data { .. } => Err(RemuxError::fatal(
            "unsupported data track in MPEG-TS stream: native remux would drop it",
        )),
        CodecConfig::Subtitle { .. } => Err(RemuxError::fatal(
            "unsupported subtitle track in MPEG-TS stream: native remux would drop it",
        )),
        other => {
            Err(RemuxError::fatal(format!("unsupported codec in MPEG-TS stream: {}", other.name())))
        }
    }
}

struct TrackBuilder {
    spec: TrackSpec,
    kind: TrackKind,
    timescale: NonZeroU32,
    sample_entry: SampleEntry,
    sample_count: u64,
    payload_bytes: u64,
    total_duration: u64,
    first_dts: Option<i64>,
    first_pts: Option<i64>,
    last_dts: Option<i64>,
    last_duration: Option<u32>,
}

impl TrackBuilder {
    fn new(
        spec: TrackSpec,
        kind: TrackKind,
        sample_entry: SampleEntry,
    ) -> Result<Self, RemuxError> {
        let timescale = NonZeroU32::new(spec.timescale.get()).expect("timescale is non-zero");
        if kind == TrackKind::Video && timescale.get() != 90_000 {
            return Err(RemuxError::fatal(format!(
                "video timescale {} is unsupported (expected 90000)",
                timescale.get()
            )));
        }
        if let CodecConfig::Aac { sample_rate, .. } = &spec.config {
            if kind == TrackKind::Audio && timescale.get() != *sample_rate {
                return Err(RemuxError::fatal(format!(
                    "AAC timescale {} does not match sample rate {sample_rate}",
                    timescale.get()
                )));
            }
        }
        Ok(Self {
            spec,
            kind,
            timescale,
            sample_entry,
            sample_count: 0,
            payload_bytes: 0,
            total_duration: 0,
            first_dts: None,
            first_pts: None,
            last_dts: None,
            last_duration: None,
        })
    }

    fn rebuild(&mut self, spec: TrackSpec) -> Result<(), RemuxError> {
        let Some((kind, sample_entry)) = classify_spec(&spec)? else {
            return Err(RemuxError::fatal("supported track became a data track"));
        };
        if kind != self.kind {
            return Err(RemuxError::fatal("track changed media kind mid-stream"));
        }
        let rebuilt = Self::new(spec, kind, sample_entry)?;
        self.timescale = rebuilt.timescale;
        self.sample_entry = rebuilt.sample_entry;
        self.spec = rebuilt.spec;
        Ok(())
    }

    fn record_sample(
        &mut self,
        timing: SampleTimingInfo,
        payload_len: usize,
    ) -> Result<(), RemuxError> {
        // A timestamp-reset segment is concatenated as a new continuous
        // timeline: MP4 stts durations are monotonic by construction, while
        // composition offsets are unaffected by a uniform rebase.
        if self.last_dts.is_some_and(|last| timing.dts < last) {
            log::debug!(
                "timestamp reset in {:?} track; continuing on the output timeline",
                self.kind
            );
        }
        if self.sample_count == 0 {
            self.first_dts = Some(timing.dts);
            self.first_pts = Some(timing.pts);
        }
        self.last_dts = Some(timing.dts);
        self.last_duration = Some(timing.duration);
        self.sample_count += 1;
        self.payload_bytes += payload_len as u64;
        self.total_duration += u64::from(timing.duration);
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct SampleTimingInfo {
    dts: i64,
    pts: i64,
    duration: u32,
    composition_offset: i64,
}

fn sample_timing(sample: &transmux::Sample) -> Result<SampleTimingInfo, RemuxError> {
    let timing =
        sample.timing.ok_or_else(|| RemuxError::fatal("sample has no timing information"))?;
    // Wire DTS wins when present; DTS=PTS is the fallback required for PES
    // packets that carry only a presentation timestamp.
    let dts = timing
        .dts()
        .or_else(|| timing.pts())
        .ok_or_else(|| RemuxError::fatal("sample has neither DTS nor PTS"))?;
    let pts = timing.pts().unwrap_or(dts);
    // Missing duration metadata is represented as zero here and repaired by
    // the caller from the previous frame. This happens at timestamp-reset
    // seams; audio/video access units themselves never have a zero duration.
    let duration = timing.duration().unwrap_or(0);
    Ok(SampleTimingInfo { dts, pts, duration, composition_offset: pts - dts })
}

#[derive(Default)]
struct TrackBuilders {
    tracks: BTreeMap<TrackId, TrackBuilder>,
    saw_video: bool,
    saw_audio: bool,
}

impl TrackBuilders {
    fn add_or_update(&mut self, spec: TrackSpec) -> Result<(), RemuxError> {
        if let Some(existing) = self.tracks.get_mut(&spec.track_id) {
            if existing.sample_count == 0
                && (existing.spec.config != spec.config
                    || existing.spec.timescale != spec.timescale)
            {
                return existing.rebuild(spec);
            }
            if existing.spec.config != spec.config || existing.spec.timescale != spec.timescale {
                return Err(RemuxError::fatal(
                    "codec configuration changes mid-stream are unsupported",
                ));
            }
            return Ok(());
        }

        let Some((kind, sample_entry)) = classify_spec(&spec)? else {
            log::debug!(
                "ignoring {} track {} in MPEG-TS input",
                spec.config.name(),
                spec.track_id.value()
            );
            return Ok(());
        };
        match kind {
            TrackKind::Video if self.saw_video => {
                return Err(RemuxError::fatal(
                    "multiple video tracks in one MPEG-TS stream are unsupported",
                ));
            }
            TrackKind::Audio if self.saw_audio => {
                return Err(RemuxError::fatal(
                    "multiple audio tracks in one MPEG-TS stream are unsupported",
                ));
            }
            TrackKind::Video => self.saw_video = true,
            TrackKind::Audio => self.saw_audio = true,
            _ => {}
        }
        let id = spec.track_id;
        self.tracks.insert(id, TrackBuilder::new(spec, kind, sample_entry)?);
        Ok(())
    }
}

#[derive(Clone)]
struct TrackPlan {
    track_id: TrackId,
    kind: TrackKind,
    timescale: NonZeroU32,
    sample_count: u64,
    payload_bytes: u64,
    total_duration: u64,
    first_dts: i64,
    first_pts: i64,
}

impl From<&TrackBuilder> for Option<TrackPlan> {
    fn from(track: &TrackBuilder) -> Self {
        if track.sample_count == 0 {
            return None;
        }
        Some(TrackPlan {
            track_id: track.spec.track_id,
            kind: track.kind,
            timescale: track.timescale,
            sample_count: track.sample_count,
            payload_bytes: track.payload_bytes,
            total_duration: track.total_duration,
            first_dts: track.first_dts.expect("sampled track"),
            first_pts: track.first_pts.expect("sampled track"),
        })
    }
}

fn planned_tracks(tracks: &TrackBuilders) -> Vec<TrackPlan> {
    tracks.tracks.values().filter_map(Option::<TrackPlan>::from).collect()
}

fn cmp_ratio(a_ticks: i64, a_scale: u32, b_ticks: i64, b_scale: u32) -> std::cmp::Ordering {
    ((a_ticks as i128) * (b_scale as i128)).cmp(&((b_ticks as i128) * (a_scale as i128)))
}

fn round_ratio(numerator: i128, denominator: i128) -> u64 {
    debug_assert!(numerator >= 0);
    debug_assert!(denominator > 0);
    u64::try_from((numerator + denominator / 2) / denominator).unwrap_or(u64::MAX)
}

fn edit_delay(plan: &TrackPlan, origin_ticks: i64, origin_scale: u32) -> u64 {
    let numerator = (plan.first_pts as i128) * (origin_scale as i128)
        - (origin_ticks as i128) * (plan.timescale.get() as i128);
    let denominator = (plan.timescale.get() as i128) * (origin_scale as i128);
    round_ratio(numerator * i128::from(EDIT_TIMESCALE), denominator)
}

fn scaled_duration(duration: u64, media_timescale: NonZeroU32) -> u64 {
    round_ratio((duration as i128) * i128::from(EDIT_TIMESCALE), i128::from(media_timescale.get()))
}

fn edit_list(plan: &TrackPlan, origin_ticks: i64, origin_scale: u32) -> EditList {
    let delay = edit_delay(plan, origin_ticks, origin_scale);
    let media_duration = scaled_duration(plan.total_duration, plan.timescale).max(1);
    let mut entries = Vec::new();
    if delay > 0 {
        entries.push(ElstEntry {
            edit_duration: delay,
            media_time: -1,
            media_rate: FixedPointNumber::new(1i16, 0i16),
        });
    }
    entries.push(ElstEntry {
        edit_duration: media_duration,
        media_time: plan.first_pts - plan.first_dts,
        media_rate: FixedPointNumber::new(1i16, 0i16),
    });
    EditList {
        movie_timescale: NonZeroU32::new(EDIT_TIMESCALE).expect("non-zero"),
        media_timescale: plan.timescale,
        elst_box: ElstBox { entries },
    }
}

fn muxer_options(plans: &[TrackPlan], reserved_moov_box_size: usize) -> Mp4FileMuxerOptions {
    let mut options = Mp4FileMuxerOptions { reserved_moov_box_size, ..Default::default() };
    let Some(first) = plans.first() else {
        return options;
    };
    let (origin_ticks, origin_scale) =
        plans.iter().skip(1).fold((first.first_pts, first.timescale.get()), |origin, plan| {
            if cmp_ratio(plan.first_pts, plan.timescale.get(), origin.0, origin.1).is_lt() {
                (plan.first_pts, plan.timescale.get())
            } else {
                origin
            }
        });
    for plan in plans {
        let metadata = TrackMetadata {
            edit_list: Some(edit_list(plan, origin_ticks, origin_scale)),
            ..Default::default()
        };
        match plan.kind {
            TrackKind::Video => options.video_track = metadata,
            TrackKind::Audio => options.audio_track = metadata,
            _ => {}
        }
    }
    options
}

fn drain_events(
    demux: &mut StreamingTsDemux,
    on_event: &mut dyn FnMut(DemuxEvent) -> Result<(), RemuxError>,
) -> Result<(), RemuxError> {
    while let Some(event) = demux.poll_event() {
        on_event(event)?;
    }
    Ok(())
}

fn stream_events(
    inputs: &[PathBuf],
    should_cancel: &dyn Fn() -> bool,
    mut on_event: impl FnMut(DemuxEvent) -> Result<(), RemuxError>,
) -> Result<(), RemuxError> {
    let mut demux = StreamingTsDemux::new().with_strict_pmt_order();
    let mut buf = vec![0u8; IO_CHUNK_BYTES];
    for input in inputs {
        let mut reader = open_input(input)?;
        loop {
            check_cancel(should_cancel)?;
            let n = reader.read(&mut buf).map_err(|e| {
                RemuxError::fatal(format!("could not read {}: {e}", input.display()))
            })?;
            if n == 0 {
                break;
            }
            demux
                .feed(&buf[..n])
                .map_err(|e| RemuxError::fatal(format!("MPEG-TS parse failed: {e}")))?;
            drain_events(&mut demux, &mut on_event)?;
        }
    }
    demux.finish().map_err(|e| RemuxError::fatal(format!("MPEG-TS parse failed: {e}")))?;
    drain_events(&mut demux, &mut on_event)?;
    check_cancel(should_cancel)
}

fn mp4_sample(
    track: &TrackBuilder,
    timing: SampleTimingInfo,
    is_sync: bool,
    data_offset: u64,
    data_size: usize,
) -> Mp4Sample {
    Mp4Sample {
        track_kind: track.kind,
        sample_entry: (track.sample_count == 0).then(|| track.sample_entry.clone()),
        keyframe: track.kind != TrackKind::Video || is_sync,
        timescale: track.timescale,
        duration: timing.duration,
        composition_time_offset: (timing.composition_offset != 0 || track.kind == TrackKind::Video)
            .then_some(timing.composition_offset),
        data_offset,
        data_size,
        encryption_info: None,
    }
}

fn append_sample(
    muxer: &mut Mp4FileMuxer,
    track: &mut TrackBuilder,
    sample: &transmux::Sample,
    data_offset: u64,
) -> Result<(), RemuxError> {
    let mut timing = sample_timing(sample)?;
    if timing.duration == 0 {
        timing.duration = track
            .last_duration
            .ok_or_else(|| RemuxError::fatal("first sample has no positive duration"))?;
    }
    let is_sync = sample.flags.is_sync;
    let mp4_sample = mp4_sample(track, timing, is_sync, data_offset, sample.data.len());
    muxer
        .append_sample(&mp4_sample)
        .map_err(|e| RemuxError::fatal(format!("MP4 muxer rejected a sample: {e}")))?;
    track.record_sample(timing, sample.data.len())
}

struct ScanResult {
    plans: Vec<TrackPlan>,
    moov_size: usize,
}

fn scan_media(
    inputs: &[PathBuf],
    should_cancel: &dyn Fn() -> bool,
) -> Result<ScanResult, RemuxError> {
    let mut muxer = Mp4FileMuxer::new()
        .map_err(|e| RemuxError::fatal(format!("could not initialize MP4 muxer: {e}")))?;
    let mut position = muxer.initial_boxes_bytes().len() as u64;
    let mut tracks = TrackBuilders::default();
    stream_events(inputs, should_cancel, |event| {
        match event {
            DemuxEvent::TrackAdded(spec) | DemuxEvent::TrackUpdated(spec) => {
                tracks.add_or_update(spec)?;
            }
            DemuxEvent::Sample { track_id, sample, .. } => {
                if let Some(track) = tracks.tracks.get_mut(&track_id) {
                    append_sample(&mut muxer, track, &sample, position)?;
                    position += sample.data.len() as u64;
                }
            }
            _ => {}
        }
        Ok(())
    })?;
    let plans = planned_tracks(&tracks);
    if plans.is_empty() {
        return Err(RemuxError::fatal("MPEG-TS input contains no supported audio/video samples"));
    }
    let moov_size = muxer
        .finalize()
        .map_err(|e| RemuxError::fatal(format!("could not measure MP4 metadata: {e}")))?
        .moov_box_size();
    Ok(ScanResult { plans, moov_size })
}

fn validate_same_media(expected: &[TrackPlan], actual: &TrackBuilders) -> Result<(), RemuxError> {
    let actual = planned_tracks(actual);
    if actual.len() != expected.len() {
        return Err(RemuxError::fatal("input changed between MP4 mux passes"));
    }
    for expected in expected {
        let Some(actual) = actual.iter().find(|track| track.track_id == expected.track_id) else {
            return Err(RemuxError::fatal("input changed between MP4 mux passes"));
        };
        if actual.sample_count != expected.sample_count
            || actual.payload_bytes != expected.payload_bytes
            || actual.total_duration != expected.total_duration
            || actual.first_dts != expected.first_dts
            || actual.first_pts != expected.first_pts
        {
            return Err(RemuxError::fatal("input changed between MP4 mux passes"));
        }
    }
    Ok(())
}

/// Remux ordered MPEG-TS segment files into one faststart MP4.
pub(crate) fn remux_ts_files_to_mp4(
    inputs: &[PathBuf],
    output: &Path,
    should_cancel: &dyn Fn() -> bool,
) -> Result<(), RemuxError> {
    if inputs.is_empty() {
        return Err(RemuxError::fatal("no MPEG-TS segments to remux"));
    }
    let scan = scan_media(inputs, should_cancel)?;
    check_cancel(should_cancel)?;

    let reserved = scan.moov_size.saturating_add(EDIT_LIST_RESERVE_BYTES);
    let options = muxer_options(&scan.plans, reserved);
    let mut muxer = Mp4FileMuxer::with_options(options)
        .map_err(|e| RemuxError::fatal(format!("could not initialize MP4 muxer: {e}")))?;

    let cleanup = PartialOutput::new(output);
    let mut writer = std::fs::File::create(output)
        .map_err(|e| RemuxError::fatal(format!("could not create {}: {e}", output.display())))?;
    writer
        .write_all(muxer.initial_boxes_bytes())
        .map_err(|e| RemuxError::fatal(format!("could not write {}: {e}", output.display())))?;
    let mut position = muxer.initial_boxes_bytes().len() as u64;
    let mut tracks = TrackBuilders::default();
    stream_events(inputs, should_cancel, |event| {
        match event {
            DemuxEvent::TrackAdded(spec) | DemuxEvent::TrackUpdated(spec) => {
                tracks.add_or_update(spec)?;
            }
            DemuxEvent::Sample { track_id, sample, .. } => {
                if let Some(track) = tracks.tracks.get_mut(&track_id) {
                    writer.write_all(&sample.data).map_err(|e| {
                        RemuxError::fatal(format!("could not write media data: {e}"))
                    })?;
                    append_sample(&mut muxer, track, &sample, position)?;
                    position += sample.data.len() as u64;
                }
            }
            _ => {}
        }
        Ok(())
    })?;
    validate_same_media(&scan.plans, &tracks)?;
    check_cancel(should_cancel)?;

    let finalized = muxer
        .finalize()
        .map_err(|e| RemuxError::fatal(format!("could not finalize MP4 metadata: {e}")))?;
    if !finalized.is_faststart_enabled() {
        return Err(RemuxError::fatal("internal error: MP4 faststart reservation was too small"));
    }
    for (offset, bytes) in finalized.offset_and_bytes_pairs() {
        writer
            .seek(SeekFrom::Start(offset))
            .and_then(|_| writer.write_all(bytes))
            .map_err(|e| RemuxError::fatal(format!("could not write MP4 metadata: {e}")))?;
    }
    writer
        .flush()
        .map_err(|e| RemuxError::fatal(format!("could not write {}: {e}", output.display())))?;
    cleanup.keep();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shiguredo_mp4::boxes::{RootBox, SampleEntry, StszBox};
    use shiguredo_mp4::{Decode, Mp4File};

    fn fixture_parts() -> Vec<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls");
        (0..3).map(|i| root.join(format!("part{i}.ts"))).collect()
    }

    fn out(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("falcon-mux-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn remuxed_mp4_is_faststart_and_has_av_track_tables() {
        let output = out("structure.mp4");
        remux_ts_files_to_mp4(&fixture_parts(), &output, &|| false).unwrap();
        let bytes = std::fs::read(&output).unwrap();
        let (mp4, _) = Mp4File::<RootBox>::decode(&bytes).unwrap();
        let mut saw_moov = false;
        for b in &mp4.boxes {
            match b {
                RootBox::Moov(_) => saw_moov = true,
                RootBox::Mdat(_) => assert!(saw_moov, "moov must precede mdat"),
                _ => {}
            }
        }
        let moov = mp4
            .boxes
            .iter()
            .find_map(|b| match b {
                RootBox::Moov(moov) => Some(moov),
                _ => None,
            })
            .expect("moov");
        assert_eq!(moov.trak_boxes.len(), 2);
        let video = moov
            .trak_boxes
            .iter()
            .find(|t| t.mdia_box.hdlr_box.handler_type == *b"vide")
            .expect("video track");
        let audio = moov
            .trak_boxes
            .iter()
            .find(|t| t.mdia_box.hdlr_box.handler_type == *b"soun")
            .expect("audio track");
        assert_eq!(video.mdia_box.mdhd_box.timescale.get(), 90_000);
        assert_eq!(audio.mdia_box.mdhd_box.timescale.get(), 44_100);
        let video_stbl = &video.mdia_box.minf_box.stbl_box;
        assert!(video_stbl.ctts_box.as_ref().is_some_and(|c| !c.entries.is_empty()));
        assert!(video_stbl.stss_box.as_ref().is_some_and(|s| !s.sample_numbers.is_empty()));
        let edit =
            video.edts_box.as_ref().and_then(|e| e.elst_box.as_ref()).expect("video edit list");
        assert!(edit.entries.iter().any(|e| e.media_time > 0), "B-frame lead-in edit");
        assert!(matches!(video_stbl.stsd_box.entries.first(), Some(SampleEntry::Avc1(_))));
        assert!(matches!(
            audio.mdia_box.minf_box.stbl_box.stsd_box.entries.first(),
            Some(SampleEntry::Mp4a(_))
        ));
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn concat_ts_is_byte_exact() {
        let dir = std::env::temp_dir().join(format!("falcon-mux-concat-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.ts");
        let b = dir.join("b.ts");
        std::fs::write(&a, b"first\x47").unwrap();
        std::fs::write(&b, b"second\x47").unwrap();
        let output = dir.join("out.ts");
        concat_ts_files(&[a, b], &output, &|| false).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"first\x47second\x47");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_input_fails_without_partial_output() {
        let dir = std::env::temp_dir().join(format!("falcon-mux-bad-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("bad.ts");
        std::fs::write(&input, b"not an mpeg-ts stream").unwrap();
        let output = dir.join("out.mp4");
        assert!(remux_ts_files_to_mp4(&[input], &output, &|| false).is_err());
        assert!(!output.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn sample_sizes(mp4: &Mp4File<RootBox>, handler: &[u8; 4]) -> (Vec<u32>, Vec<(u32, i64)>) {
        let moov = mp4
            .boxes
            .iter()
            .find_map(|b| match b {
                RootBox::Moov(moov) => Some(moov),
                _ => None,
            })
            .expect("moov");
        let track = moov
            .trak_boxes
            .iter()
            .find(|track| track.mdia_box.hdlr_box.handler_type == *handler)
            .expect("track");
        let stbl = &track.mdia_box.minf_box.stbl_box;
        let sizes = match &stbl.stsz_box {
            StszBox::Fixed { sample_size, sample_count } => {
                vec![sample_size.get(); *sample_count as usize]
            }
            StszBox::Variable { entry_sizes } => entry_sizes.clone(),
        };
        let ctts = stbl
            .ctts_box
            .as_ref()
            .map(|ctts| {
                ctts.entries.iter().map(|entry| (entry.sample_count, entry.sample_offset)).collect()
            })
            .unwrap_or_default();
        (sizes, ctts)
    }

    /// ffmpeg remains the oracle where available: the remux must carry the same
    /// access-unit sizes and B-frame composition offsets, and ffmpeg must be
    /// able to decode the faststart result without a single error.
    #[test]
    fn ffmpeg_oracle_matches_bitstream_tables_and_decodes_output() {
        let Ok(ffmpeg) = crate::download::tools::find_ffmpeg() else {
            eprintln!("ffmpeg not found — skipping oracle comparison");
            return;
        };
        let dir = std::env::temp_dir().join(format!("falcon-mux-oracle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut concatenated = Vec::new();
        for part in fixture_parts() {
            concatenated.extend_from_slice(&std::fs::read(part).unwrap());
        }
        let input = dir.join("all.ts");
        let oracle = dir.join("oracle.mp4");
        let native = dir.join("native.mp4");
        std::fs::write(&input, concatenated).unwrap();
        let status = std::process::Command::new(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(&input)
            .args(["-map", "0", "-c", "copy", "-y"])
            .arg(&oracle)
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg oracle remux failed");
        remux_ts_files_to_mp4(&fixture_parts(), &native, &|| false).unwrap();

        let oracle_bytes = std::fs::read(&oracle).unwrap();
        let native_bytes = std::fs::read(&native).unwrap();
        let (oracle_mp4, _) = Mp4File::<RootBox>::decode(&oracle_bytes).unwrap();
        let (native_mp4, _) = Mp4File::<RootBox>::decode(&native_bytes).unwrap();
        assert_eq!(
            sample_sizes(&native_mp4, b"vide"),
            sample_sizes(&oracle_mp4, b"vide"),
            "video access units are bit-exact in count and size"
        );
        assert_eq!(
            sample_sizes(&native_mp4, b"soun").0,
            sample_sizes(&oracle_mp4, b"soun").0,
            "AAC access units are bit-exact in count and size"
        );
        assert_eq!(
            sample_sizes(&native_mp4, b"vide").1,
            sample_sizes(&oracle_mp4, b"vide").1,
            "B-frame composition offsets match ffmpeg"
        );
        let decoded = std::process::Command::new(&ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&native)
            .args(["-f", "null", "-"])
            .status()
            .unwrap();
        assert!(decoded.success(), "ffmpeg cannot decode the native MP4");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hevc_input_remuxes_to_an_hvc1_track() {
        let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls/h265.ts");
        let output = out("hevc.mp4");
        remux_ts_files_to_mp4(&[input], &output, &|| false).unwrap();
        let bytes = std::fs::read(&output).unwrap();
        let (mp4, _) = Mp4File::<RootBox>::decode(&bytes).unwrap();
        let moov = mp4
            .boxes
            .iter()
            .find_map(|b| match b {
                RootBox::Moov(moov) => Some(moov),
                _ => None,
            })
            .expect("moov");
        let video = moov
            .trak_boxes
            .iter()
            .find(|track| track.mdia_box.hdlr_box.handler_type == *b"vide")
            .expect("video track");
        assert!(matches!(
            video.mdia_box.minf_box.stbl_box.stsd_box.entries.first(),
            Some(SampleEntry::Hvc1(_))
        ));
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn timestamp_reset_seams_reuse_frame_duration() {
        let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls/part0.ts");
        let output = out("repeated.mp4");
        remux_ts_files_to_mp4(&[input.clone(), input], &output, &|| false).unwrap();
        let bytes = std::fs::read(&output).unwrap();
        let (mp4, _) = Mp4File::<RootBox>::decode(&bytes).unwrap();
        assert!(mp4.boxes.iter().any(|b| matches!(b, RootBox::Moov(_))));
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn multiple_audio_tracks_fail_instead_of_being_dropped() {
        let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls/multi-audio.ts");
        let output = out("multi-audio.mp4");
        let Err(RemuxError::Fatal(message)) = remux_ts_files_to_mp4(&[input], &output, &|| false)
        else {
            panic!("multiple audio tracks must be rejected");
        };
        assert!(message.contains("multiple audio"), "{message}");
        assert!(!output.exists());
    }

    #[test]
    fn cancellation_leaves_no_partial_output() {
        let output = out("cancel.mp4");
        let result = remux_ts_files_to_mp4(&fixture_parts(), &output, &|| true);
        assert_eq!(result, Err(RemuxError::Cancelled));
        assert!(!output.exists());
    }
}
