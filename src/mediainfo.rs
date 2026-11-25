//! The app's own reader of a finished media file's facts (length, sound,
//! frame size), so no ffprobe is needed for the common files.
//!
//! It reads only the container's headers, never the media bytes:
//! * MP4 / M4A / MOV (ISO-BMFF): `moov` → `mvhd`, and per track `mdhd`,
//!   `hdlr` and the first sample description (`stsd`).
//! * Matroska / WebM (EBML): `Segment` → `Info` (`Duration`) and `Tracks`.
//!
//! The rule is "never guess": a value it cannot read for certain is `None`,
//! and a file it does not know returns `None` as a whole. The caller then
//! asks ffprobe or ffmpeg, exactly as before, so nothing can come out less
//! accurate than with ffprobe alone.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// One video track's facts.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoTrack {
    /// The codec as a lower-case tag: an MP4 sample entry ("avc1", "jpeg")
    /// or a Matroska CodecID ("v_mpeg4/iso/avc", "v_mjpeg").
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// Number of frames, when the container says (MP4 only).
    pub frames: Option<u64>,
}

/// What the headers say about a file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaFacts {
    /// Length in seconds; `None` when the headers do not state it for
    /// certain (a fragmented MP4, a Matroska file written without one).
    pub duration: Option<f64>,
    /// Any audio track.
    pub has_audio: bool,
    /// The video tracks, in file order.
    pub video: Vec<VideoTrack>,
}

impl MediaFacts {
    /// The first video track that is real video: not a still-image codec
    /// and not a single frame. `None` = no moving picture in the file.
    pub fn moving_video(&self) -> Option<&VideoTrack> {
        self.video.iter().find(|v| !still_codec(&v.codec) && v.frames != Some(1))
    }
}

/// Still-picture codecs as they appear in MP4 sample entries and Matroska
/// CodecIDs (a photo, cover art, a thumbnail track).
fn still_codec(codec: &str) -> bool {
    matches!(
        codec,
        "jpeg" | "png " | "png" | "mjp2" | "mjpa" | "mjpb" | "gif " | "bmp " | "tiff" | "webp" | "v_mjpeg"
    )
}

/// Read the facts of `path`, or `None` when the file is not an MP4/MOV or
/// Matroska/WebM file whose headers this reader can read whole.
pub fn read(path: &Path) -> Option<MediaFacts> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut magic = [0u8; 8];
    f.read_exact(&mut magic).ok()?;
    if magic[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return read_matroska(&mut f, len);
    }
    if matches!(&magic[4..8], b"ftyp" | b"moov" | b"free" | b"skip" | b"wide" | b"mdat") {
        return read_iso(&mut f, len);
    }
    None
}

// ---------------------------------------------------------------- MP4 / MOV

