// Private local-socket framing and bounded-write implementation.
// Textual inclusion retains the SDK\x27s existing private implementation scope.

#[derive(Default)]
struct FrameReader {
    bytes: Vec<u8>,
}

enum WireFrame {
    Json(Vec<u8>),
    Binary,
}

impl FrameReader {
    fn read_json(&mut self, stream: &mut LocalSocketStream) -> Result<WireHostMessage> {
        loop {
            if let Some(frame) = self.take_complete_frame()? {
                return decode_json(frame);
            }
            let mut received = [0_u8; 8192];
            match stream.read(&mut received) {
                Ok(0) => {
                    if empty_read_is_disconnected(stream)? {
                        return Err(Error::Protocol("the CLP transport disconnected".to_owned()));
                    }
                    thread::sleep(IO_RETRY_INTERVAL);
                }
                Ok(count) => self.bytes.extend_from_slice(&received[..count]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if read_disconnected(&error) => {
                    return Err(Error::Protocol("the CLP transport disconnected".to_owned()));
                }
                Err(error) if read_would_block(&error) => thread::sleep(IO_RETRY_INTERVAL),
                Err(error) => return Err(Error::Io(error)),
            }
        }
    }

    fn try_read_json(&mut self, stream: &mut LocalSocketStream) -> Result<Option<WireHostMessage>> {
        loop {
            if let Some(frame) = self.take_complete_frame()? {
                return decode_json(frame).map(Some);
            }
            let mut received = [0_u8; 8192];
            match stream.read(&mut received) {
                Ok(0) => {
                    if empty_read_is_disconnected(stream)? {
                        return Err(Error::Protocol("the CLP transport disconnected".to_owned()));
                    }
                    return Ok(None);
                }
                Ok(count) => self.bytes.extend_from_slice(&received[..count]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if read_disconnected(&error) => {
                    return Err(Error::Protocol("the CLP transport disconnected".to_owned()));
                }
                Err(error) if read_would_block(&error) => return Ok(None),
                Err(error) => return Err(Error::Io(error)),
            }
        }
    }

    fn take_complete_frame(&mut self) -> Result<Option<WireFrame>> {
        if self.bytes.len() < 4 {
            return Ok(None);
        }
        let length = usize::try_from(u32::from_be_bytes(
            self.bytes[..4].try_into().expect("frame length is present"),
        ))
        .map_err(|_| Error::Protocol("frame length does not fit this platform".to_owned()))?;
        if length > MAX_FRAME_BYTES {
            return Err(Error::Protocol(format!(
                "frame length {length} exceeds {MAX_FRAME_BYTES}"
            )));
        }
        if length == 0 {
            return Err(Error::Protocol(
                "frame is missing its kind discriminator".to_owned(),
            ));
        }
        let frame_end = 4_usize
            .checked_add(length)
            .ok_or_else(|| Error::Protocol("frame length overflows this platform".to_owned()))?;
        if self.bytes.len() < frame_end {
            return Ok(None);
        }
        let frame = match self.bytes[4] {
            b'J' => WireFrame::Json(self.bytes[5..frame_end].to_vec()),
            b'B' => WireFrame::Binary,
            _ => Err(Error::Protocol(
                "frame has an unknown kind discriminator".to_owned(),
            ))?,
        };
        self.bytes.drain(..frame_end);
        Ok(Some(frame))
    }
}

fn read_json(stream: &mut LocalSocketStream, reader: &mut FrameReader) -> Result<WireHostMessage> {
    reader.read_json(stream)
}

fn decode_json(frame: WireFrame) -> Result<WireHostMessage> {
    let WireFrame::Json(payload) = frame else {
        return Err(Error::Protocol(
            "expected a JSON frame from the host".to_owned(),
        ));
    };
    Ok(serde_json::from_slice(&payload)?)
}

fn write_json(stream: &mut LocalSocketStream, message: &impl Serialize) -> Result<()> {
    write_frame(stream, b'J', &serde_json::to_vec(message)?)
}

fn write_frame(stream: &mut LocalSocketStream, kind: u8, payload: &[u8]) -> Result<()> {
    let length = payload
        .len()
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("frame length overflow".to_owned()))?;
    if length > MAX_FRAME_BYTES {
        return Err(Error::InvalidInput(format!(
            "frame length {length} exceeds {MAX_FRAME_BYTES}"
        )));
    }
    let length = u32::try_from(length)
        .map_err(|_| Error::InvalidInput("frame length exceeds u32".to_owned()))?;
    write_all_resilient(stream, &length.to_be_bytes())?;
    write_all_resilient(stream, &[kind])?;
    write_all_resilient(stream, payload)
}

fn write_all_resilient(stream: &mut LocalSocketStream, mut bytes: &[u8]) -> Result<()> {
    let deadline = Instant::now() + IO_STALL_TIMEOUT;
    while !bytes.is_empty() {
        match stream.write(bytes) {
            Ok(0) => wait_for_write_capacity(deadline)?,
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if read_would_block(&error) => wait_for_write_capacity(deadline)?,
            Err(error) => return Err(Error::Io(error)),
        }
    }
    Ok(())
}

fn wait_for_write_capacity(deadline: Instant) -> Result<()> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "the CLP peer stopped reading",
            ))
        })?;
    thread::sleep(remaining.min(IO_RETRY_INTERVAL));
    Ok(())
}

#[cfg(target_os = "windows")]
fn empty_read_is_disconnected(stream: &LocalSocketStream) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::{
        Foundation::{ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED},
        System::Pipes::PeekNamedPipe,
    };

    let LocalSocketStream::NamedPipe(pipe) = stream;
    // SAFETY: `pipe` owns a live named-pipe handle for the duration of this
    // call. A zero-length peek permits null optional output pointers and does
    // not read or write a caller-provided buffer.
    let available = unsafe {
        PeekNamedPipe(
            pipe.inner().as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if available != 0 {
        return Ok(false);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(code)
            if code == ERROR_BROKEN_PIPE as i32 || code == ERROR_PIPE_NOT_CONNECTED as i32 =>
        {
            Ok(true)
        }
        _ => Err(error),
    }
}

#[cfg(not(target_os = "windows"))]
fn empty_read_is_disconnected(_: &LocalSocketStream) -> io::Result<bool> {
    Ok(true)
}

fn read_disconnected(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset | io::ErrorKind::UnexpectedEof
    )
}

fn read_would_block(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        error.raw_os_error() == Some(232)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

