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
