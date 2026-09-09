//! Herdr Unix-socket channels backed by the transport contract.
//!
//! Herdr's JSON and bincode protocols use blocking `Read`/`Write` helpers,
//! while `ByteChannel` deliberately exposes non-blocking byte chunks.  This
//! module is the only adapter between those two shapes; Herdr runtime code
//! never opens a Unix socket or starts SSH forwarding directly.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::transport::{ByteChannel, ChannelRequest, TargetConnection};

/// A shared, full-duplex Herdr channel.
pub type SharedChannel = Arc<Mutex<Box<dyn ByteChannel>>>;

/// Open one target-side Unix socket through the transport-owned connection.
pub fn open_unix_socket(connection: &dyn TargetConnection, path: &Path) -> Result<SharedChannel> {
    let channel = connection
        .open_channel(ChannelRequest::UnixSocket {
            path: path.to_path_buf(),
        })
        .with_context(|| {
            format!(
                "open Herdr UnixSocket channel 失败（transport={}, target={}, path={}）",
                connection.transport_id(),
                connection.target(),
                path.display()
            )
        })?;
    Ok(Arc::new(Mutex::new(channel)))
}

/// Shut down a shared channel, preserving the first transport error.
pub fn shutdown(channel: &SharedChannel) -> Result<()> {
    let mut channel = channel
        .lock()
        .map_err(|_| anyhow::anyhow!("Herdr channel lock poisoned"))?;
    channel.shutdown()
}

/// Blocking `Read`/`Write` view of a non-blocking [`SharedChannel`].
pub struct ChannelIo {
    channel: SharedChannel,
    pending: VecDeque<u8>,
    read_timeout: Option<Duration>,
}

impl ChannelIo {
    pub fn new(channel: SharedChannel) -> Self {
        Self {
            channel,
            pending: VecDeque::new(),
            read_timeout: None,
        }
    }

    pub fn with_read_timeout(channel: SharedChannel, timeout: Duration) -> Self {
        Self {
            channel,
            pending: VecDeque::new(),
            read_timeout: Some(timeout),
        }
    }

    /// Stop applying the handshake/API timeout before entering a long-lived
    /// event or pane stream.
    pub fn clear_read_timeout(&mut self) {
        self.read_timeout = None;
    }

    pub fn channel(&self) -> SharedChannel {
        Arc::clone(&self.channel)
    }

    fn read_from_channel(&self) -> io::Result<Option<Vec<u8>>> {
        self.channel
            .lock()
            .map_err(|_| io::Error::other("Herdr channel lock poisoned"))?
            .read()
    }
}

impl Read for ChannelIo {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let deadline = self.read_timeout.map(|timeout| Instant::now() + timeout);
        loop {
            if !self.pending.is_empty() {
                let count = buffer.len().min(self.pending.len());
                for slot in &mut buffer[..count] {
                    *slot = self.pending.pop_front().expect("pending count checked");
                }
                return Ok(count);
            }

            match self.read_from_channel()? {
                Some(data) if !data.is_empty() => {
                    self.pending.extend(data);
                }
                Some(_) | None => {
                    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Herdr channel read timed out",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }
}

impl Write for ChannelIo {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut written = 0;
        while written < buffer.len() {
            let count = self
                .channel
                .lock()
                .map_err(|_| io::Error::other("Herdr channel lock poisoned"))?
                .write(&buffer[written..])?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Herdr channel write returned zero bytes",
                ));
            }
            written += count;
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
