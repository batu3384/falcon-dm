//! Streaming MPEG-TS remuxing without external tools.
//!
//! The MP4 path is deliberately two-pass over the segment files: the first
//! pass measures the sample tables with zero media payloads in memory, the
//! second pass writes payload bytes directly to disk. The demuxer's event
//! queue is drained after every read in both passes, so memory scales with
//! the MP4 sample tables rather than with the media byte size.
//!
//! The writer boundary is byte-oriented rather than TS-specific. The MP4 merge
//! path uses the same two-pass discipline for fragmented and progressive MP4
//! inputs: sample payloads are copied straight between files in bounded chunks
//! and never accumulate in memory.

use shiguredo_mp4::boxes::{
    AudioSampleEntryFields, Avc1Box, AvccBox, ElstBox, ElstEntry, EsdsBox, Hvc1Box, HvccBox,
    HvccNalUintArray, MdatBox, MoofBox, MoovBox, Mp4aBox, SampleEntry, TrexBox, TrunSamples,
    VisualSampleEntryFields,
};
use shiguredo_mp4::demux::{Input as Mp4DemuxInput, Mp4FileDemuxer};
use shiguredo_mp4::descriptors::{
    DecoderConfigDescriptor, DecoderSpecificInfo, EsDescriptor, SlConfigDescriptor,
};
use shiguredo_mp4::mux::{
    EditList, Mp4FileMuxer, Mp4FileMuxerOptions, Sample as Mp4Sample, TrackMetadata,
};
use shiguredo_mp4::{BoxHeader, Decode, FixedPointNumber, TrackKind, Uint};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::io::{Read, Seek, SeekFrom, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use transmux::{CodecConfig, DemuxEvent, StreamingTsDemux, TrackId, TrackSpec};

const IO_CHUNK_BYTES: usize = 1024 * 1024;
/// The MPEG transport-stream timescale. Using it as the edit-list input unit
/// keeps A/V lead offsets exact; shiguredo converts them to the output movie
/// timescale it chooses.
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

/// Which side of an audio/video merge a codec rejection belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MergeTrackRole {
    Video,
    Audio,
}

impl MergeTrackRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }
}

/// A codec or container combination the progressive-MP4 merger refuses to
/// mislabel. This is deliberately separate from malformed-input failures so
/// the orchestration layer can select a different container or engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnsupportedMergeFormat {
    pub(crate) role: MergeTrackRole,
    pub(crate) codec: String,
    pub(crate) reason: &'static str,
}

/// Structured result of the split audio/video MP4 merge. I/O and bitstream
/// shape problems remain fatal strings, while codec-container policy is a
/// typed variant callers can match without parsing text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeError {
    Cancelled,
    Fatal(String),
    UnsupportedFormat(UnsupportedMergeFormat),
}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "merge cancelled"),
            Self::Fatal(message) => write!(f, "{message}"),
            Self::UnsupportedFormat(rejection) => write!(
                f,
                "unsupported {} codec {} for progressive MP4 merge: {}",
                rejection.role.as_str(),
                rejection.codec,
                rejection.reason
            ),
        }
    }
}

impl std::error::Error for MergeError {}

impl From<RemuxError> for MergeError {
    fn from(error: RemuxError) -> Self {
        match error {
            RemuxError::Cancelled => Self::Cancelled,
            RemuxError::Fatal(message) => Self::Fatal(message),
        }
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

/// `moov` and `moof` are metadata, so buffering one of them does not make the
/// merge proportional to media bytes. The cap is a malformed-input guard far
/// above legitimate DASH/YouTube metadata sizes.
const MAX_MP4_METADATA_BOX_BYTES: u64 = 64 * 1024 * 1024;

fn mp4_input_error(path: &Path, message: impl Into<String>) -> RemuxError {
    RemuxError::fatal(format!("{}: {}", path.display(), message.into()))
}

fn read_box_header_at(
    file: &mut std::fs::File,
    offset: u64,
    file_len: u64,
    path: &Path,
) -> Result<Option<(BoxHeader, usize)>, RemuxError> {
    if offset > file_len {
        return Err(mp4_input_error(path, "box offset lies beyond end of file"));
    }
    if offset == file_len {
        return Ok(None);
    }
    let readable = (file_len - offset).min(BoxHeader::MAX_SIZE as u64) as usize;
    let mut bytes = vec![0u8; readable];
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(&mut bytes))
        .map_err(|e| mp4_input_error(path, format!("could not read box header: {e}")))?;
    let (header, header_size) = BoxHeader::decode(&bytes)
        .map_err(|e| mp4_input_error(path, format!("invalid box header at byte {offset}: {e}")))?;
    Ok(Some((header, header_size)))
}

fn box_end(offset: u64, header: &BoxHeader, file_len: u64, path: &Path) -> Result<u64, RemuxError> {
    let size = header.box_size.get();
    if size == 0 {
        // 32-bit size=0 extends to EOF. size=1/largesize=0 is undefined by the
        // specification and shiguredo rejects it when decoding typed boxes.
        if header.external_size() != 8 {
            return Err(mp4_input_error(path, "box uses unsupported largesize=0"));
        }
        return Ok(file_len);
    }
    let end =
        offset.checked_add(size).ok_or_else(|| mp4_input_error(path, "box end offset overflow"))?;
    if end > file_len {
        return Err(mp4_input_error(path, "box extends beyond end of file"));
    }
    Ok(end)
}

fn read_metadata_box(
    file: &mut std::fs::File,
    offset: u64,
    end: u64,
    path: &Path,
    name: &str,
) -> Result<Vec<u8>, RemuxError> {
    let size = end
        .checked_sub(offset)
        .ok_or_else(|| mp4_input_error(path, format!("{name} box has invalid bounds")))?;
    if size > MAX_MP4_METADATA_BOX_BYTES {
        return Err(mp4_input_error(
            path,
            format!("{name} metadata box is too large ({size} bytes)"),
        ));
    }
    let size = usize::try_from(size)
        .map_err(|_| mp4_input_error(path, format!("{name} metadata box exceeds usize::MAX")))?;
    let mut bytes = vec![0u8; size];
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(&mut bytes))
        .map_err(|e| mp4_input_error(path, format!("could not read {name} box: {e}")))?;
    Ok(bytes)
}

#[derive(Clone)]
struct Mp4Probe {
    fragmented: bool,
    moov_end: u64,
    track_id: u32,
    kind: TrackKind,
    timescale: NonZeroU32,
    sample_entry: SampleEntry,
    trex: Option<TrexBox>,
}

