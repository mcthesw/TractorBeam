use super::{MAX_ENCODED_FRAME_LEN, ProtocolError, WireMessage};

pub fn encode<T: WireMessage>(message: &T) -> Result<Vec<u8>, ProtocolError> {
    message.validate_message()?;
    let encoded = postcard::to_stdvec_cobs(message)?;
    if encoded.len() > MAX_ENCODED_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge {
            actual: encoded.len(),
            maximum: MAX_ENCODED_FRAME_LEN,
        });
    }
    Ok(encoded)
}

pub fn decode<T: WireMessage>(frame: &mut [u8]) -> Result<T, ProtocolError> {
    if frame.len() > MAX_ENCODED_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge {
            actual: frame.len(),
            maximum: MAX_ENCODED_FRAME_LEN,
        });
    }
    let message: T = postcard::from_bytes_cobs(frame)?;
    message.validate_message()?;
    Ok(message)
}

#[derive(Debug, Default)]
pub struct FrameDecoder {
    frame: Vec<u8>,
}

impl FrameDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            frame: Vec::with_capacity(4_096),
        }
    }

    pub fn push<T: WireMessage>(&mut self, input: &[u8]) -> Result<Vec<T>, ProtocolError> {
        let mut messages = Vec::new();
        for &byte in input {
            self.frame.push(byte);
            if self.frame.len() > MAX_ENCODED_FRAME_LEN {
                self.frame.clear();
                return Err(ProtocolError::FrameTooLarge {
                    actual: MAX_ENCODED_FRAME_LEN.saturating_add(1),
                    maximum: MAX_ENCODED_FRAME_LEN,
                });
            }
            if byte == 0 {
                let decoded = decode(&mut self.frame);
                self.frame.clear();
                messages.push(decoded?);
            }
        }
        Ok(messages)
    }

    pub fn finish(&self) -> Result<(), ProtocolError> {
        if self.frame.is_empty() {
            Ok(())
        } else {
            Err(ProtocolError::TruncatedFrame)
        }
    }
}
