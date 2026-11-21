//! Experimental on-demand HLS access for RTMP Telegram livestreams.
//! Groups self-contained 1-second MP4 pieces into longer MPEG-TS HLS segments.
//! Uses a small sliding playlist and does not transcode.
//! Every media URL is authorized with the same local bearer token.
use crate::*;
use axum::extract::Path;
use axum::http::HeaderValue;
use std::sync::OnceLock;

const MAX_SEGMENTS: usize = 12;
const MAX_BYTES: usize = 24 * 1024 * 1024;
const WINDOW: usize = 3;
const PART_MS: i64 = 1000;
const SEGMENT_MS: i64 = 3000;

struct Cache {
    pieces: HashMap<(String, i64), Vec<u8>>,
    order: std::collections::VecDeque<(String, i64)>,
    size: usize,
    continuation: HashMap<String, (i64, crate::hls_ts::Continuity)>,
}
impl Cache {
    fn new() -> Self {
        Self {
            pieces: HashMap::new(),
            order: std::collections::VecDeque::new(),
            size: 0,
            continuation: HashMap::new(),
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
static BUILD_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
fn build_lock() -> &'static tokio::sync::Mutex<()> {
    BUILD_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}
static JOINED: OnceLock<tokio::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();
fn joined() -> &'static tokio::sync::Mutex<std::collections::HashSet<String>> {
    JOINED.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashSet::new()))
}

/// Only one join per channel and engine instance. A request failing with
/// GROUPCALL_JOIN_MISSING explicitly invalidates this optimistic state.
async fn ensure_join(app: &Shared, id: &str, call: tl::enums::InputGroupCall) -> ApiResult<()> {
    let mut guard = joined().lock().await;
    if guard.contains(id) {
        return Ok(());
    }
    crate::live_join::join_spectator(app, call).await?;
    guard.insert(id.to_owned());
    Ok(())
}

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
    ensure_join(&app, &id, call.clone()).await?;
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
    // Advertise only complete 3-second segments that are safely behind live.
    // Keep the advertised window inside Telegram's short media retention buffer.
    let newest = (selected - 4000).div_euclid(SEGMENT_MS) * SEGMENT_MS;
    if newest <= (WINDOW as i64) * SEGMENT_MS {
        return Err(bad("live buffer not ready"));
    }
    let oldest = newest - (WINDOW as i64 - 1) * SEGMENT_MS;
    let token = &app.config.token;
    let mut body = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:3\n#EXT-X-MEDIA-SEQUENCE:{}\n",
        oldest / SEGMENT_MS
    );
    for ts in (oldest..=newest).step_by(SEGMENT_MS as usize) {
        body.push_str(&format!("#EXTINF:3.000,\nsegment/{ts}?token={token}\n"));
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
    if timestamp <= 0 || timestamp % SEGMENT_MS != 0 {
        return Err(bad("invalid segment timestamp"));
    }
    // Serialize just the on-demand HLS builder: browsers may fetch segments
    // concurrently but the TS continuity counters must follow request order.
    let _build_guard = build_lock().lock().await;
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
    if timestamp + SEGMENT_MS > latest || timestamp < latest - 30_000 {
        return Err(bad("segment is outside the live window"));
    }
    let mut continuity = cache()
        .lock()
        .await
        .continuation
        .get(&id)
        .and_then(|(next, state)| (*next == timestamp).then(|| state.clone()))
        .unwrap_or_else(crate::hls_ts::Continuity::new);
    let mut data = Vec::new();
    for index in 0..(SEGMENT_MS / PART_MS) {
        let part_time = timestamp + index * PART_MS;
        let request = tl::functions::upload::GetFile {
            precise: false,
            cdn_supported: false,
            location: tl::enums::InputFileLocation::InputGroupCallStream(
                tl::types::InputGroupCallStream {
                    call: call.clone(),
                    time_ms: part_time,
                    scale: 0,
                    video_channel: Some(1),
                    video_quality: Some(2),
                },
            ),
            offset: 0,
            limit: 1024 * 1024,
        };
        let response = match app.pool.invoke_in_dc(dc, &request).await {
            Ok(file) => file,
            Err(error) if error.to_string().contains("GROUPCALL_JOIN_MISSING") => {
                joined().lock().await.remove(&id);
                ensure_join(&app, &id, call.clone()).await?;
                app.pool.invoke_in_dc(dc, &request).await?
            }
            Err(error) => return Err(error.into()),
        };
        let tl::enums::upload::File::File(file) = response else {
            return Err(bad("unexpected Telegram stream response"));
        };
        let mp4 = crate::stream_part::extract_single_mp4(&file.bytes).map_err(bad)?;
        let piece = crate::hls_ts::remux(mp4, part_time, &mut continuity).map_err(bad)?;
        data.extend_from_slice(&piece);
    }
    let mut guard = cache().lock().await;
    guard.continuation.insert(id.clone(), (timestamp + SEGMENT_MS, continuity));
    guard.put(id, timestamp, data.clone());
    Ok(([(header::CONTENT_TYPE, "video/mp2t")], data).into_response())
}