/// Read only the metadata needed to classify an input. The scan is streaming
/// except for the bounded `moov` box itself; in particular it does not read
/// `mdat` bytes even when a non-faststart input keeps `moov` at EOF.
fn probe_mp4(path: &Path) -> Result<Mp4Probe, RemuxError> {
    let mut file = open_input(path)?;
    let file_len = file
        .metadata()
        .map_err(|e| mp4_input_error(path, format!("could not stat input: {e}")))?
        .len();
    let mut offset = 0u64;
    let mut first_box = true;
    while let Some((header, _)) = read_box_header_at(&mut file, offset, file_len, path)? {
        let end = box_end(offset, &header, file_len, path)?;
        if first_box && header.box_type.as_bytes() != b"ftyp" {
            return Err(mp4_input_error(path, "first box is not ftyp; not an MP4 input"));
        }
        first_box = false;
        if header.box_type == MoovBox::TYPE {
            let bytes = read_metadata_box(&mut file, offset, end, path, "moov")?;
            let (moov, _) = MoovBox::decode(&bytes)
                .map_err(|e| mp4_input_error(path, format!("could not decode moov box: {e}")))?;
            if !moov.pssh_boxes.is_empty() {
                return Err(mp4_input_error(
                    path,
                    "DRM initialization data is present; encrypted MP4 merge is unsupported",
                ));
            }
            if moov.trak_boxes.len() != 1 {
                return Err(mp4_input_error(
                    path,
                    format!(
                        "input has {} tracks; split audio/video merge requires exactly one",
                        moov.trak_boxes.len()
                    ),
                ));
            }
            let trak = &moov.trak_boxes[0];
            let kind = match trak.mdia_box.hdlr_box.handler_type {
                h if h == *b"vide" => TrackKind::Video,
                h if h == *b"soun" => TrackKind::Audio,
                other => {
                    return Err(mp4_input_error(
                        path,
                        format!(
                            "unsupported track handler {}; split merge accepts only video/audio",
                            String::from_utf8_lossy(&other)
                        ),
                    ));
                }
            };
            let entries = &trak.mdia_box.minf_box.stbl_box.stsd_box.entries;
            if entries.len() != 1 {
                return Err(mp4_input_error(
                    path,
                    format!(
                        "input uses {} sample descriptions; codec configuration changes are unsupported",
                        entries.len()
                    ),
                ));
            }
            let track_id = trak.tkhd_box.track_id;
            let fragmented = moov.mvex_box.is_some();
            let trex = if fragmented {
                let mvex = moov.mvex_box.as_ref().expect("fragmented checked");
                Some(
                    mvex.trex_boxes
                        .iter()
                        .find(|trex| trex.track_id == track_id)
                        .cloned()
                        .ok_or_else(|| {
                            mp4_input_error(path, "fragmented input has no trex defaults")
                        })?,
                )
            } else {
                None
            };
            return Ok(Mp4Probe {
                fragmented,
                moov_end: end,
                track_id,
                kind,
                timescale: trak.mdia_box.mdhd_box.timescale,
                sample_entry: repaired_sample_entry(&entries[0]),
                trex,
            });
        }
        if header.box_size.get() == 0 {
            return Err(mp4_input_error(path, "moov box not found before EOF-sized box"));
        }
        offset = end;
    }
    Err(mp4_input_error(path, "moov box not found"))
}

#[derive(Debug, Clone)]
struct InputSampleMeta {
    dts: i64,
    duration: u32,
    composition_offset: i64,
    keyframe: bool,
    data_offset: u64,
    data_size: usize,
}

struct ProgressiveMp4Input {
    file: std::fs::File,
    demuxer: Mp4FileDemuxer,
    timescale: NonZeroU32,
    sample_entry: SampleEntry,
    path: PathBuf,
}

impl ProgressiveMp4Input {
    fn new(
        path: &Path,
        probe: &Mp4Probe,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<Self, RemuxError> {
        let mut file = open_input(path)?;
        let file_len = file
            .metadata()
            .map_err(|e| mp4_input_error(path, format!("could not stat input: {e}")))?
            .len();
        let mut demuxer = Mp4FileDemuxer::new();
        while let Some(required) = demuxer.required_input() {
            check_cancel(should_cancel)?;
            if required.position > file_len {
                return Err(mp4_input_error(path, "progressive MP4 metadata lies beyond EOF"));
            }
            let available = file_len - required.position;
            let size = match required.size {
                Some(size) => size.min(usize::try_from(available).unwrap_or(usize::MAX)),
                None => usize::try_from(available).map_err(|_| {
                    mp4_input_error(path, "progressive MP4 metadata exceeds usize::MAX")
                })?,
            };
            if size as u64 > MAX_MP4_METADATA_BOX_BYTES {
                return Err(mp4_input_error(
                    path,
                    format!("progressive MP4 metadata is too large ({size} bytes)"),
                ));
            }
            let mut data = vec![0u8; size];
            file.seek(SeekFrom::Start(required.position))
                .and_then(|_| file.read_exact(&mut data))
                .map_err(|e| mp4_input_error(path, format!("could not read MP4 metadata: {e}")))?;
            demuxer.handle_input(Mp4DemuxInput { position: required.position, data: &data });
            if let Some(error) = demuxer.last_error() {
                return Err(mp4_input_error(
                    path,
                    format!("progressive MP4 metadata parse failed: {error}"),
                ));
            }
        }
        let tracks = demuxer
            .tracks()
            .map_err(|e| mp4_input_error(path, format!("progressive MP4 parse failed: {e}")))?;
        if tracks.len() != 1 || tracks[0].track_id != probe.track_id {
            return Err(mp4_input_error(path, "progressive MP4 track table changed while probing"));
        }
        Ok(Self {
            file,
            demuxer,
            timescale: probe.timescale,
            sample_entry: probe.sample_entry.clone(),
            path: path.to_path_buf(),
        })
    }

    fn next_sample(&mut self) -> Result<Option<InputSampleMeta>, RemuxError> {
        let sample = match self.demuxer.next_sample() {
            Ok(Some(sample)) => sample,
            Ok(None) => return Ok(None),
            Err(error) => {
                return Err(mp4_input_error(
                    &self.path,
                    format!("progressive MP4 sample table parse failed: {error}"),
                ));
            }
        };
        if let Some(entry) = sample.sample_entry {
            if repaired_sample_entry(entry) != self.sample_entry {
                return Err(mp4_input_error(
                    &self.path,
                    "codec configuration changes mid-stream are unsupported",
                ));
            }
        }
        if sample.encryption_info.is_some() {
            return Err(mp4_input_error(&self.path, "encrypted samples are unsupported"));
        }
        let dts = i64::try_from(sample.timestamp)
            .map_err(|_| mp4_input_error(&self.path, "sample timestamp exceeds i64::MAX"))?;
        Ok(Some(InputSampleMeta {
            dts,
            duration: sample.duration,
            composition_offset: sample.composition_time_offset.unwrap_or(0),
            keyframe: sample.keyframe,
            data_offset: sample.data_offset,
            data_size: sample.data_size,
        }))
    }
}

struct FragmentedMp4Input {
    file: std::fs::File,
    file_len: u64,
    offset: u64,
    pending_moofs: Vec<(u64, MoofBox)>,
    pending_samples: VecDeque<InputSampleMeta>,
    next_fragment_dts: i64,
    finished: bool,
    probe: Mp4Probe,
    path: PathBuf,
}

impl FragmentedMp4Input {
    fn new(path: &Path, probe: Mp4Probe) -> Result<Self, RemuxError> {
        let file = open_input(path)?;
        let file_len = file
            .metadata()
            .map_err(|e| mp4_input_error(path, format!("could not stat input: {e}")))?
            .len();
        Ok(Self {
            file,
            file_len,
            offset: probe.moov_end,
            pending_moofs: Vec::new(),
            pending_samples: VecDeque::new(),
            next_fragment_dts: 0,
            finished: false,
            probe,
            path: path.to_path_buf(),
        })
    }

