use super::{Error, Result, messaging::KernelMessage};
use tokio_tungstenite::tungstenite::Message;

pub(crate) fn encode(message: &KernelMessage) -> Result<Message> {
    let json = serde_json::to_vec(message)?;
    if message.buffers.is_empty() {
        return Ok(Message::Text(
            String::from_utf8(json).expect("JSON is UTF-8").into(),
        ));
    }
    let count = message.buffers.len().checked_add(1).ok_or_else(invalid)?;
    let header_size = count
        .checked_add(1)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(invalid)?;
    let mut offset = header_size;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&u32::try_from(count).map_err(|_| invalid())?.to_be_bytes());
    for part in std::iter::once(&json).chain(message.buffers.iter()) {
        bytes.extend_from_slice(&u32::try_from(offset).map_err(|_| invalid())?.to_be_bytes());
        offset = offset.checked_add(part.len()).ok_or_else(invalid)?;
    }
    u32::try_from(offset).map_err(|_| invalid())?;
    bytes.extend_from_slice(&json);
    for buffer in &message.buffers {
        bytes.extend_from_slice(buffer);
    }
    Ok(Message::Binary(bytes.into()))
}

fn invalid() -> Error {
    Error::Protocol("invalid binary message offsets or length".into())
}

pub(crate) fn decode_binary(bytes: &[u8]) -> Result<KernelMessage> {
    let read = |index: usize| -> Result<usize> {
        let slice = bytes.get(index..index + 4).ok_or_else(invalid)?;
        Ok(u32::from_be_bytes(slice.try_into().map_err(|_| invalid())?) as usize)
    };
    let count = read(0)?;
    let header_size = count
        .checked_add(1)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(invalid)?;
    if count == 0 || header_size > bytes.len() {
        return Err(invalid());
    }
    let mut offsets = Vec::with_capacity(count + 1);
    for i in 0..count {
        offsets.push(read(4 * (i + 1))?);
    }
    offsets.push(bytes.len());
    if offsets[0] != header_size || offsets.windows(2).any(|pair| pair[0] > pair[1]) {
        return Err(invalid());
    }
    let mut message: KernelMessage = serde_json::from_slice(&bytes[offsets[0]..offsets[1]])?;
    message.buffers = offsets[1..]
        .windows(2)
        .map(|pair| bytes[pair[0]..pair[1]].to_vec())
        .collect();
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn binary_buffers_round_trip_including_empty_buffers() {
        let mut msg = KernelMessage::request("session", "shell", "comm_msg", json!({"data":"🪐"}));
        msg.buffers = vec![vec![0, 255, 1], vec![], vec![42]];
        let Message::Binary(bytes) = encode(&msg).unwrap() else {
            panic!("expected binary")
        };
        assert_eq!(&bytes[..8], &[0, 0, 0, 4, 0, 0, 0, 20]);
        let decoded = decode_binary(&bytes).unwrap();
        assert_eq!(decoded.buffers, msg.buffers);
        assert_eq!(decoded.content, msg.content);
        assert_eq!(decoded.header.msg_id, msg.header.msg_id);
    }
    #[test]
    fn rejects_malformed_frames_without_panicking() {
        for frame in [
            vec![],
            vec![0; 4],
            vec![255; 8],
            vec![0, 0, 0, 1, 0, 0, 0, 7],
            vec![0, 0, 0, 2, 0, 0, 0, 12, 0, 0, 0, 11],
            vec![0, 0, 0, 1, 0, 0, 0, 99],
        ] {
            assert!(decode_binary(&frame).is_err());
        }
    }
}