fn be16(b: &[u8], i: usize) -> Option<u64> {
    b.get(i..i + 2).map(|s| u16::from_be_bytes([s[0], s[1]]) as u64)
}
fn be32(b: &[u8], i: usize) -> Option<u64> {
    b.get(i..i + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as u64)
}
fn be64(b: &[u8], i: usize) -> Option<u64> {
    b.get(i..i + 8).map(|s| u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

/// The child boxes of `data`: (type, body). `None` when a size is broken.
fn iso_children(data: &[u8]) -> Option<Vec<([u8; 4], &[u8])>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 8 <= data.len() {
        let size32 = be32(data, i)?;
        let kind = [data[i + 4], data[i + 5], data[i + 6], data[i + 7]];
        let (size, head) = match size32 {
            0 => ((data.len() - i) as u64, 8usize),
            1 => (be64(data, i + 8)?, 16usize),
            n => (n, 8usize),
        };
        let size = usize::try_from(size).ok()?;
        if size < head || i + size > data.len() {
            return None;
        }
        out.push((kind, &data[i + head..i + size]));
        i += size;
    }
    Some(out)
}

fn find<'a>(boxes: &'a [([u8; 4], &'a [u8])], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes.iter().find(|(k, _)| k == kind).map(|(_, b)| *b)
}

fn read_iso(f: &mut std::fs::File, len: u64) -> Option<MediaFacts> {
    // Walk the top level to `moov` (at the front or at the end).
    let mut pos = 0u64;
    let mut moov: Option<Vec<u8>> = None;
    let mut fragments = false;
    while pos + 8 <= len {
        let mut h = [0u8; 16];
        f.seek(SeekFrom::Start(pos)).ok()?;
        f.read_exact(&mut h[..8]).ok()?;
        let kind = [h[4], h[5], h[6], h[7]];
        let (size, head) = match u32::from_be_bytes([h[0], h[1], h[2], h[3]]) {
            0 => (len - pos, 8u64),
            1 => {
                f.read_exact(&mut h[8..16]).ok()?;
                (u64::from_be_bytes([h[8], h[9], h[10], h[11], h[12], h[13], h[14], h[15]]), 16)
            }
            n => (n as u64, 8),
        };
        if size < head || pos.checked_add(size)? > len {
            // A cut-off file: what is there cannot be trusted.
            return None;
        }
        match &kind {
            b"moov" => {
                if size - head > 64 << 20 {
                    return None;
                }
                let mut body = vec![0u8; (size - head) as usize];
                f.read_exact(&mut body).ok()?;
                moov = Some(body);
            }
            b"moof" | b"sidx" | b"styp" => fragments = true,
            _ => {}
        }
        pos += size;
    }
    let moov = moov?;
    let top = iso_children(&moov)?;
    if find(&top, b"mvex").is_some() {
        fragments = true;
    }
    let mvhd = find(&top, b"mvhd")?;
    let (timescale, duration) = match mvhd.first()? {
        0 => (be32(mvhd, 12)?, be32(mvhd, 16)?),
        1 => (be32(mvhd, 20)?, be64(mvhd, 24)?),
        _ => return None,
    };
    let mut facts = MediaFacts::default();
    let mut longest = 0f64;
    for (kind, trak) in &top {
        if kind != b"trak" {
            continue;
        }
        let t = iso_children(trak)?;
        let mdia = iso_children(find(&t, b"mdia")?)?;
        let hdlr = find(&mdia, b"hdlr")?;
        let handler = hdlr.get(8..12)?;
        if let Some(mdhd) = find(&mdia, b"mdhd") {
            let (ts, d) = match mdhd.first()? {
                0 => (be32(mdhd, 12)?, be32(mdhd, 16)?),
                1 => (be32(mdhd, 20)?, be64(mdhd, 24)?),
                _ => return None,
            };
            if ts > 0 && d > 0 && d != u32::MAX as u64 && d != u64::MAX {
                longest = longest.max(d as f64 / ts as f64);
            }
        }
        match handler {
            b"soun" => facts.has_audio = true,
            b"vide" => {
                let minf = iso_children(find(&mdia, b"minf")?)?;
                let stbl = iso_children(find(&minf, b"stbl")?)?;
                let stsd = find(&stbl, b"stsd")?;
                // version/flags + entry count, then the first entry.
                let entry = stsd.get(8..)?;
                let codec = String::from_utf8_lossy(entry.get(4..8)?).to_ascii_lowercase();
                // VisualSampleEntry: 8 head + 6 reserved + 2 index + 16
                // pre-defined/reserved, then width and height.
                let width = be16(entry, 32)? as u32;
                let height = be16(entry, 34)? as u32;
                let frames = find(&stbl, b"stsz").and_then(|s| be32(s, 8)).filter(|n| *n > 0);
                facts.video.push(VideoTrack { codec, width, height, frames: if fragments { None } else { frames } });
            }
            _ => {}
        }
    }
    // Fragmented files keep their samples (and length) in the fragments:
    // leave the length to ffprobe/ffmpeg there.
    if !fragments && timescale > 0 && duration > 0 && duration != u32::MAX as u64 && duration != u64::MAX {
        facts.duration = Some((duration as f64 / timescale as f64).max(longest));
    }
    Some(facts)
}

// ---------------------------------------------------------- Matroska / WebM

const SEGMENT: u64 = 0x1853_8067;
const INFO: u64 = 0x1549_A966;
const TIMECODE_SCALE: u64 = 0x2A_D7B1;
const DURATION: u64 = 0x4489;
const TRACKS: u64 = 0x1654_AE6B;
const TRACK_ENTRY: u64 = 0xAE;
const TRACK_TYPE: u64 = 0x83;
const CODEC_ID: u64 = 0x86;
const VIDEO: u64 = 0xE0;
const PIXEL_WIDTH: u64 = 0xB0;
const PIXEL_HEIGHT: u64 = 0xBA;
const CLUSTER: u64 = 0x1F43_B675;

/// An EBML element ID (its marker bits kept, as IDs are written).
fn ebml_id(b: &[u8], i: usize) -> Option<(u64, usize)> {
    let first = *b.get(i)?;
    let n = first.leading_zeros() as usize + 1;
    if n > 4 {
        return None;
    }
    let mut v = 0u64;
    for k in 0..n {
        v = (v << 8) | *b.get(i + k)? as u64;
    }
    Some((v, n))
}

/// An EBML size; `u64::MAX` means "unknown size".
fn ebml_size(b: &[u8], i: usize) -> Option<(u64, usize)> {
    let first = *b.get(i)?;
    let n = first.leading_zeros() as usize + 1;
    if n > 8 {
        return None;
    }
    let mut v = (first as u64) & (0xFFu64 >> n);
    let mut all_ones = v == (0xFFu64 >> n);
    for k in 1..n {
        let byte = *b.get(i + k)?;
        all_ones &= byte == 0xFF;
        v = (v << 8) | byte as u64;
    }
    Some((if all_ones { u64::MAX } else { v }, n))
}

/// The child elements of `data`: (id, body). `None` when a size is broken
/// or unknown (only the Segment and Clusters may have unknown sizes).
fn ebml_children(data: &[u8]) -> Option<Vec<(u64, &[u8])>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let (id, a) = ebml_id(data, i)?;
        let (size, b) = ebml_size(data, i + a)?;
        let start = i + a + b;
        let size = usize::try_from(size).ok()?;
        let end = start.checked_add(size)?;
        if end > data.len() {
            return None;
        }
        out.push((id, &data[start..end]));
        i = end;
    }
    Some(out)
}

