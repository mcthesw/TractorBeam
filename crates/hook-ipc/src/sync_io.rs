use std::{
    io::{self, Read, Write},
    thread,
    time::{Duration, Instant},
};

use super::{FrameDecoder, WireMessage, encode};

pub fn write_message<W, T>(
    writer: &mut W,
    message: &T,
    timeout: Duration,
    poll_interval: Duration,
) -> io::Result<()>
where
    W: Write,
    T: WireMessage,
{
    let encoded = encode(message).map_err(protocol_io)?;
    write_all_bounded(writer, &encoded, timeout, poll_interval)
}

pub fn write_all_bounded(
    writer: &mut impl Write,
    bytes: &[u8],
    timeout: Duration,
    poll_interval: Duration,
) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut written = 0;
    while written < bytes.len() {
        match writer.write(&bytes[written..]) {
            #[cfg(windows)]
            Ok(0) if Instant::now() < deadline => thread::sleep(poll_interval),
            #[cfg(windows)]
            Ok(0) => return Err(write_timeout()),
            #[cfg(not(windows))]
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "local IPC stream stopped accepting bytes",
                ));
            }
            Ok(size) => written += size,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if is_transient(&error) && Instant::now() < deadline => {
                thread::sleep(poll_interval);
            }
            Err(error) if is_transient(&error) => return Err(write_timeout()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub fn read_messages<R, T>(reader: &mut R, decoder: &mut FrameDecoder) -> io::Result<Vec<T>>
where
    R: Read,
    T: WireMessage,
{
    let mut buffer = [0_u8; 4_096];
    match reader.read(&mut buffer) {
        #[cfg(windows)]
        Ok(0) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "local IPC named pipe has no bytes available",
        )),
        #[cfg(not(windows))]
        Ok(0) => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "local IPC peer disconnected",
        )),
        Ok(size) => decoder.push(&buffer[..size]).map_err(protocol_io),
        Err(error) => Err(error),
    }
}

#[must_use]
pub fn is_transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

pub fn protocol_io(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[must_use]
pub fn write_timeout() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "local IPC write timed out")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    struct ZeroThenWrite {
        returned_zero: bool,
        bytes: Vec<u8>,
    }

    impl Write for ZeroThenWrite {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if !self.returned_zero {
                self.returned_zero = true;
                return Ok(0);
            }
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn retries_zero_progress_windows_named_pipe_write() {
        let mut writer = ZeroThenWrite {
            returned_zero: false,
            bytes: Vec::new(),
        };

        write_all_bounded(
            &mut writer,
            b"game-packet",
            Duration::from_millis(250),
            Duration::from_millis(1),
        )
        .unwrap();

        assert!(writer.returned_zero);
        assert_eq!(writer.bytes, b"game-packet");
    }
}
