//! Minimal, dependency-free MP4 (AVC + AAC-LC) to MPEG-TS remuxer for
//! Telegram's one-second, self-contained live segments. No transcoding.
//! Intentionally rejects unknown codecs and malformed sample tables.
use std::cmp::min;

type RemuxResult<T> = Result<T, &'static str>;

fn be16(b: &[u8], at: usize) -> RemuxResult<u16> {
    let x = b.get(at..at + 2).ok_or("short u16")?;
    Ok(u16::from_be_bytes([x[0], x[1]]))
}
fn be32(b: &[u8], at: usize) -> RemuxResult<u32> {
    let x = b.get(at..at + 4).ok_or("short u32")?;
    Ok(u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}
fn be64(b: &[u8], at: usize) -> RemuxResult<u64> {
    let x = b.get(at..at + 8).ok_or("short u64")?;
    Ok(u64::from_be_bytes(x.try_into().map_err(|_| "short u64")?))
}
fn children(mut bytes: &[u8]) -> RemuxResult<Vec<([u8; 4], &[u8])>> {
    let mut output = Vec::new();
    while !bytes.is_empty() {
        if bytes.len() < 8 {
            return Err("short MP4 box");
        }
        let tag: [u8; 4] = bytes[4..8].try_into().map_err(|_| "box tag")?;
        let mut header = 8usize;
        let size = match be32(bytes, 0)? {
            0 => bytes.len(),
            1 => {
                header = 16;
                usize::try_from(be64(bytes, 8)?).map_err(|_| "large MP4 box")?
            }
            x => x as usize,
        };
        if size < header || size > bytes.len() {
            return Err("invalid MP4 box length");
        }
        output.push((tag, &bytes[header..size]));
        bytes = &bytes[size..];
    }
    Ok(output)
}
fn child<'a>(bytes: &'a [u8], name: &[u8; 4]) -> RemuxResult<&'a [u8]> {
    children(bytes)?
        .into_iter()
        .find(|(tag, _)| tag == name)
        .map(|(_, data)| data)
        .ok_or("MP4 box unavailable")
}
fn table(bytes: &[u8], width: usize) -> RemuxResult<Vec<&[u8]>> {
    let count = be32(bytes, 4)? as usize;
    if count > 100_000 || 8usize.saturating_add(count.saturating_mul(width)) > bytes.len() {
        return Err("invalid MP4 sample table");
    }
    Ok((0..count)
        .map(|i| &bytes[8 + i * width..8 + (i + 1) * width])
        .collect())
}
fn timescale(mdia: &[u8]) -> RemuxResult<u32> {
    let mdhd = child(mdia, b"mdhd")?;
    let scale = be32(mdhd, if mdhd.first() == Some(&1) { 20 } else { 12 })?;
    if scale == 0 {
        return Err("invalid MP4 timescale");
    }
    Ok(scale)
}
fn map_frequency(rate: u32) -> RemuxResult<u8> {
    match rate {
        96000 => Ok(0),
        88200 => Ok(1),
        64000 => Ok(2),
        48000 => Ok(3),
        44100 => Ok(4),
        32000 => Ok(5),
        24000 => Ok(6),
        22050 => Ok(7),
        16000 => Ok(8),
        12000 => Ok(9),
        11025 => Ok(10),
        8000 => Ok(11),
        7350 => Ok(12),
        _ => Err("unsupported AAC sample rate"),
    }
}
struct Track {
    video: bool,
    scale: u32,
    samples: Vec<Sample>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    freq: u8,
    channels: u8,
}
struct Sample {
    dts: i64,
    pts: i64,
    offset: usize,
    size: usize,
    key: bool,
}
fn parse_track(mp4: &[u8], trak: &[u8]) -> RemuxResult<Option<Track>> {
    let mdia = child(trak, b"mdia")?;
    let handler = child(mdia, b"hdlr")?;
    let video = match handler.get(8..12).ok_or("short handler")? {
        b"vide" => true,
        b"soun" => false,
        _ => return Ok(None),
    };
    let scale = timescale(mdia)?;
    let stbl = child(child(mdia, b"minf")?, b"stbl")?;
    let stsd = child(stbl, b"stsd")?;
    let entry = children(stsd.get(8..).ok_or("short stsd")?)?
        .into_iter()
        .next()
        .ok_or("empty stsd")?;
    let mut sps = Vec::new();
    let mut pps = Vec::new();
    let mut freq = 0;
    let mut channels = 0;
    if video {
        if entry.0 != *b"avc1" {
            return Err("only H.264 video is supported");
        }
        let avcc = child(entry.1.get(78..).ok_or("short avc1")?, b"avcC")?;
        if avcc.len() < 7 || avcc[4] & 3 != 3 {
            return Err("unsupported AVC NAL length");
        }
        let count = usize::from(avcc[5] & 31);
        if count < 1 {
            return Err("AVC has no SPS");
        }
        let mut pos = 6;
        for i in 0..count {
            let len = be16(avcc, pos)? as usize;
            pos += 2;
            let bytes = avcc.get(pos..pos + len).ok_or("short SPS")?;
            if i == 0 {
                sps.extend_from_slice(bytes);
            }
            pos += len;
        }
        let pps_count = usize::from(*avcc.get(pos).ok_or("no PPS count")?);
        pos += 1;
        if pps_count == 0 {
            return Err("AVC has no PPS");
        }
        let len = be16(avcc, pos)? as usize;
        pos += 2;
        pps.extend_from_slice(avcc.get(pos..pos + len).ok_or("short PPS")?);
    } else {
        if entry.0 != *b"mp4a" {
            return Err("only AAC audio is supported");
        }
        channels = u8::try_from(be16(entry.1, 16)?).map_err(|_| "invalid AAC channels")?;
        if channels == 0 || channels > 7 {
            return Err("invalid AAC channel count");
        }
        freq = map_frequency(be32(entry.1, 24)? >> 16)?;
    }
    let stsz = child(stbl, b"stsz")?;
    let fixed_size = be32(stsz, 4)? as usize;
    let count = be32(stsz, 8)? as usize;
    if count == 0 || count > 100_000 {
        return Err("invalid sample count");
    }
    let sizes = if fixed_size > 0 {
        vec![fixed_size; count]
    } else {
        let payload = stsz.get(4..).ok_or("short stsz")?;
        let entries = table(payload, 4)?;
        if entries.len() != count {
            return Err("sample count mismatch");
        }
        entries
            .iter()
            .map(|b| be32(b, 0).map(|x| x as usize))
            .collect::<RemuxResult<Vec<_>>>()?
    };
    let stts = table(child(stbl, b"stts")?, 8)?;
    let mut dts = Vec::with_capacity(count);
    let mut cursor = 0i64;
    for row in stts {
        let repeats = be32(row, 0)? as usize;
        let delta = i64::from(be32(row, 4)?);
        if repeats > count.saturating_sub(dts.len()) {
            return Err("invalid stts sample count");
        }
        for _ in 0..repeats {
            dts.push(cursor);
            cursor += delta;
        }
    }
    if dts.len() != count {
        return Err("missing timing samples");
    }
    let mut cto = vec![0i64; count];
    if let Ok(ctts) = child(stbl, b"ctts") {
        let signed = ctts.first() == Some(&1);
        let mut idx = 0usize;
        for row in table(ctts, 8)? {
            let n = be32(row, 0)? as usize;
            let value = be32(row, 4)?;
            let offset = if signed {
                i64::from(value as i32)
            } else {
                i64::from(value)
            };
            if n > count.saturating_sub(idx) {
                return Err("invalid ctts sample count");
            }
            for item in &mut cto[idx..idx + n] {
                *item = offset;
            }
            idx += n;
        }
        if idx != count {
            return Err("missing ctts entries");
        }
    }
    let sync = match child(stbl, b"stss") {
        Ok(stss) => table(stss, 4)?
            .into_iter()
            .map(|b| be32(b, 0))
            .collect::<RemuxResult<Vec<_>>>()?,
        Err(_) => Vec::new(),
    };
    let chunks = if let Ok(stco) = child(stbl, b"stco") {
        table(stco, 4)?
            .into_iter()
            .map(|b| be32(b, 0).map(u64::from))
            .collect::<RemuxResult<Vec<_>>>()?
    } else {
        table(child(stbl, b"co64")?, 8)?
            .into_iter()
            .map(|b| be64(b, 0))
            .collect::<RemuxResult<Vec<_>>>()?
    };
    let stsc = table(child(stbl, b"stsc")?, 12)?;
    let chunk_rules = stsc
        .iter()
        .map(|b| Ok((be32(b, 0)? as usize, be32(b, 4)? as usize)))
        .collect::<RemuxResult<Vec<_>>>()?;
    if chunks.is_empty() || chunk_rules.is_empty() {
        return Err("no media chunks");
    }
    let mut samples = Vec::with_capacity(count);
    let mut index = 0;
    for (chunk_i, chunk_offset) in chunks.iter().enumerate() {
        let chunk_number = chunk_i + 1;
        let rule = chunk_rules
            .iter()
            .rev()
            .find(|(first, _)| *first <= chunk_number)
            .ok_or("missing stsc rule")?;
        let mut at = usize::try_from(*chunk_offset).map_err(|_| "chunk offset overflow")?;
        for _ in 0..rule.1 {
            if index >= count {
                break;
            }
            let size = sizes[index];
            if size == 0 || at.checked_add(size).ok_or("sample offset overflow")? > mp4.len() {
                return Err("MP4 sample outside file");
            }
            samples.push(Sample {
                dts: dts[index],
                pts: dts[index] + cto[index],
                offset: at,
                size,
                key: sync.is_empty() || sync.contains(&((index + 1) as u32)),
            });
            at += size;
            index += 1;
        }
    }
    if index != count {
        return Err("missing MP4 samples");
    }
    Ok(Some(Track {
        video,
        scale,
        samples,
        sps,
        pps,
        freq,
        channels,
    }))
}
fn annexb(bytes: &[u8], track: &Track, key: bool) -> RemuxResult<Vec<u8>> {
    let mut result = Vec::with_capacity(bytes.len() + 100);
    result.extend_from_slice(&[0, 0, 0, 1, 9, 0xf0]);
    if key {
        for nal in [&track.sps, &track.pps] {
            result.extend_from_slice(&[0, 0, 0, 1]);
            result.extend_from_slice(nal);
        }
    }
    let mut offset = 0;
    while offset < bytes.len() {
        let size = be32(bytes, offset)? as usize;
        offset += 4;
        if size == 0 || offset + size > bytes.len() {
            return Err("malformed H.264 NAL unit");
        }
        result.extend_from_slice(&[0, 0, 0, 1]);
        result.extend_from_slice(&bytes[offset..offset + size]);
        offset += size;
    }
    Ok(result)
}
fn adts(bytes: &[u8], track: &Track) -> RemuxResult<Vec<u8>> {
    let len = bytes.len() + 7;
    if len > 8191 {
        return Err("AAC frame too large");
    }
    let ch = track.channels;
    let mut output = Vec::with_capacity(len);
    output.extend_from_slice(&[
        0xff,
        0xf1,
        (1 << 6) | (track.freq << 2) | (ch >> 2),
        ((ch & 3) << 6) | ((len >> 11) as u8 & 3),
        (len >> 3) as u8,
        (((len & 7) as u8) << 5) | 0x1f,
        0xfc,
    ]);
    output.extend_from_slice(bytes);
    Ok(output)
}
fn crc32_mpeg(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}
fn section(out: &mut Vec<u8>, pid: u16, cc: &mut [u8; 4], payload: &[u8]) {
    let mut packet = [0xffu8; 188];
    packet[..4].copy_from_slice(&[
        0x47,
        0x40 | (pid >> 8) as u8 & 0x1f,
        pid as u8,
        0x10 | cc[pid_index(pid)],
    ]);
    packet[4] = 0; // pointer_field
    packet[5..5 + payload.len()].copy_from_slice(payload);
    cc[pid_index(pid)] = (cc[pid_index(pid)] + 1) & 15;
    out.extend_from_slice(&packet);
}
fn pid_index(pid: u16) -> usize {
    match pid {
        0 => 0,
        0x1000 => 1,
        0x100 => 2,
        _ => 3,
    }
}
fn psi(out: &mut Vec<u8>, cc: &mut [u8; 4]) {
    let mut pat = vec![0x00, 0xb0, 0x0d, 0, 1, 0xc1, 0, 0, 0, 1, 0xf0, 0x00];
    pat.extend_from_slice(&crc32_mpeg(&pat).to_be_bytes());
    section(out, 0, cc, &pat);
    let mut pmt = vec![
        0x02, 0xb0, 0x17, 0, 1, 0xc1, 0, 0, 0xe1, 0, 0xf0, 0, 0x1b, 0xe1, 0x00, 0xf0, 0, 0x0f,
        0xe1, 0x01, 0xf0, 0,
    ];
    pmt.extend_from_slice(&crc32_mpeg(&pmt).to_be_bytes());
    section(out, 0x1000, cc, &pmt);
}
fn stamp(t: u64, prefix: u8) -> [u8; 5] {
    let v = t & 0x1_ffff_ffff;
    [
        (prefix << 4) | (((v >> 30) as u8 & 7) << 1) | 1,
        (v >> 22) as u8,
        (((v >> 15) as u8 & 0x7f) << 1) | 1,
        (v >> 7) as u8,
        ((v as u8 & 0x7f) << 1) | 1,
    ]
}
fn packets(out: &mut Vec<u8>, cc: &mut [u8; 4], pid: u16, payload: &[u8], pcr: Option<u64>) {
    let index = pid_index(pid);
    let mut offset = 0;
    let mut first = true;
    while offset < payload.len() {
        let mut packet = [0xffu8; 188];
        packet[0] = 0x47;
        packet[1] = ((pid >> 8) as u8 & 0x1f) | if first { 0x40 } else { 0 };
        packet[2] = pid as u8;
        let must_pcr = first && pcr.is_some();
        let available = if must_pcr { 176 } else { 184 };
        let take = min(available, payload.len() - offset);
        if must_pcr || take < 184 {
            packet[3] = 0x30 | cc[index];
            let adapt_len = 183 - take;
            packet[4] = adapt_len as u8;
            if adapt_len > 0 {
                packet[5] = if must_pcr { 0x10 } else { 0 };
                if let Some(clock) = pcr.filter(|_| must_pcr) {
                    let b = clock & 0x1_ffff_ffff;
                    packet[6..12].copy_from_slice(&[
                        (b >> 25) as u8,
                        (b >> 17) as u8,
                        (b >> 9) as u8,
                        (b >> 1) as u8,
                        ((b as u8 & 1) << 7) | 0x7e,
                        0,
                    ]);
                }
            }
            let start = 5 + adapt_len;
            packet[start..start + take].copy_from_slice(&payload[offset..offset + take]);
        } else {
            packet[3] = 0x10 | cc[index];
            packet[4..188].copy_from_slice(&payload[offset..offset + take]);
        }
        cc[index] = (cc[index] + 1) & 15;
        out.extend_from_slice(&packet);
        offset += take;
        first = false;
    }
}
fn pes(
    out: &mut Vec<u8>,
    cc: &mut [u8; 4],
    video: bool,
    pts: u64,
    dts: u64,
    data: &[u8],
    pcr: bool,
) {
    let pid = if video { 0x100 } else { 0x101 };
    let mut stream = Vec::with_capacity(data.len() + 20);
    stream.extend_from_slice(&[0, 0, 1, if video { 0xe0 } else { 0xc0 }]);
    let both = video && pts != dts;
    let length = data.len() + if both { 13 } else { 8 };
    let length = if length > 0xffff { 0 } else { length as u16 };
    stream.extend_from_slice(&length.to_be_bytes());
    stream.extend_from_slice(&[
        0x80,
        if both { 0xc0 } else { 0x80 },
        if both { 10 } else { 5 },
    ]);
    stream.extend_from_slice(&stamp(pts, if both { 3 } else { 2 }));
    if both {
        stream.extend_from_slice(&stamp(dts, 1));
    }
    stream.extend_from_slice(data);
    packets(out, cc, pid, &stream, if pcr { Some(dts) } else { None });
}
pub struct Continuity {
    cc: [u8; 4],
}
impl Continuity {
    pub fn new() -> Self {
        Self { cc: [0; 4] }
    }
}
/// All timestamps are based on the Telegram segment timestamp (milliseconds).
/// An offset of 90000 ticks allows samples with negative CTS/DTS edit offsets.
pub fn remux(mp4: &[u8], segment_ms: i64, state: &mut Continuity) -> RemuxResult<Vec<u8>> {
    if mp4.len() > 2_000_000 {
        return Err("MP4 segment too large");
    }
    let moov = child(mp4, b"moov")?;
    let mut tracks = Vec::new();
    for (tag, bytes) in children(moov)? {
        if tag == *b"trak" {
            if let Some(track) = parse_track(mp4, bytes)? {
                tracks.push(track);
            }
        }
    }
    if tracks.len() != 2 || tracks.iter().filter(|t| t.video).count() != 1 {
        return Err("expected one AVC track and one AAC track");
    }
    let mut events = Vec::new();
    for (track_idx, track) in tracks.iter().enumerate() {
        for sample in &track.samples {
            let to_clock = |t: i64| -> i64 { (t * 90_000) / i64::from(track.scale) };
            let start = segment_ms * 90 + 90_000;
            let dts = start + to_clock(sample.dts);
            let pts = start + to_clock(sample.pts);
            if dts < 0 || pts < 0 {
                return Err("negative transport timestamp");
            }
            let payload = &mp4[sample.offset..sample.offset + sample.size];
            let bytes = if track.video {
                annexb(payload, track, sample.key)?
            } else {
                adts(payload, track)?
            };
            events.push((dts as u64, pts as u64, track_idx, track.video, bytes));
        }
    }
    events.sort_by_key(|event| event.0);
    let mut output = Vec::new();
    psi(&mut output, &mut state.cc);
    let mut pcr_written = false;
    for (dts, pts, _track, video, bytes) in events {
        let pcr = video && !pcr_written;
        if pcr {
            pcr_written = true;
        }
        pes(&mut output, &mut state.cc, video, pts, dts, &bytes, pcr);
    }
    if !pcr_written {
        return Err("segment contains no video");
    }
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tables_and_crc() {
        let mut bytes = Vec::new();
        psi(&mut bytes, &mut [0; 4]);
        assert_eq!(bytes.len(), 376);
        assert_eq!(bytes[0], 0x47);
        assert_eq!(bytes[188], 0x47);
        assert_eq!(crc32_mpeg(&[0]), 0x4e08bfb4);
    }
    #[test]
    fn timestamp_marker_bits() {
        for v in [0, 90000, 9_000_000, (1 << 33) - 1] {
            let b = stamp(v, 2);
            assert_eq!(b[0] & 1, 1);
            assert_eq!(b[2] & 1, 1);
            assert_eq!(b[4] & 1, 1);
        }
    }
}