    fn parse_pending_moofs(
        &self,
        moofs: &[(u64, MoofBox)],
        mdat_payload_start: u64,
        mdat_end: u64,
    ) -> Result<(Vec<InputSampleMeta>, i64), RemuxError> {
        let trex =
            self.probe.trex.as_ref().ok_or_else(|| {
                mp4_input_error(&self.path, "fragmented input has no trex defaults")
            })?;
        let mut samples = Vec::new();
        let mut running_decode_time = self.next_fragment_dts;
        for (moof_offset, moof) in moofs {
            let mut prev_traf_data_end: Option<u64> = None;
            for traf in &moof.traf_boxes {
                let tfhd = &traf.tfhd_box;
                if tfhd.track_id != self.probe.track_id {
                    return Err(mp4_input_error(
                        &self.path,
                        format!(
                            "fragment carries unknown track_id {}; dropping it is not allowed",
                            tfhd.track_id
                        ),
                    ));
                }
                if traf.unknown_boxes.iter().any(|b| {
                    let ty = b.box_type.as_bytes();
                    ty == b"senc" || ty == b"saio" || ty == b"saiz"
                }) {
                    return Err(mp4_input_error(
                        &self.path,
                        "fragment carries sample encryption metadata; encrypted MP4 merge is unsupported",
                    ));
                }
                let sample_description_index =
                    tfhd.sample_description_index.unwrap_or(trex.default_sample_description_index);
                if sample_description_index != 1 {
                    return Err(mp4_input_error(
                        &self.path,
                        "fragment changes sample description; codec configuration changes are unsupported",
                    ));
                }
                if let Some(tfdt) = &traf.tfdt_box {
                    running_decode_time =
                        i64::try_from(tfdt.base_media_decode_time).map_err(|_| {
                            mp4_input_error(&self.path, "tfdt decode time exceeds i64::MAX")
                        })?;
                }
                let base_data_offset = if let Some(explicit) = tfhd.base_data_offset {
                    explicit
                } else if tfhd.default_base_is_moof {
                    *moof_offset
                } else {
                    prev_traf_data_end.unwrap_or(*moof_offset)
                };
                let mut prev_trun_data_end: Option<u64> = None;
                let mut traf_data_end = base_data_offset;
                for trun in &traf.trun_boxes {
                    let trun_start = match trun.data_offset {
                        Some(delta) => {
                            let position = (base_data_offset as i128) + i128::from(delta);
                            u64::try_from(position).map_err(|_| {
                                mp4_input_error(&self.path, "trun data offset is negative")
                            })?
                        }
                        None => prev_trun_data_end.unwrap_or(base_data_offset),
                    };
                    let mut sample_data_offset = trun_start;
                    let sample_count = match &trun.samples {
                        TrunSamples::CountOnly(count) => *count as usize,
                        TrunSamples::PerSample(rows) => rows.len(),
                    };
                    for row_index in 0..sample_count {
                        let row = match &trun.samples {
                            TrunSamples::CountOnly(_) => None,
                            TrunSamples::PerSample(rows) => Some(&rows[row_index]),
                        };
                        let duration = row
                            .and_then(|sample| sample.duration)
                            .or(tfhd.default_sample_duration)
                            .unwrap_or(trex.default_sample_duration);
                        if duration == 0 {
                            return Err(mp4_input_error(
                                &self.path,
                                "fragment sample has no positive duration",
                            ));
                        }
                        let size = row
                            .and_then(|sample| sample.size)
                            .or(tfhd.default_sample_size)
                            .unwrap_or(trex.default_sample_size);
                        // A zero trex size means "not declared" in real muxer output.
                        // Treating it as a zero-byte sample would silently lose media.
                        if size == 0 {
                            return Err(mp4_input_error(
                                &self.path,
                                "fragment sample has no declared byte size",
                            ));
                        }
                        let flags = match (row_index, trun.first_sample_flags) {
                            (0, Some(flags)) => flags,
                            _ => row
                                .and_then(|sample| sample.flags)
                                .or(tfhd.default_sample_flags)
                                .unwrap_or(trex.default_sample_flags),
                        };
                        let sample_data_end =
                            sample_data_offset.checked_add(u64::from(size)).ok_or_else(|| {
                                mp4_input_error(&self.path, "sample data offset overflow")
                            })?;
                        if sample_data_offset < mdat_payload_start || sample_data_end > mdat_end {
                            return Err(mp4_input_error(
                                &self.path,
                                "fragment sample bytes lie outside the following mdat; out-of-fragment references are unsupported",
                            ));
                        }
                        samples.push(InputSampleMeta {
                            dts: running_decode_time,
                            duration,
                            composition_offset: row
                                .and_then(|sample| sample.composition_time_offset)
                                .unwrap_or(0),
                            keyframe: !flags.sample_is_non_sync_sample(),
                            data_offset: sample_data_offset,
                            data_size: size as usize,
                        });
                        running_decode_time =
                            running_decode_time.checked_add(i64::from(duration)).ok_or_else(
                                || mp4_input_error(&self.path, "fragment decode time overflow"),
                            )?;
                        sample_data_offset = sample_data_end;
                    }
                    prev_trun_data_end = Some(sample_data_offset);
                    traf_data_end = traf_data_end.max(sample_data_offset);
                }
                prev_traf_data_end = Some(traf_data_end);
            }
        }
        Ok((samples, running_decode_time))
    }

