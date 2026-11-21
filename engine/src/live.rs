//! Read-only detection of livestreams associated with Telegram channels.
//! No join call, media download, or modification to the Telegram account.
use crate::*;
use axum::extract::Path;

pub async fn status(State(app): State<Shared>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let peer = telegram::peer(&app, &id).await?;
    let Peer::Channel(_) = peer else {
        return Ok(Json(json!({"status":"unsupported"})));
    };
    let reference = telegram::peer_ref(&app, &id).await?;
    let full = app
        .client
        .invoke(&tl::functions::channels::GetFullChannel {
            channel: reference.into(),
        })
        .await?;
    let tl::enums::messages::ChatFull::Full(full) = full;
    let tl::enums::ChatFull::ChannelFull(details) = full.full_chat else {
        return Ok(Json(json!({"status":"unsupported"})));
    };
    let Some(call) = details.call else {
        return Ok(Json(json!({"status":"none"})));
    };

    // Fetch the current state; the channelFull.call field can also point to a scheduled call.
    let group = app
        .client
        .invoke(&tl::functions::phone::GetGroupCall { call, limit: 0 })
        .await?;
    let tl::enums::phone::GroupCall::Call(group) = group;
    let result = match group.call {
        tl::enums::GroupCall::Call(info) => {
            let status = if info.schedule_date.is_some() {
                "scheduled"
            } else {
                "live"
            };
            json!({
                "status":status,
                "participants":info.participants_count,
                "rtmp_stream":info.rtmp_stream,
                "has_stream_dc":info.stream_dc_id.is_some()
            })
        }
        tl::enums::GroupCall::Discarded(_) => json!({"status":"ended"}),
    };
    Ok(Json(result))
}

// Read-only check against the Telegram media data centre.
// Does not join the call or download media. Telegram will normally answer
// GROUPCALL_JOIN_MISSING until a valid spectator session is established.
pub async fn probe(State(app): State<Shared>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let peer = telegram::peer(&app, &id).await?;
    let Peer::Channel(_) = peer else {
        return Ok(Json(json!({"status":"unsupported"})));
    };
    let reference = telegram::peer_ref(&app, &id).await?;
    let full = app
        .client
        .invoke(&tl::functions::channels::GetFullChannel {
            channel: reference.into(),
        })
        .await?;
    let tl::enums::messages::ChatFull::Full(full) = full;
    let tl::enums::ChatFull::ChannelFull(details) = full.full_chat else {
        return Ok(Json(json!({"status":"unsupported"})));
    };
    let Some(call) = details.call else {
        return Ok(Json(json!({"status":"none"})));
    };
    let group = app
        .client
        .invoke(&tl::functions::phone::GetGroupCall {
            call: call.clone(),
            limit: 0,
        })
        .await?;
    let tl::enums::phone::GroupCall::Call(group) = group;
    let tl::enums::GroupCall::Call(info) = group.call else {
        return Ok(Json(json!({"status":"ended"})));
    };
    if info.schedule_date.is_some() {
        return Ok(Json(json!({"status":"scheduled"})));
    }
    if !info.rtmp_stream {
        return Ok(Json(json!({"status":"not_rtmp"})));
    }
    let Some(dc) = info.stream_dc_id else {
        return Ok(Json(json!({"status":"media_dc_unavailable"})));
    };
    let request = tl::functions::phone::GetGroupCallStreamChannels { call };
    match app.pool.invoke_in_dc(dc, &request).await {
        Ok(_) => Ok(Json(json!({
            "status":"channels_accessible",
            "media_dc_id":dc,
            "joined":true
        }))),
        Err(error) => Ok(Json(json!({
            "status":"media_dc_replied",
            "media_dc_id":dc,
            "joined":false,
            "telegram_error":error.to_string()
        }))),
    }
}

#[derive(Deserialize)]
pub struct SegmentProbeOptions {
    // Explicitly cap the diagnostic at one 1-second segment.
    // It does not write anything to disk or start playback.
    #[serde(default)]
    pub video_quality: Option<i32>,
}