fn ebml_uint(b: &[u8]) -> Option<u64> {
    if b.is_empty() || b.len() > 8 {
        return None;
    }
    Some(b.iter().fold(0u64, |v, x| (v << 8) | *x as u64))
}

fn ebml_float(b: &[u8]) -> Option<f64> {
    match b.len() {
        4 => Some(f32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64),
        8 => Some(f64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])),
        _ => None,
    }
}

fn read_matroska(f: &mut std::fs::File, len: u64) -> Option<MediaFacts> {
    // Info and Tracks sit before the first Cluster in every file ffmpeg,
    // yt-dlp and browsers write; reading the first 16 MB covers them.
    let want = len.min(16 << 20) as usize;
    let mut buf = vec![0u8; want];
    f.seek(SeekFrom::Start(0)).ok()?;
    f.read_exact(&mut buf).ok()?;
    // EBML header, then the Segment.
    let (id, a) = ebml_id(&buf, 0)?;
    if id != 0x1A45_DFA3 {
        return None;
    }
    let (hsize, b) = ebml_size(&buf, a)?;
    let mut i = a + b + usize::try_from(hsize).ok()?;
    let (seg, a) = ebml_id(&buf, i)?;
    if seg != SEGMENT {
        return None;
    }
    let (_, b) = ebml_size(&buf, i + a)?;
    i += a + b;
    let mut info: Option<&[u8]> = None;
    let mut tracks: Option<&[u8]> = None;
    // Walk the Segment's children until both are found or a Cluster starts.
    while i < buf.len() && (info.is_none() || tracks.is_none()) {
        let (id, a) = ebml_id(&buf, i)?;
        if id == CLUSTER {
            break;
        }
        let (size, b) = ebml_size(&buf, i + a)?;
        let start = i + a + b;
        let end = start.checked_add(usize::try_from(size).ok()?)?;
        if end > buf.len() {
            return None;
        }
        match id {
            INFO => info = Some(&buf[start..end]),
            TRACKS => tracks = Some(&buf[start..end]),
            _ => {}
        }
        i = end;
    }
    let (info, tracks) = (info?, tracks?);
    let mut facts = MediaFacts::default();
    let info = ebml_children(info)?;
    let scale = info.iter().find(|(id, _)| *id == TIMECODE_SCALE).and_then(|(_, b)| ebml_uint(b)).unwrap_or(1_000_000);
    if let Some(d) = info.iter().find(|(id, _)| *id == DURATION).and_then(|(_, b)| ebml_float(b)) {
        let secs = d * scale as f64 / 1e9;
        if secs.is_finite() && secs > 0.0 {
            facts.duration = Some(secs);
        }
    }
    for (id, entry) in ebml_children(tracks)? {
        if id != TRACK_ENTRY {
            continue;
        }
        let e = ebml_children(entry)?;
        let kind = e.iter().find(|(id, _)| *id == TRACK_TYPE).and_then(|(_, b)| ebml_uint(b));
        match kind {
            Some(2) => facts.has_audio = true,
            Some(1) => {
                let codec = e
                    .iter()
                    .find(|(id, _)| *id == CODEC_ID)
                    .map(|(_, b)| String::from_utf8_lossy(b).trim_end_matches('\0').to_ascii_lowercase())
                    .unwrap_or_default();
                let v = ebml_children(e.iter().find(|(id, _)| *id == VIDEO).map(|(_, b)| *b)?)?;
                let px = |want: u64| v.iter().find(|(id, _)| *id == want).and_then(|(_, b)| ebml_uint(b));
                facts.video.push(VideoTrack {
                    codec,
                    width: px(PIXEL_WIDTH)? as u32,
                    height: px(PIXEL_HEIGHT)? as u32,
                    frames: None,
                });
            }
            _ => {}
        }
    }
    Some(facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ebml_sizes() {
        assert_eq!(ebml_size(&[0x81], 0), Some((1, 1)));
        assert_eq!(ebml_size(&[0x40, 0x02], 0), Some((2, 2)));
        assert_eq!(ebml_size(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], 0), Some((u64::MAX, 8)));
        assert_eq!(ebml_id(&[0x1A, 0x45, 0xDF, 0xA3], 0), Some((0x1A45_DFA3, 4)));
    }

    #[test]
    fn unknown_files_are_left_to_ffprobe() {
        let dir = std::env::temp_dir().join(format!("dc_mi_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.mp3");
        std::fs::write(&p, b"ID3\x04\0\0\0\0\0\0 not a container").unwrap();
        assert_eq!(read(&p), None);
        // A cut-off MP4: a box running past the end.
        let q = dir.join("cut.mp4");
        std::fs::write(&q, [0, 0, 0, 24, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm', 0, 0, 2, 0, 0, 0, 1, 0, b'm', b'o', b'o', b'v']).unwrap();
        assert_eq!(read(&q), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn still_pictures_are_not_video() {
        let mut f = MediaFacts::default();
        f.video.push(VideoTrack { codec: "jpeg".into(), width: 600, height: 600, frames: None });
        f.video.push(VideoTrack { codec: "avc1".into(), width: 640, height: 360, frames: Some(1) });
        assert_eq!(f.moving_video(), None);
        f.video.push(VideoTrack { codec: "avc1".into(), width: 1280, height: 720, frames: Some(900) });
        assert_eq!(f.moving_video().map(|v| v.height), Some(720));
    }
}

/// Compare with ffprobe by hand: `DC_MEDIAINFO_DIR=<folder> cargo test
/// --release mediainfo_dump -- --ignored --nocapture` prints one line per file.
#[cfg(test)]
#[test]
#[ignore]
fn mediainfo_dump() {
    let list = std::env::var("DC_MEDIAINFO_DIR").expect("DC_MEDIAINFO_DIR");
    for p in std::fs::read_to_string(&list).unwrap().lines().map(|l| std::path::PathBuf::from(l.trim())) {
        match read(&p) {
            Some(f) => {
                let v = f.moving_video().map(|v| format!("{}x{}", v.width, v.height)).unwrap_or_else(|| "-".into());
                let d = f.duration.map(|d| format!("{d:.3}")).unwrap_or_else(|| "?".into());
                println!("MI\t{}\t{}\t{}\t{}", p.file_name().unwrap().to_string_lossy(), d, f.has_audio, v);
            }
            None => println!("MI\t{}\tNONE", p.file_name().unwrap().to_string_lossy()),
        }
    }
}
