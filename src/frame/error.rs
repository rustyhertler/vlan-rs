use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// Fewer than 14 bytes: not enough for dst + src + EtherType/TPID.
    TooShort { len: usize },
    /// TPID was 0x8100 but fewer than 18 bytes: not enough for TCI + EtherType.
    TruncatedTag { len: usize },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::TooShort { len } => {
                write!(f, "frame too short for an Ethernet header: {len} bytes")
            }
            ParseError::TruncatedTag { len } => write!(
                f,
                "802.1Q TPID present but frame too short for TCI + EtherType: {len} bytes"
            ),
        }
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// `tag: None` with `ethertype: 0x8100` would be indistinguishable from a
    /// tagged frame on the wire — parsing it back would misread it as tagged.
    AmbiguousUntaggedEtherType,
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::AmbiguousUntaggedEtherType => write!(
                f,
                "untagged frame can't use EtherType 0x8100 (802.1Q TPID) — parse() would misread it as tagged"
            ),
        }
    }
}

impl std::error::Error for WriteError {}
