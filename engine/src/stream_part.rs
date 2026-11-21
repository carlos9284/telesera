//! Inspector for Telegram's proprietary livestream segment wrapper.
//! The bytes after this header contain one or more media portions in the
//! advertised container format. This parser does not decode the video.
use serde_json::{Value, json};

struct Reader<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn u32(&mut self) -> Result<u32, &'static str> {
        let bytes = self
            .input
            .get(self.offset..self.offset + 4)
            .ok_or("short integer")?;
        self.offset += 4;
        Ok(u32::from_le_bytes(
            bytes.try_into().map_err(|_| "invalid integer")?,
        ))
    }

    fn tl_string(&mut self) -> Result<String, &'static str> {
        let prefix = *self.input.get(self.offset).ok_or("short TL string")?;
        self.offset += 1;
        let (len, padding) = if prefix == 254 {
            let b = self
                .input
                .get(self.offset..self.offset + 3)
                .ok_or("short TL string length")?;
            self.offset += 3;
            let len = usize::from(b[0]) | (usize::from(b[1]) << 8) | (usize::from(b[2]) << 16);
            (len, (4 - len % 4) % 4)
        } else if prefix < 254 {
            let len = usize::from(prefix);
            (len, (4 - ((len + 1) % 4)) % 4)
        } else {
            return Err("invalid TL string prefix");
        };
        let end = self
            .offset
            .checked_add(len)
            .ok_or("string length overflow")?;
        let value = self
            .input
            .get(self.offset..end)
            .ok_or("short TL string bytes")?;
        let value = std::str::from_utf8(value)
            .map_err(|_| "non-UTF8 TL string")?
            .to_owned();
        self.offset = end.checked_add(padding).ok_or("padding overflow")?;
        if self.offset > self.input.len() {
            return Err("short TL string padding");
        }
        Ok(value)
    }
}

pub fn inspect(bytes: &[u8]) -> Result<Value, &'static str> {
    let mut r = Reader {
        input: bytes,
        offset: 0,
    };
    if r.u32()? != 0xa12e810d {
        return Err("unrecognized livestream segment magic");
    }
    let container = r.tl_string()?;
    let active_mask = r.u32()?;
    let event_count = r.u32()?;
    if !(1..=16).contains(&event_count) {
        return Err("unexpected event count");
    }
    let mut events = Vec::new();
    for _ in 0..event_count {
        let offset = r.u32()? as usize;
        let _endpoint_id = r.tl_string()?;
        let rotation = r.u32()?;
        let extra = r.u32()?;
        events.push((offset, rotation, extra));
    }
    let payload = bytes.get(r.offset..).ok_or("missing media payload")?;
    let mut parts = Vec::new();
    for (index, &(start, rotation, extra)) in events.iter().enumerate() {
        let end = events
            .get(index + 1)
            .map(|event| event.0)
            .unwrap_or(payload.len());
        if end <= start || end > payload.len() {
            return Err("invalid media part offsets");
        }
        let section = &payload[start..end];
        parts.push(json!({
            "bytes":section.len(),
            "offset":start,
            "rotation":rotation,
            "extra":extra,
            "first_16_hex":hex::encode(&section[..section.len().min(16)]),
        }));
    }
    Ok(json!({
        "wrapper":"telegram-video-stream-part",
        "container":container,
        "active_mask":active_mask,
        "header_bytes":r.offset,
        "payload_bytes":payload.len(),
        "event_count":event_count,
        "parts":parts,
    }))
}