    fn next_sample(&mut self) -> Result<Option<InputSampleMeta>, RemuxError> {
        loop {
            if let Some(sample) = self.pending_samples.pop_front() {
                return Ok(Some(sample));
            }
            if self.finished {
                return Ok(None);
            }
            let Some((header, header_size)) =
                read_box_header_at(&mut self.file, self.offset, self.file_len, &self.path)?
            else {
                if self.pending_moofs.is_empty() {
                    self.finished = true;
                    return Ok(None);
                }
                return Err(mp4_input_error(&self.path, "moof at EOF has no following mdat"));
            };
            let box_offset = self.offset;
            let end = box_end(box_offset, &header, self.file_len, &self.path)?;
            if header.box_type == MoofBox::TYPE {
                let bytes = read_metadata_box(&mut self.file, box_offset, end, &self.path, "moof")?;
                let (moof, _) = MoofBox::decode(&bytes).map_err(|e| {
                    mp4_input_error(&self.path, format!("could not decode moof box: {e}"))
                })?;
                self.pending_moofs.push((box_offset, moof));
            } else if header.box_type == MdatBox::TYPE && !self.pending_moofs.is_empty() {
                let payload_start = box_offset
                    .checked_add(header_size as u64)
                    .ok_or_else(|| mp4_input_error(&self.path, "mdat payload offset overflow"))?;
                let (samples, next_decode_time) =
                    self.parse_pending_moofs(&self.pending_moofs, payload_start, end)?;
                self.pending_moofs.clear();
                self.pending_samples.extend(samples);
                self.next_fragment_dts = next_decode_time;
            }
            self.offset = end;
        }
    }
}

enum Mp4InputReader {
    Progressive(ProgressiveMp4Input),
    Fragmented(FragmentedMp4Input),
}

impl Mp4InputReader {
    fn open(
        path: &Path,
        expected_kind: TrackKind,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<Self, RemuxError> {
        let probe = probe_mp4(path)?;
        if probe.kind != expected_kind {
            return Err(mp4_input_error(
                path,
                format!("expected a {expected_kind:?} track but found {:?}", probe.kind),
            ));
        }
        if probe.fragmented {
            FragmentedMp4Input::new(path, probe).map(Self::Fragmented)
        } else {
            ProgressiveMp4Input::new(path, &probe, should_cancel).map(Self::Progressive)
        }
    }

    fn timescale(&self) -> NonZeroU32 {
        match self {
            Self::Progressive(input) => input.timescale,
            Self::Fragmented(input) => input.probe.timescale,
        }
    }

    fn sample_entry(&self) -> &SampleEntry {
        match self {
            Self::Progressive(input) => &input.sample_entry,
            Self::Fragmented(input) => &input.probe.sample_entry,
        }
    }

    fn next_sample(&mut self) -> Result<Option<InputSampleMeta>, RemuxError> {
        match self {
            Self::Progressive(input) => input.next_sample(),
            Self::Fragmented(input) => input.next_sample(),
        }
    }

    fn copy_sample_payload(
        &mut self,
        sample: &InputSampleMeta,
        writer: &mut dyn Write,
        buffer: &mut [u8],
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<(), RemuxError> {
        let (file, path) = match self {
            Self::Progressive(input) => (&mut input.file, &input.path),
            Self::Fragmented(input) => (&mut input.file, &input.path),
        };
        file.seek(SeekFrom::Start(sample.data_offset))
            .map_err(|e| mp4_input_error(path, format!("could not seek to sample payload: {e}")))?;
        let mut remaining = sample.data_size;
        while remaining > 0 {
            check_cancel(should_cancel)?;
            let amount = remaining.min(buffer.len());
            file.read_exact(&mut buffer[..amount]).map_err(|e| {
                mp4_input_error(path, format!("could not read sample payload: {e}"))
            })?;
            writer
                .write_all(&buffer[..amount])
                .map_err(|e| RemuxError::fatal(format!("could not write media data: {e}")))?;
            remaining -= amount;
        }
        Ok(())
    }
}

fn sample_entry_codec_label(entry: &SampleEntry) -> String {
    match entry {
        SampleEntry::Avc1(_) => "h264/avc1".to_string(),
        SampleEntry::Hev1(_) => "h265/hev1".to_string(),
        SampleEntry::Hvc1(_) => "h265/hvc1".to_string(),
        SampleEntry::Vp08(_) => "vp8/vp08".to_string(),
        SampleEntry::Vp09(_) => "vp9/vp09".to_string(),
        SampleEntry::Av01(_) => "av1/av01".to_string(),
        SampleEntry::Opus(_) => "opus/Opus".to_string(),
        SampleEntry::Mp4a(_) => "mp4a".to_string(),
        SampleEntry::Flac(_) => "flac/fLaC".to_string(),
        SampleEntry::Stpp(_) => "ttml/stpp".to_string(),
        SampleEntry::Wvtt(_) => "webvtt/wvtt".to_string(),
        SampleEntry::Tx3g(_) => "timed-text/tx3g".to_string(),
        SampleEntry::Encv(_) => "encrypted-video/encv".to_string(),
        SampleEntry::Enca(_) => "encrypted-audio/enca".to_string(),
        SampleEntry::Unknown(b) => format!(
            "unknown/{}",
            String::from_utf8_lossy(b.box_type.as_bytes()).trim_end_matches('\0')
        ),
    }
}

fn is_aac_mp4a(entry: &SampleEntry) -> bool {
    let SampleEntry::Mp4a(mp4a) = entry else {
        return false;
    };
    let config = &mp4a.esds_box.es.dec_config_descr;
    config.object_type_indication
        == DecoderConfigDescriptor::OBJECT_TYPE_INDICATION_AUDIO_ISO_IEC_14496_3
        && config.dec_specific_info.is_some()
}

fn unsupported_merge_format(
    role: MergeTrackRole,
    entry: &SampleEntry,
    reason: &'static str,
) -> MergeError {
    MergeError::UnsupportedFormat(UnsupportedMergeFormat {
        role,
        codec: sample_entry_codec_label(entry),
        reason,
    })
}

/// MP4 output policy for the native split-file merger:
///
/// - video: `avc1`, `hev1`/`hvc1`, `av01`, and `vp09` are accepted and their
///   sample entries (including codec configuration child boxes) are copied
///   verbatim;
/// - audio: only MPEG-4 AAC in `mp4a` is accepted;
/// - Opus, VP8, FLAC, encrypted tracks, subtitle/text entries and unknown
///   entries are rejected as typed errors instead of producing a mislabeled or
///   practically unplayable MP4.
///
/// VP9-in-MP4 is standardized but QuickTime support is uneven. It is still a
/// legal MP4 combination with AAC, so the muxer preserves it and leaves the
/// playback-support tradeoff to format selection. Opus-in-MP4 is rejected here:
/// it is not QuickTime-playable, and WebM/Matroska is the correct fallback
/// container for the orchestration layer.
fn validate_merge_format(video: &SampleEntry, audio: &SampleEntry) -> Result<(), MergeError> {
    match video {
        SampleEntry::Avc1(_)
        | SampleEntry::Hev1(_)
        | SampleEntry::Hvc1(_)
        | SampleEntry::Av01(_)
        | SampleEntry::Vp09(_) => {}
        other => {
            return Err(unsupported_merge_format(
                MergeTrackRole::Video,
                other,
                "only h264, h265, AV1 and VP9 video can be carried by this MP4 merger",
            ));
        }
    }
    if !is_aac_mp4a(audio) {
        return Err(unsupported_merge_format(
            MergeTrackRole::Audio,
            audio,
            "only AAC audio is MP4-safe here; Opus needs a WebM/Matroska fallback",
        ));
    }
    Ok(())
}

struct MergeTrackStats {
    kind: TrackKind,
    timescale: NonZeroU32,
    sample_entry: SampleEntry,
    sample_count: u64,
    payload_bytes: u64,
    total_duration: u64,
    first_dts: Option<i64>,
    first_pts: Option<i64>,
}

impl MergeTrackStats {
    fn new(kind: TrackKind, timescale: NonZeroU32, sample_entry: SampleEntry) -> Self {
        Self {
            kind,
            timescale,
            sample_entry,
            sample_count: 0,
            payload_bytes: 0,
            total_duration: 0,
            first_dts: None,
            first_pts: None,
        }
    }

    fn record(&mut self, sample: &InputSampleMeta) -> Result<SampleTimingInfo, RemuxError> {
        if sample.duration == 0 {
            return Err(RemuxError::fatal(format!(
                "{:?} input sample has zero duration",
                self.kind
            )));
        }
        let raw_pts = sample
            .dts
            .checked_add(sample.composition_offset)
            .ok_or_else(|| RemuxError::fatal("input presentation timestamp overflow"))?;
        let first_dts = self.first_dts.unwrap_or(sample.dts);
        let rebased_dts = sample
            .dts
            .checked_sub(first_dts)
            .ok_or_else(|| RemuxError::fatal("input decode timestamp overflow"))?;
        let rebased_pts = raw_pts
            .checked_sub(first_dts)
            .ok_or_else(|| RemuxError::fatal("input presentation timestamp overflow"))?;
        if rebased_dts < 0 || rebased_dts as u64 != self.total_duration {
            return Err(RemuxError::fatal(format!(
                "{:?} input has a discontinuous decode timeline (expected DTS {}, got {})",
                self.kind, self.total_duration, rebased_dts
            )));
        }
        if self.sample_count == 0 {
            self.first_dts = Some(first_dts);
            self.first_pts = Some(rebased_pts);
        }
        self.sample_count += 1;
        self.payload_bytes += sample.data_size as u64;
        self.total_duration += u64::from(sample.duration);
        Ok(SampleTimingInfo {
            dts: rebased_dts,
            pts: rebased_pts,
            duration: sample.duration,
            composition_offset: sample.composition_offset,
        })
    }

    fn plan(&self, track_id: TrackId) -> Option<TrackPlan> {
        if self.sample_count == 0 {
            return None;
        }
        Some(TrackPlan {
            track_id,
            kind: self.kind,
            timescale: self.timescale,
            sample_count: self.sample_count,
            payload_bytes: self.payload_bytes,
            total_duration: self.total_duration,
            // Every output track is rebased to decode time zero. The first
            // presentation timestamp remains in track units and can be
            // negative (AAC priming) or positive (B-frame reordering).
            first_dts: 0,
            first_pts: self.first_pts.expect("sampled track"),
        })
    }
}

struct MergedInputSample {
    kind: TrackKind,
    timing: SampleTimingInfo,
    keyframe: bool,
    data_offset: u64,
    data_size: usize,
    first_in_track: bool,
}

struct MergeInputCursor {
    reader: Mp4InputReader,
    stats: MergeTrackStats,
    next: Option<InputSampleMeta>,
}

impl MergeInputCursor {
    fn open(
        path: &Path,
        kind: TrackKind,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<Self, RemuxError> {
        let mut reader = Mp4InputReader::open(path, kind, should_cancel)?;
        let stats = MergeTrackStats::new(kind, reader.timescale(), reader.sample_entry().clone());
        let next = reader.next_sample()?;
        Ok(Self { reader, stats, next })
    }

    fn peek_rebased_dts(&self) -> Option<(i64, u32)> {
        self.next.as_ref().map(|sample| {
            let base = self.stats.first_dts.unwrap_or(sample.dts);
            (sample.dts.saturating_sub(base), self.stats.timescale.get())
        })
    }

    fn pop(&mut self, should_cancel: &dyn Fn() -> bool) -> Result<MergedInputSample, RemuxError> {
        check_cancel(should_cancel)?;
        let sample = self
            .next
            .take()
            .ok_or_else(|| RemuxError::fatal("internal error: empty merge cursor"))?;
        let first_in_track = self.stats.sample_count == 0;
        let timing = self.stats.record(&sample)?;
        self.next = self.reader.next_sample()?;
        Ok(MergedInputSample {
            kind: self.stats.kind,
            timing,
            keyframe: sample.keyframe,
            data_offset: sample.data_offset,
            data_size: sample.data_size,
            first_in_track,
        })
    }

    fn copy_payload(
        &mut self,
        sample: &MergedInputSample,
        writer: &mut dyn Write,
        buffer: &mut [u8],
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<(), RemuxError> {
        let meta = InputSampleMeta {
            dts: 0,
            duration: sample.timing.duration,
            composition_offset: sample.timing.composition_offset,
            keyframe: sample.keyframe,
            data_offset: sample.data_offset,
            data_size: sample.data_size,
        };
        self.reader.copy_sample_payload(&meta, writer, buffer, should_cancel)
    }
}

struct MergeCursor {
    video: MergeInputCursor,
    audio: MergeInputCursor,
}

impl MergeCursor {
    fn open(
        video_path: &Path,
        audio_path: &Path,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<Self, MergeError> {
        let video = MergeInputCursor::open(video_path, TrackKind::Video, should_cancel)?;
        let audio = MergeInputCursor::open(audio_path, TrackKind::Audio, should_cancel)?;
        validate_merge_format(video.reader.sample_entry(), audio.reader.sample_entry())?;
        if video.next.is_none() {
            return Err(MergeError::Fatal("video input contains no samples".to_string()));
        }
        if audio.next.is_none() {
            return Err(MergeError::Fatal("audio input contains no samples".to_string()));
        }
        Ok(Self { video, audio })
    }

    fn next_sample(
        &mut self,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<Option<MergedInputSample>, MergeError> {
        check_cancel(should_cancel)?;
        let ordering = match (self.video.peek_rebased_dts(), self.audio.peek_rebased_dts()) {
            (Some((v_ts, v_scale)), Some((a_ts, a_scale))) => {
                cmp_ratio(v_ts, v_scale, a_ts, a_scale)
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => return Ok(None),
        };
        // Equal decode times put the video sample first deterministically. The
        // choice only affects chunk boundaries, not presentation timing.
        if ordering.is_gt() {
            self.audio.pop(should_cancel).map(Some).map_err(MergeError::from)
        } else {
            self.video.pop(should_cancel).map(Some).map_err(MergeError::from)
        }
    }

    fn plans(&self) -> Vec<TrackPlan> {
        [self.video.stats.plan(TrackId::new(1)), self.audio.stats.plan(TrackId::new(2))]
            .into_iter()
            .flatten()
            .collect()
    }
}

fn merged_mp4_sample(
    cursor: &MergeInputCursor,
    sample: &MergedInputSample,
    data_offset: u64,
) -> Mp4Sample {
    Mp4Sample {
        track_kind: sample.kind,
        sample_entry: sample.first_in_track.then(|| cursor.stats.sample_entry.clone()),
        keyframe: sample.kind != TrackKind::Video || sample.keyframe,
        timescale: cursor.stats.timescale,
        duration: sample.timing.duration,
        composition_time_offset: (sample.timing.composition_offset != 0
            || sample.kind == TrackKind::Video)
            .then_some(sample.timing.composition_offset),
        data_offset,
        data_size: sample.data_size,
        encryption_info: None,
    }
}

fn append_merged_sample(
    muxer: &mut Mp4FileMuxer,
    cursor: &MergeInputCursor,
    sample: &MergedInputSample,
    data_offset: u64,
) -> Result<(), MergeError> {
    let mp4_sample = merged_mp4_sample(cursor, sample, data_offset);
    muxer
        .append_sample(&mp4_sample)
        .map_err(|e| MergeError::Fatal(format!("MP4 muxer rejected a sample: {e}")))
}

struct MergeScanResult {
    plans: Vec<TrackPlan>,
    moov_size: usize,
}

fn scan_mp4_merge(
    video_path: &Path,
    audio_path: &Path,
    should_cancel: &dyn Fn() -> bool,
) -> Result<MergeScanResult, MergeError> {
    let mut muxer = Mp4FileMuxer::new()
        .map_err(|e| MergeError::Fatal(format!("could not initialize MP4 muxer: {e}")))?;
    let mut position = muxer.initial_boxes_bytes().len() as u64;
    let mut cursor = MergeCursor::open(video_path, audio_path, should_cancel)?;
    while let Some(sample) = cursor.next_sample(should_cancel)? {
        let input = if sample.kind == TrackKind::Video { &cursor.video } else { &cursor.audio };
        append_merged_sample(&mut muxer, input, &sample, position)?;
        position += sample.data_size as u64;
    }
    let plans = cursor.plans();
    if plans.len() != 2 {
        return Err(MergeError::Fatal("split inputs did not produce both audio and video".into()));
    }
    let moov_size = muxer
        .finalize()
        .map_err(|e| MergeError::Fatal(format!("could not measure MP4 metadata: {e}")))?
        .moov_box_size();
    Ok(MergeScanResult { plans, moov_size })
}

fn validate_same_merge_media(
    expected: &[TrackPlan],
    actual: &[TrackPlan],
) -> Result<(), MergeError> {
    if actual.len() != expected.len() {
        return Err(MergeError::Fatal("input changed between MP4 mux passes".into()));
    }
    for expected in expected {
        let Some(actual) = actual.iter().find(|track| track.kind == expected.kind) else {
            return Err(MergeError::Fatal("input changed between MP4 mux passes".into()));
        };
        if actual.sample_count != expected.sample_count
            || actual.payload_bytes != expected.payload_bytes
            || actual.total_duration != expected.total_duration
            || actual.first_dts != expected.first_dts
            || actual.first_pts != expected.first_pts
            || actual.timescale != expected.timescale
        {
            return Err(MergeError::Fatal("input changed between MP4 mux passes".into()));
        }
    }
    Ok(())
}

/// Merge one video-only and one audio-only MP4/fMP4 input into a progressive
/// faststart MP4. The function is synchronous and `spawn_blocking`-friendly,
/// follows the existing cancellation closure convention, and removes partial
/// output on every error or cancellation path.
///
/// Inputs may be ordinary progressive MP4 or byte-concatenated fragmented MP4.
/// Every input must contain exactly one track of its declared role. Samples
/// are copied byte-exactly in bounded chunks; no sample payload is buffered as
/// a whole, so RSS does not scale with media file size.
pub(crate) fn merge_av_mp4(
    video_path: &Path,
    audio_path: &Path,
    output: &Path,
    should_cancel: &dyn Fn() -> bool,
) -> Result<(), MergeError> {
    let scan = scan_mp4_merge(video_path, audio_path, should_cancel)?;
    check_cancel(should_cancel)?;

    let reserved = scan.moov_size.saturating_add(EDIT_LIST_RESERVE_BYTES);
    let options = muxer_options(&scan.plans, reserved);
    let mut muxer = Mp4FileMuxer::with_options(options)
        .map_err(|e| MergeError::Fatal(format!("could not initialize MP4 muxer: {e}")))?;

    let cleanup = PartialOutput::new(output);
    let mut writer = std::fs::File::create(output)
        .map_err(|e| MergeError::Fatal(format!("could not create {}: {e}", output.display())))?;
    writer
        .write_all(muxer.initial_boxes_bytes())
        .map_err(|e| MergeError::Fatal(format!("could not write {}: {e}", output.display())))?;
    let mut position = muxer.initial_boxes_bytes().len() as u64;
    let mut cursor = MergeCursor::open(video_path, audio_path, should_cancel)?;
    let mut copy_buffer = vec![0u8; IO_CHUNK_BYTES];
    while let Some(sample) = cursor.next_sample(should_cancel)? {
        let input =
            if sample.kind == TrackKind::Video { &mut cursor.video } else { &mut cursor.audio };
        input.copy_payload(&sample, &mut writer, &mut copy_buffer, should_cancel)?;
        append_merged_sample(&mut muxer, input, &sample, position)?;
        position += sample.data_size as u64;
    }
    let actual_plans = cursor.plans();
    validate_same_merge_media(&scan.plans, &actual_plans)?;
    check_cancel(should_cancel)?;

    let finalized = muxer
        .finalize()
        .map_err(|e| MergeError::Fatal(format!("could not finalize MP4 metadata: {e}")))?;
    if !finalized.is_faststart_enabled() {
        return Err(MergeError::Fatal(
            "internal error: MP4 faststart reservation was too small".into(),
        ));
    }
    for (offset, bytes) in finalized.offset_and_bytes_pairs() {
        writer
            .seek(SeekFrom::Start(offset))
            .and_then(|_| writer.write_all(bytes))
            .map_err(|e| MergeError::Fatal(format!("could not write MP4 metadata: {e}")))?;
    }
    writer
        .flush()
        .map_err(|e| MergeError::Fatal(format!("could not write {}: {e}", output.display())))?;
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

/// Normalize an avcC entry copied from an input file: real-world files in the
/// wild (YouTube included) declare a high-family profile yet omit the avcC
/// tail the spec makes mandatory for it. The tail duplicates what the SPS
/// already declares, and a high-profile stream without override bits is
/// 4:2:0 8-bit, so repair the record into a spec-valid one instead of failing
/// the whole merge when the muxer re-encodes it.
fn repaired_sample_entry(entry: &SampleEntry) -> SampleEntry {
    let mut entry = entry.clone();
    if let SampleEntry::Avc1(avc1) = &mut entry {
        let high = !matches!(avc1.avcc_box.avc_profile_indication, 66 | 77 | 88);
        if high && avc1.avcc_box.chroma_format.is_none() {
            avc1.avcc_box.chroma_format = Some(Uint::new(1));
            avc1.avcc_box.bit_depth_luma_minus8 =
                avc1.avcc_box.bit_depth_luma_minus8.or(Some(Uint::new(0)));
            avc1.avcc_box.bit_depth_chroma_minus8 =
                avc1.avcc_box.bit_depth_chroma_minus8.or(Some(Uint::new(0)));
        }
    }
    entry
}

fn avc_entry(config: &transmux::AVCConfigurationBox, width: u16, height: u16) -> SampleEntry {
    let record = &config.config;
    // Real-world files in the wild (YouTube included) declare a high-family
    // profile yet omit the avcC tail the spec makes mandatory for it. The
    // tail values duplicate what the SPS already declares, and a high-profile
    // stream without override bits is 4:2:0 8-bit — so repair the record into
    // a spec-valid one instead of failing the whole merge at encode time.
    let high_profile = !matches!(record.profile_indication, 66 | 77 | 88);
    let repair = high_profile && record.chroma_format.is_none();
    let chroma_format = record.chroma_format.or(repair.then_some(1));
    let bit_depth_luma = record.bit_depth_luma_minus8.or(repair.then_some(0));
    let bit_depth_chroma = record.bit_depth_chroma_minus8.or(repair.then_some(0));
    SampleEntry::Avc1(Avc1Box {
        visual: visual_fields(width, height),
        avcc_box: AvccBox {
            avc_profile_indication: record.profile_indication,
            profile_compatibility: record.profile_compatibility,
            avc_level_indication: record.level_indication,
            length_size_minus_one: Uint::new(record.length_size_minus_one),
            sps_list: record.sps.iter().map(|sps| sps.0.clone()).collect(),
            pps_list: record.pps.iter().map(|pps| pps.0.clone()).collect(),
            chroma_format: chroma_format.map(Uint::new),
            bit_depth_luma_minus8: bit_depth_luma.map(Uint::new),
            bit_depth_chroma_minus8: bit_depth_chroma.map(Uint::new),
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

    /// High-profile avcC without the spec-mandated tail (chroma_format & co)
    /// is common in the wild — YouTube serves such files. The encoder must
    /// produce a spec-valid record instead of failing the merge.
    #[test]
    fn avc_entry_repairs_missing_high_profile_tail() {
        use shiguredo_mp4::Encode;
        use transmux::{AVCConfigurationBox, AVCDecoderConfigurationRecord};
        let record = AVCDecoderConfigurationRecord {
            configuration_version: 1,
            profile_indication: 100,
            profile_compatibility: 0,
            level_indication: 31,
            length_size_minus_one: 3,
            sps: vec![transmux::AvcSps(vec![0x67, 0x64, 0x00, 0x1f])],
            pps: vec![transmux::AvcPps(vec![0x68, 0xee, 0x3c, 0x80])],
            chroma_format: None,
            bit_depth_luma_minus8: None,
            bit_depth_chroma_minus8: None,
            sps_ext: Vec::new(),
        };
        let config = AVCConfigurationBox { config: record };
        let SampleEntry::Avc1(entry) = avc_entry(&config, 640, 360) else {
            panic!("avc1 entry");
        };
        assert_eq!(entry.avcc_box.chroma_format.map(|c| c.get()), Some(1));
        assert_eq!(entry.avcc_box.bit_depth_luma_minus8.map(|c| c.get()), Some(0));
        // The strict encoder accepts the repaired box.
        let mut buf = vec![0u8; 4096];
        assert!(entry.encode(&mut buf).is_ok(), "repaired avcC must encode");
    }

    /// Baseline-profile records legitimately carry no tail: nothing is invented.
    #[test]
    fn avc_entry_keeps_baseline_tail_absent() {
        use shiguredo_mp4::Encode;
        use transmux::{AVCConfigurationBox, AVCDecoderConfigurationRecord};
        let record = AVCDecoderConfigurationRecord {
            configuration_version: 1,
            profile_indication: 66,
            profile_compatibility: 0,
            level_indication: 30,
            length_size_minus_one: 3,
            sps: vec![transmux::AvcSps(vec![0x67, 0x42, 0x00, 0x1e])],
            pps: vec![transmux::AvcPps(vec![0x68, 0xce, 0x06, 0xe2])],
            chroma_format: None,
            bit_depth_luma_minus8: None,
            bit_depth_chroma_minus8: None,
            sps_ext: Vec::new(),
        };
        let config = AVCConfigurationBox { config: record };
        let SampleEntry::Avc1(entry) = avc_entry(&config, 640, 360) else {
            panic!("avc1 entry");
        };
        assert_eq!(entry.avcc_box.chroma_format, None);
        let mut buf = vec![0u8; 4096];
        assert!(entry.encode(&mut buf).is_ok(), "baseline avcC encodes without the tail");
    }

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

    fn dash_fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/dash").join(name)
    }

    fn collect_input_samples(path: &Path, kind: TrackKind) -> Vec<InputSampleMeta> {
        let mut reader = Mp4InputReader::open(path, kind, &|| false).unwrap();
        let mut samples = Vec::new();
        while let Some(sample) = reader.next_sample().unwrap() {
            samples.push(sample);
        }
        samples
    }

    fn input_sample_entry(path: &Path, kind: TrackKind) -> SampleEntry {
        Mp4InputReader::open(path, kind, &|| false).unwrap().sample_entry().clone()
    }

    type DemuxedSampleRow = (u32, u64, u32, bool, i64, usize, u64);

    fn collect_progressive_samples(path: &Path) -> Vec<DemuxedSampleRow> {
        let bytes = std::fs::read(path).unwrap();
        let mut demuxer = Mp4FileDemuxer::new();
        while let Some(required) = demuxer.required_input() {
            let position = required.position as usize;
            let size = required.size.unwrap_or(bytes.len() - position);
            demuxer.handle_input(Mp4DemuxInput {
                position: required.position,
                data: &bytes[position..position + size],
            });
        }
        let mut rows = Vec::new();
        while let Some(sample) = demuxer.next_sample().unwrap() {
            rows.push((
                sample.track.track_id,
                sample.timestamp,
                sample.duration,
                sample.keyframe,
                sample.composition_time_offset.unwrap_or(0),
                sample.data_size,
                sample.data_offset,
            ));
        }
        rows
    }

    fn decoded_mp4(path: &Path) -> Mp4File<RootBox> {
        let bytes = std::fs::read(path).unwrap();
        Mp4File::<RootBox>::decode(&bytes).unwrap().0
    }

    #[test]
    fn fragmented_merge_is_faststart_sample_exact_and_rebased() {
        let video_path = dash_fixture("video-frag.mp4");
        let audio_path = dash_fixture("audio-frag.m4a");
        let output = out("merge-frag.mp4");
        merge_av_mp4(&video_path, &audio_path, &output, &|| false).unwrap();

        let video_input = collect_input_samples(&video_path, TrackKind::Video);
        let audio_input = collect_input_samples(&audio_path, TrackKind::Audio);
        let output_rows = collect_progressive_samples(&output);
        let video_output: Vec<_> = output_rows.iter().copied().filter(|row| row.0 == 1).collect();
        let audio_output: Vec<_> = output_rows.iter().copied().filter(|row| row.0 == 2).collect();
        assert_eq!(video_output.len(), video_input.len());
        assert_eq!(audio_output.len(), audio_input.len());

        let video_base = video_input[0].dts;
        let audio_base = audio_input[0].dts;
        let video_input_bytes = std::fs::read(&video_path).unwrap();
        let audio_input_bytes = std::fs::read(&audio_path).unwrap();
        let output_bytes = std::fs::read(&output).unwrap();
        for (input, output) in video_input.iter().zip(&video_output) {
            assert_eq!(output.1 as i64, input.dts - video_base, "video DTS ladder is rebased");
            assert_eq!(output.2, input.duration);
            assert_eq!(output.3, input.keyframe);
            assert_eq!(output.4, input.composition_offset, "wire PTS-DTS offsets survive");
            assert_eq!(output.5, input.data_size, "sample payload sizes are exact");
            let output_offset = output.6 as usize;
            let input_offset = input.data_offset as usize;
            assert_eq!(
                &output_bytes[output_offset..output_offset + input.data_size],
                &video_input_bytes[input_offset..input_offset + input.data_size],
                "video sample payload bytes are copied exactly"
            );
        }
        for (input, output) in audio_input.iter().zip(&audio_output) {
            assert_eq!(output.1 as i64, input.dts - audio_base, "audio DTS ladder is rebased");
            assert_eq!(output.2, input.duration);
            assert_eq!(output.4, input.composition_offset);
            assert_eq!(output.5, input.data_size);
            let output_offset = output.6 as usize;
            let input_offset = input.data_offset as usize;
            assert_eq!(
                &output_bytes[output_offset..output_offset + input.data_size],
                &audio_input_bytes[input_offset..input_offset + input.data_size],
                "audio sample payload bytes are copied exactly"
            );
        }

        let mp4 = decoded_mp4(&output);
        let mut saw_moov = false;
        for root in &mp4.boxes {
            match root {
                RootBox::Moov(_) => saw_moov = true,
                RootBox::Mdat(_) => assert!(saw_moov, "merged MP4 must be faststart"),
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
        let video = moov
            .trak_boxes
            .iter()
            .find(|track| track.mdia_box.hdlr_box.handler_type == *b"vide")
            .expect("video");
        let audio = moov
            .trak_boxes
            .iter()
            .find(|track| track.mdia_box.hdlr_box.handler_type == *b"soun")
            .expect("audio");
        assert_eq!(
            video.mdia_box.minf_box.stbl_box.stsd_box.entries.first(),
            Some(&input_sample_entry(&video_path, TrackKind::Video)),
            "avc1 sample entry is copied verbatim"
        );
        assert_eq!(
            audio.mdia_box.minf_box.stbl_box.stsd_box.entries.first(),
            Some(&input_sample_entry(&audio_path, TrackKind::Audio)),
            "mp4a sample entry is copied verbatim"
        );
        let video_edit =
            video.edts_box.as_ref().and_then(|e| e.elst_box.as_ref()).expect("video edit list");
        assert!(video_edit.entries.iter().any(|entry| entry.media_time == -1));
        assert!(video_edit.entries.iter().any(|entry| entry.media_time == 1024));
        let audio_edit =
            audio.edts_box.as_ref().and_then(|e| e.elst_box.as_ref()).expect("audio edit list");
        assert!(audio_edit.entries.iter().all(|entry| entry.media_time == 0));

        let _ = std::fs::remove_file(output);
    }

    fn write_tfdt_offset_copy(source: &Path, dest: &Path, delta: u64) {
        let mut bytes = std::fs::read(source).unwrap();
        let positions: Vec<usize> = bytes
            .windows(4)
            .enumerate()
            .filter_map(|(index, window)| (window == b"tfdt").then_some(index))
            .collect();
        assert_eq!(positions.len(), 2, "fixture should contain two tfdt boxes");
        for type_position in positions {
            assert_eq!(bytes[type_position + 4], 1, "fixture uses version-1 tfdt");
            let value_offset = type_position + 8;
            let value =
                u64::from_be_bytes(bytes[value_offset..value_offset + 8].try_into().unwrap());
            bytes[value_offset..value_offset + 8].copy_from_slice(&(value + delta).to_be_bytes());
        }
        std::fs::write(dest, bytes).unwrap();
    }

    #[test]
    fn nonzero_fragment_decode_origin_is_rebased_to_zero() {
        let dir = std::env::temp_dir().join(format!("falcon-mux-tfdt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let shifted_video = dir.join("shifted-video.mp4");
        write_tfdt_offset_copy(&dash_fixture("video-frag.mp4"), &shifted_video, 7_777);
        let input = collect_input_samples(&shifted_video, TrackKind::Video);
        assert_eq!(input[0].dts, 7_777);

        let output = dir.join("merged.mp4");
        merge_av_mp4(&shifted_video, &dash_fixture("audio-frag.m4a"), &output, &|| false).unwrap();
        let video_output: Vec<_> =
            collect_progressive_samples(&output).into_iter().filter(|row| row.0 == 1).collect();
        assert_eq!(video_output[0].1, 0, "first output video DTS is zero");
        assert_eq!(video_output[0].4, input[0].composition_offset);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn progressive_inputs_merge_with_the_same_sample_tables() {
        let output = out("merge-progressive.mp4");
        merge_av_mp4(
            &dash_fixture("video-progressive.mp4"),
            &dash_fixture("audio-progressive.m4a"),
            &output,
            &|| false,
        )
        .unwrap();
        let mp4 = decoded_mp4(&output);
        assert_eq!(
            sample_sizes(&mp4, b"vide").0.len(),
            collect_input_samples(&dash_fixture("video-progressive.mp4"), TrackKind::Video).len()
        );
        assert_eq!(
            sample_sizes(&mp4, b"soun").0.len(),
            collect_input_samples(&dash_fixture("audio-progressive.m4a"), TrackKind::Audio).len()
        );
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn mixed_progressive_and_fragmented_inputs_merge() {
        let output = out("merge-mixed.mp4");
        merge_av_mp4(
            &dash_fixture("video-frag.mp4"),
            &dash_fixture("audio-progressive.m4a"),
            &output,
            &|| false,
        )
        .unwrap();
        assert_eq!(collect_progressive_samples(&output).len(), 60 + 88);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn progressive_inputs_with_moov_at_eof_merge() {
        let output = out("merge-late-moov.mp4");
        merge_av_mp4(
            &dash_fixture("video-progressive-late.mp4"),
            &dash_fixture("audio-progressive-late.m4a"),
            &output,
            &|| false,
        )
        .unwrap();
        assert_eq!(collect_progressive_samples(&output).len(), 60 + 88);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn h265_vp9_and_av1_entries_merge_verbatim_with_aac() {
        for (fixture, expected_codec) in [
            ("video-h265-frag.mp4", "h265"),
            ("video-vp9-frag.mp4", "vp9"),
            ("video-av1-frag.mp4", "av1"),
        ] {
            let video = dash_fixture(fixture);
            let output = out(&format!("merge-{fixture}"));
            merge_av_mp4(&video, &dash_fixture("audio-frag.m4a"), &output, &|| false).unwrap();
            let mp4 = decoded_mp4(&output);
            let moov = mp4
                .boxes
                .iter()
                .find_map(|b| match b {
                    RootBox::Moov(moov) => Some(moov),
                    _ => None,
                })
                .expect("moov");
            let video_track = moov
                .trak_boxes
                .iter()
                .find(|track| track.mdia_box.hdlr_box.handler_type == *b"vide")
                .expect("video");
            let output_entry = video_track
                .mdia_box
                .minf_box
                .stbl_box
                .stsd_box
                .entries
                .first()
                .expect("video sample entry");
            assert!(
                sample_entry_codec_label(output_entry).starts_with(expected_codec),
                "{fixture} codec entry changed"
            );
            assert_eq!(
                output_entry,
                &input_sample_entry(&video, TrackKind::Video),
                "{fixture} codec configuration must be copied verbatim"
            );
            let _ = std::fs::remove_file(output);
        }
    }

    #[test]
    fn multi_track_input_is_rejected_instead_of_dropping_a_track() {
        let combined = out("merge-combined-source.mp4");
        merge_av_mp4(
            &dash_fixture("video-frag.mp4"),
            &dash_fixture("audio-frag.m4a"),
            &combined,
            &|| false,
        )
        .unwrap();
        let output = out("merge-multi-track.mp4");
        let Err(MergeError::Fatal(message)) =
            merge_av_mp4(&combined, &dash_fixture("audio-frag.m4a"), &output, &|| false)
        else {
            panic!("multi-track input must be rejected");
        };
        assert!(message.contains("requires exactly one"), "{message}");
        assert!(!output.exists());
        let _ = std::fs::remove_file(combined);
    }

    #[test]
    fn invalid_mp4_input_fails_without_partial_output() {
        let dir = std::env::temp_dir().join(format!("falcon-mux-bad-mp4-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.mp4");
        std::fs::write(&bad, b"not an mp4").unwrap();
        let output = dir.join("out.mp4");
        let result = merge_av_mp4(&bad, &dash_fixture("audio-frag.m4a"), &output, &|| false);
        assert!(matches!(result, Err(MergeError::Fatal(_))), "{result:?}");
        assert!(!output.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn opus_audio_is_a_typed_unsupported_format_error() {
        let output = out("merge-opus.mp4");
        let result = merge_av_mp4(
            &dash_fixture("video-frag.mp4"),
            &dash_fixture("audio-opus.mp4"),
            &output,
            &|| false,
        );
        let Err(MergeError::UnsupportedFormat(unsupported)) = result else {
            panic!("Opus input must produce a typed unsupported-format error");
        };
        assert_eq!(unsupported.role, MergeTrackRole::Audio);
        assert!(unsupported.codec.contains("opus"), "{}", unsupported.codec);
        assert!(!output.exists());
    }

    #[test]
    fn merge_cancellation_leaves_no_partial_output() {
        let output = out("merge-cancel.mp4");
        let result = merge_av_mp4(
            &dash_fixture("video-frag.mp4"),
            &dash_fixture("audio-frag.m4a"),
            &output,
            &|| true,
        );
        assert_eq!(result, Err(MergeError::Cancelled));
        assert!(!output.exists());
    }

    #[cfg(unix)]
    fn max_rss_bytes() -> u64 {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        let ok = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
        assert_eq!(ok, 0);
        let usage = unsafe { usage.assume_init() };
        #[cfg(target_os = "macos")]
        {
            usage.ru_maxrss as u64
        }
        #[cfg(not(target_os = "macos"))]
        {
            usage.ru_maxrss as u64 * 1024
        }
    }

    /// The parent spawns this same test in an isolated test process so
    /// `ru_maxrss` is not polluted by the rest of the parallel unit-test suite.
    #[cfg(unix)]
    #[test]
    fn large_merge_memory_does_not_scale_with_payload_bytes() {
        const CHILD_ENV: &str = "FALCON_MUX_MEMORY_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            let executable = std::env::current_exe().unwrap();
            let output = std::process::Command::new(executable)
                .args([
                    "download::mux::tests::large_merge_memory_does_not_scale_with_payload_bytes",
                    "--exact",
                    "--test-threads=1",
                ])
                .env(CHILD_ENV, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "memory child failed:
{}",
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        // Byte-concatenated fragmented MP4 is a supported merge input; copies
        // of the fixture fragment stream make a meaningfully large input
        // without any external generator.
        let dir = std::env::temp_dir().join(format!("falcon-mux-memory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("large-video.mp4");
        let output = dir.join("merged.mp4");
        let fragment = std::fs::read(dash_fixture("video-frag.mp4")).unwrap();
        // Only the media-fragment tail (first `moof` onward) may repeat: the
        // init segment is written once, then the fragment stream continues.
        let moof = fragment
            .windows(4)
            .position(|w| w == b"moof")
            .map(|i| i - 4)
            .expect("fragmented fixture has a moof box");
        // Repeated fragments must continue the decode timeline: each copy's
        // tfdt is shifted by the running stride of the copies before it.
        let samples = collect_input_samples(&dash_fixture("video-frag.mp4"), TrackKind::Video);
        let first_dts = samples[0].dts;
        let stride =
            samples.iter().map(|s| (s.dts - first_dts) + s.duration as i64).max().unwrap() as u64;
        let tail = &fragment[moof..];
        let tfdt_positions: Vec<usize> = tail
            .windows(4)
            .enumerate()
            .filter_map(|(index, window)| (window == b"tfdt").then_some(index))
            .collect();
        assert!(!tfdt_positions.is_empty(), "fragment tail carries tfdt boxes");
        let copies = (40 * 1024 * 1024) / fragment.len() + 1;
        {
            use std::io::Write;
            let mut writer = std::io::BufWriter::new(std::fs::File::create(&video).unwrap());
            writer.write_all(&fragment).unwrap();
            for copy in 1..copies {
                let mut patched = tail.to_vec();
                for type_position in &tfdt_positions {
                    let value_offset = type_position + 8;
                    let value = u64::from_be_bytes(
                        patched[value_offset..value_offset + 8].try_into().unwrap(),
                    );
                    patched[value_offset..value_offset + 8]
                        .copy_from_slice(&(value + stride * copy as u64).to_be_bytes());
                }
                writer.write_all(&patched).unwrap();
            }
        }
        let input_bytes = video.metadata().unwrap().len();
        assert!(input_bytes > 32 * 1024 * 1024, "fixture must be meaningfully large");
        merge_av_mp4(&video, &dash_fixture("audio-frag.m4a"), &output, &|| false).unwrap();
        let peak = max_rss_bytes();
        eprintln!("large merge peak RSS: {peak} bytes for a {input_bytes}-byte video input");
        // A file-sized Vec or segment-sized demuxer input would exceed this
        // budget immediately. Legitimate metadata tables, the muxer state and
        // the 1 MiB payload copy buffer remain far below it.
        let budget = input_bytes * 3 / 4;
        assert!(
            peak < budget,
            "peak RSS {peak} bytes scaled with a {input_bytes}-byte input (budget {budget})"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
