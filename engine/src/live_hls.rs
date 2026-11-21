//! Experimental on-demand HLS access for RTMP Telegram livestreams.
//! Produces MPEG-TS from the self-contained 1-second MP4 pieces; never transcodes.
//! Player fetches a short sliding playlist and sequential TS segments.
//! Every media URL is authorized with the same local bearer token.
use crate::*;
use axum::extract::Path;
use axum::http::HeaderValue;
use std::sync::OnceLock;

const MAX_SEGMENTS: usize = 12;
const MAX_BYTES: usize = 24 * 1024 * 1024;
const WINDOW: usize = 5;

struct Cache {
    pieces: HashMap<(String, i64), Vec<u8>>,
    order: std::collections::VecDeque<(String, i64)>,
    size: usize,
}
impl Cache {
    fn new() -> Self {
        Self {
            pieces: HashMap::new(),
            order: std::collections::VecDeque::new(),
            size: 0,
        }
    }
    fn get(&self, id: &str, time: i64) -> Option<Vec<u8>> {
        self.pieces.get(&(id.to_owned(), time)).cloned()
    }
    fn put(&mut self, id: String, time: i64, bytes: Vec<u8>) {
        let key = (id, time);
        if self.pieces.contains_key(&key) {
            return;
        }
        self.size += bytes.len();
        self.order.push_back(key.clone());
        self.pieces.insert(key, bytes);
        while self.order.len() > MAX_SEGMENTS || self.size > MAX_BYTES {
            if let Some(key) = self.order.pop_front()
                && let Some(bytes) = self.pieces.remove(&key)
            {
                self.size -= bytes.len();
            }
        }
    }
}
static CACHE: OnceLock<tokio::sync::Mutex<Cache>> = OnceLock::new();
fn cache() -> &'static tokio::sync::Mutex<Cache> {
    CACHE.get_or_init(|| tokio::sync::Mutex::new(Cache::new()))
}

async fn source(app: &Shared, id: &str) -> ApiResult<(tl::enums::InputGroupCall, i32)> {
    let peer = telegram::peer(app, id).await?;
    let Peer::Channel(_) = peer else {
        return Err(bad("HLS requires a channel"));
    };
    let reference = telegram::peer_ref(app, id).await?;
    let full = app
        .client
        .invoke(&tl::functions::channels::GetFullChannel {
            channel: reference.into(),
        })
        .await?;
    let tl::enums::messages::ChatFull::Full(full) = full;
    let tl::enums::ChatFull::ChannelFull(details) = full.full_chat else {
        return Err(bad("unsupported channel"));
    };
    let call = details.call.ok_or_else(|| bad("no active livestream"))?;
    let group = app
        .client
        .invoke(&tl::functions::phone::GetGroupCall {
            call: call.clone(),
            limit: 0,
        })
        .await?;
    let tl::enums::phone::GroupCall::Call(group) = group;
    let tl::enums::GroupCall::Call(info) = group.call else {
        return Err(bad("livestream ended"));
    };
    if info.schedule_date.is_some() || !info.rtmp_stream {
        return Err(bad("RTMP live stream unavailable"));
    }
    let dc = info
        .stream_dc_id
        .ok_or_else(|| bad("media datacenter unavailable"))?;
    Ok((call, dc))
}

pub async fn playlist(State(app): State<Shared>, Path(id): Path<String>) -> ApiResult<Response> {
    let (call, dc) = source(&app, &id).await?;
    let stream = app
        .pool
        .invoke_in_dc(
            dc,
            &tl::functions::phone::GetGroupCallStreamChannels { call },
        )
        .await?;
    let tl::enums::phone::GroupCallStreamChannels::Channels(stream) = stream;
    let selected = stream
        .channels
        .iter()
        .find_map(|channel| {
            let tl::enums::GroupCallStreamChannel::Channel(ch) = channel;
            (ch.channel == 1).then_some(ch.last_timestamp_ms)
        })
        .ok_or_else(|| bad("unified channel not found"))?;
    // Play 3 seconds behind latest availability; never advertise future chunks.
    let newest = selected.div_euclid(1000) * 1000 - 3000;
    if newest <= (WINDOW as i64) * 1000 {
        return Err(bad("live buffer not ready"));
    }
    let oldest = newest - (WINDOW as i64 - 1) * 1000;
    let token = &app.config.token;
    let mut body = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:{}\n",
        oldest / 1000
    );
    for ts in (oldest..=newest).step_by(1000) {
        body.push_str(&format!("#EXTINF:1.000,\nsegment/{ts}?token={token}\n"));
    }
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/vnd.apple.mpegurl"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-cache, no-store"),
            ),
        ],
        body,
    )
        .into_response())
}

pub async fn segment(
    State(app): State<Shared>,
    Path((id, time)): Path<(String, String)>,
) -> ApiResult<Response> {
    let timestamp: i64 = time.parse().map_err(|_| bad("invalid HLS timestamp"))?;
    if timestamp <= 0 || timestamp % 1000 != 0 {
        return Err(bad("invalid segment timestamp"));
    }
    // Explicitly bounded memory and no persistent cache.
    if let Some(data) = cache().lock().await.get(&id, timestamp) {
        return Ok(([(header::CONTENT_TYPE, "video/mp2t")], data).into_response());
    }
    let (call, dc) = source(&app, &id).await?;
    let stream = app
        .pool
        .invoke_in_dc(
            dc,
            &tl::functions::phone::GetGroupCallStreamChannels { call: call.clone() },
        )
        .await?;
    let tl::enums::phone::GroupCallStreamChannels::Channels(stream) = stream;
    let latest = stream
        .channels
        .iter()
        .find_map(|channel| {
            let tl::enums::GroupCallStreamChannel::Channel(ch) = channel;
            (ch.channel == 1).then_some(ch.last_timestamp_ms)
        })
        .ok_or_else(|| bad("stream channel unavailable"))?;
    if timestamp > latest || timestamp < latest - 15_000 {
        return Err(bad("segment is outside the live window"));
    }
    let request = tl::functions::upload::GetFile {
        precise: false,
        cdn_supported: false,
        location: tl::enums::InputFileLocation::InputGroupCallStream(
            tl::types::InputGroupCallStream {
                call,
                time_ms: timestamp,
                scale: 0,
                video_channel: Some(1),
                video_quality: Some(2),
            },
        ),
        offset: 0,
        limit: 1024 * 1024,
    };
    let response = app.pool.invoke_in_dc(dc, &request).await?;
    let tl::enums::upload::File::File(file) = response else {
        return Err(bad("unexpected Telegram stream response"));
    };
    let mp4 = crate::stream_part::extract_single_mp4(&file.bytes).map_err(bad)?;
    let data =
        crate::hls_ts::remux(mp4, timestamp, &mut crate::hls_ts::Continuity::new()).map_err(bad)?;
    cache().lock().await.put(id, timestamp, data.clone());
    Ok(([(header::CONTENT_TYPE, "video/mp2t")], data).into_response())
}
