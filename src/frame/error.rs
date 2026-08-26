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