pub async fn segment_probe(
    State(app): State<Shared>,
    Path(id): Path<String>,
    Json(options): Json<SegmentProbeOptions>,
) -> ApiResult<Json<Value>> {
    let peer = telegram::peer(&app, &id).await?;
    let Peer::Channel(_) = peer else {
        return Ok(Json(json!({"status":"unsupported"})));
    };
    let reference = telegram::peer_ref(&app, &id).await?;
    let full = app
        .client
        .invoke(&tl::functions::channels::GetFullChannel {
            channel: reference.into(),
        })
        .await?;
    let tl::enums::messages::ChatFull::Full(full) = full;
    let tl::enums::ChatFull::ChannelFull(details) = full.full_chat else {
        return Ok(Json(json!({"status":"unsupported"})));
    };
    let Some(call) = details.call else {
        return Ok(Json(json!({"status":"none"})));
    };
    let group = app
        .client
        .invoke(&tl::functions::phone::GetGroupCall {
            call: call.clone(),
            limit: 0,
        })
        .await?;
    let tl::enums::phone::GroupCall::Call(group) = group;
    let tl::enums::GroupCall::Call(info) = group.call else {
        return Ok(Json(json!({"status":"ended"})));
    };
    if !info.rtmp_stream {
        return Ok(Json(json!({"status":"not_rtmp"})));
    }
    let Some(dc) = info.stream_dc_id else {
        return Ok(Json(json!({"status":"media_dc_unavailable"})));
    };
    let stream = app
        .pool
        .invoke_in_dc(
            dc,
            &tl::functions::phone::GetGroupCallStreamChannels { call: call.clone() },
        )
        .await?;
    let tl::enums::phone::GroupCallStreamChannels::Channels(stream) = stream;
    let channels = stream
        .channels
        .iter()
        .map(|item| {
            let tl::enums::GroupCallStreamChannel::Channel(channel) = item;
            json!({
                "channel":channel.channel,
                "last_timestamp_ms":channel.last_timestamp_ms,
            })
        })
        .collect::<Vec<_>>();
    let Some(selected) = stream.channels.iter().find_map(|item| {
        let tl::enums::GroupCallStreamChannel::Channel(ch) = item;
        (ch.channel == 1).then_some(ch.last_timestamp_ms)
    }) else {
        return Ok(Json(json!({
            "status":"channels_only",
            "media_dc_id":dc,
            "channels":channels,
            "reason":"unified_channel_1_unavailable"
        })));
    };
    let timestamp = selected.div_euclid(1000) * 1000 - 2000;
    if timestamp <= 0 {
        return Ok(Json(json!({
            "status":"channels_only",
            "media_dc_id":dc,
            "channels":channels,
            "reason":"timestamp_unavailable"
        })));
    }
    let quality = options.video_quality.unwrap_or(2).clamp(0, 2);
    let request = tl::functions::upload::GetFile {
        precise: false,
        cdn_supported: false,
        location: tl::enums::InputFileLocation::InputGroupCallStream(tl::types::InputGroupCallStream {
            call,
            time_ms: timestamp,
            scale: 0,
            video_channel: Some(1),
            video_quality: Some(quality),
        }),
        offset: 0,
        limit: 1024 * 1024,
    };
    match app.pool.invoke_in_dc(dc, &request).await {
        Ok(tl::enums::upload::File::File(file)) => {
            let bytes = &file.bytes;
            let head = &bytes[..bytes.len().min(16)];
            Ok(Json(json!({
                "status":"segment_received",
                "media_dc_id":dc,
                "time_ms":timestamp,
                "channel":1,
                "quality":quality,
                "bytes":bytes.len(),
                "first_16_hex":hex::encode(head),
                "channels":channels
            })))
        }
        Ok(_) => Ok(Json(json!({
            "status":"unexpected_file_response",
            "media_dc_id":dc,
            "channels":channels
        }))),
        Err(error) => Ok(Json(json!({
            "status":"segment_error",
            "media_dc_id":dc,
            "time_ms":timestamp,
            "telegram_error":error.to_string(),
            "channels":channels
        }))),
    }
}
