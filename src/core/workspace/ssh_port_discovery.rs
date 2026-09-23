//! Detect explicitly advertised local web-server URLs in an SSH pane stream.

use std::collections::BTreeSet;

const MAX_LINE_BYTES: usize = 8192;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum EscapeState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

#[derive(Default)]
pub(super) struct SshPortDiscovery {
    line: Vec<u8>,
    escape: EscapeState,
    discovered: BTreeSet<u16>,
    ignored: BTreeSet<u16>,
}

impl SshPortDiscovery {
    pub fn feed(&mut self, data: &[u8]) {
        for &byte in data {
            match self.escape {
                EscapeState::Escape => {
                    self.escape = match byte {
                        b'[' => EscapeState::Csi,
                        b']' => EscapeState::Osc,
                        _ => EscapeState::Ground,
                    };
                }
                EscapeState::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        self.escape = EscapeState::Ground;
                    }
                }
                EscapeState::Osc => {
                    if byte == 0x07 {
                        self.escape = EscapeState::Ground;
                    } else if byte == 0x1b {
                        self.escape = EscapeState::OscEscape;
                    }
                }
                EscapeState::OscEscape => {
                    self.escape = if byte == b'\\' {
                        EscapeState::Ground
                    } else {
                        EscapeState::Osc
                    };
                }
                EscapeState::Ground => match byte {
                    0x1b => self.escape = EscapeState::Escape,
                    b'\n' | b'\r' => self.finish_line(),
                    0x08 | 0x7f => {
                        self.line.pop();
                    }
                    0x20..=0x7e => {
                        if self.line.len() < MAX_LINE_BYTES {
                            self.line.push(byte);
                        }
                    }
                    _ => {}
                },
            }
        }
    }

    pub fn ports(&self) -> impl Iterator<Item = u16> + '_ {
        self.discovered
            .iter()
            .filter(|port| !self.ignored.contains(port))
            .copied()
    }

    pub fn ignore(&mut self, port: u16) -> bool {
        if self.discovered.contains(&port) {
            self.ignored.insert(port);
            true
        } else {
            false
        }
    }

    fn finish_line(&mut self) {
        self.scan_line();
        self.line.clear();
    }

    fn scan_line(&mut self) {
        for port in ports_from_listen_urls(&self.line) {
            if !self.ignored.contains(&port) {
                self.discovered.insert(port);
            }
        }
    }
}

fn ports_from_listen_urls(line: &[u8]) -> Vec<u16> {
    let line = String::from_utf8_lossy(line);
    let bytes = line.as_bytes();
    let mut ports = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let tail = &line[offset..];
        let Some(relative) = [tail.find("http://"), tail.find("https://")]
            .into_iter()
            .flatten()
            .min()
        else {
            break;
        };
        let start = offset + relative;
        let scheme_end = bytes[start..]
            .iter()
            .position(|byte| *byte == b':')
            .map(|n| start + n + 3)
            .unwrap_or(bytes.len());
        let authority_end = bytes[scheme_end..]
            .iter()
            .position(|byte| {
                matches!(
                    *byte,
                    b'/' | b'?' | b'#' | b' ' | b'\t' | b'\r' | b'\n' | b',' | b')' | b']'
                )
            })
            .map(|n| scheme_end + n)
            .unwrap_or(bytes.len());
        if let Some(port) = parse_loopback_authority(&line[scheme_end..authority_end]) {
            ports.push(port);
        }
        offset = authority_end.max(start + 1);
    }
    ports
}

fn parse_loopback_authority(authority: &str) -> Option<u16> {
    let (host, port) = authority.rsplit_once(':')?;
    if !matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "0.0.0.0"
    ) {
        return None;
    }
    let port = port.parse::<u16>().ok()?;
    (port != 0).then_some(port)
}
