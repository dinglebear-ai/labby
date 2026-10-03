use crate::BridgeError;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};

pub(crate) const FRAME_LIMIT: usize = 64 * 1024;

#[derive(Serialize)]
pub(crate) struct Start<'a> {
    pub version: u8,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub target: String,
    pub peer: &'a str,
    #[serde(rename = "derpMapURL")]
    pub derp_map_url: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Event {
    pub version: u8,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub ciphertext: Option<String>,
}

pub(crate) async fn read_event(
    reader: &mut (impl AsyncRead + Unpin),
) -> Result<Event, BridgeError> {
    let mut bytes = Vec::with_capacity(512);
    loop {
        let byte = reader.read_u8().await.map_err(|_| BridgeError::Protocol)?;
        if byte == b'\n' {
            break;
        }
        if bytes.len() == FRAME_LIMIT {
            return Err(BridgeError::Protocol);
        }
        bytes.push(byte);
    }
    let event: Event = serde_json::from_slice(&bytes).map_err(|_| BridgeError::Protocol)?;
    if event.version != 1 {
        return Err(BridgeError::Protocol);
    }
    Ok(event)
}
