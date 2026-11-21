//! Explicit experimental spectator join for RTMP-mode calls.
//! This does not publish audio or video and is not a WebRTC transport.
//! The local ICE-looking data is only suitable for the stream-only path;
//! normal WebRTC calls require an actual tgcalls engine and certificate.
use crate::*;
use axum::extract::Path;
use std::io::Read;

fn viewer_params() -> ApiResult<(String, i32)> {
    let mut entropy = [0u8; 84];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut entropy)?;
    let ssrc = i32::from_le_bytes(
        entropy[0..4]
            .try_into()
            .map_err(|_| bad("invalid entropy"))?,
    ) & 0x7fff_ffff;
    let ssrc = ssrc.max(1);
    let ufrag = hex::encode(&entropy[4..20]);
    let pwd = hex::encode(&entropy[20..52]);
    let fingerprint = entropy[52..84]
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    let payload = json!({
        "ssrc":ssrc,
        "ufrag":ufrag,
        "pwd":pwd,
        "fingerprints":[{"hash":"sha-256","fingerprint":fingerprint,"setup":"passive"}]
    });
    Ok((payload.to_string(), ssrc))
}

pub async fn join_probe(
    State(app): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let peer = telegram::peer(&app, &id).await?;
    let Peer::Channel(_) = peer else {
        return Err(bad("RTMP spectator join is only available for channels"));
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
        return Err(bad("unsupported channel"));
    };
    let call = details.call.ok_or_else(|| bad("livestream unavailable"))?;
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
    if !info.rtmp_stream || info.schedule_date.is_some() {
        return Err(bad("not an active RTMP livestream"));
    }
    let dc = info.stream_dc_id.ok_or_else(|| bad("media DC missing"))?;
    let (payload, ssrc) = viewer_params()?;
    let result = app
        .client
        .invoke(&tl::functions::phone::JoinGroupCall {
            muted: true,
            video_stopped: true,
            call: call.clone(),
            join_as: tl::enums::InputPeer::Self_,
            invite_hash: None,
            public_key: None,
            block: None,
            params: tl::types::DataJson { data: payload }.into(),
        })
        .await;
    match result {
        Ok(_) => Ok(Json(json!({
            "status":"viewer_join_accepted",
            "source_id":ssrc,
            "media_dc_id":dc,
            "mode":"rtmp_spectator",
            "note":"Join RPC accepted; verify segment access separately"
        }))),
        Err(error) => Ok(Json(json!({
            "status":"viewer_join_rejected",
            "telegram_error":error.to_string(),
            "mode":"rtmp_spectator"
        }))),
    }
}
