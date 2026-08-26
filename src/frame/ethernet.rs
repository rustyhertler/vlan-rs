use super::dot1q::Dot1qTag;
use super::error::ParseError;

const TPID_802_1Q: u16 = 0x8100;
const UNTAGGED_HEADER_LEN: usize = 14; // dst(6) + src(6) + ethertype(2)
const TAGGED_HEADER_LEN: usize = 18; // dst(6) + src(6) + TPID(2) + TCI(2) + ethertype(2)

/// A parsed Ethernet II frame, optionally carrying a single 802.1Q tag.
/// QinQ (a second, outer tag) isn't represented — out of scope for phase 1.
#[derive(Debug, PartialEq, Eq)]
pub struct EthernetFrame<'a> {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub tag: Option<Dot1qTag>,
    pub ethertype: u16,
    pub payload: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ParseError> {
        if bytes.len() < UNTAGGED_HEADER_LEN {
            return Err(ParseError::TooShort { len: bytes.len() });
        }

        let mut dst = [0u8; 6];
        let mut src = [0u8; 6];
        dst.copy_from_slice(&bytes[0..6]);
        src.copy_from_slice(&bytes[6..12]);

        let next = u16::from_be_bytes([bytes[12], bytes[13]]);
        if next == TPID_802_1Q {
            if bytes.len() < TAGGED_HEADER_LEN {
                return Err(ParseError::TruncatedTag { len: bytes.len() });
            }
            let tci = u16::from_be_bytes([bytes[14], bytes[15]]);
            let ethertype = u16::from_be_bytes([bytes[16], bytes[17]]);
            Ok(EthernetFrame {
                dst,
                src,
                tag: Some(Dot1qTag::from_tci(tci)),
                ethertype,
                payload: &bytes[TAGGED_HEADER_LEN..],
            })
        } else {
            Ok(EthernetFrame {
                dst,
                src,
                tag: None,
                ethertype: next,
                payload: &bytes[UNTAGGED_HEADER_LEN..],
            })
        }
    }

    pub fn write_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.dst);
        out.extend_from_slice(&self.src);
        if let Some(tag) = self.tag {
            out.extend_from_slice(&TPID_802_1Q.to_be_bytes());
            out.extend_from_slice(&tag.to_tci().to_be_bytes());
        }
        out.extend_from_slice(&self.ethertype.to_be_bytes());
        out.extend_from_slice(self.payload);
    }
}
