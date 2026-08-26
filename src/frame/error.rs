use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ParseError {
    /// Fewer than 14 bytes: not enough for dst + src + EtherType/TPID.
    #[error("frame too short for an Ethernet header: {len} bytes")]
    TooShort { len: usize },
    /// TPID was 0x8100 but fewer than 18 bytes: not enough for TCI + `EtherType`.
    #[error("802.1Q TPID present but frame too short for TCI + EtherType: {len} bytes")]
    TruncatedTag { len: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum WriteError {
    /// `tag: None` with `ethertype: 0x8100` would be indistinguishable from a
    /// tagged frame on the wire — parsing it back would misread it as tagged.
    #[error(
        "untagged frame can't use EtherType 0x8100 (802.1Q TPID) — parse() would misread it as tagged"
    )]
    AmbiguousUntaggedEtherType,
}
