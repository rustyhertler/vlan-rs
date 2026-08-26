/// An 802.1Q tag: TPID is implied (it's what tells `EthernetFrame::parse`
/// a tag is present at all), so only the TCI fields live here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dot1qTag {
    /// Priority code point, 3 bits (0..=7).
    pub pcp: u8,
    /// Drop eligible indicator, 1 bit.
    pub dei: bool,
    /// VLAN identifier, 12 bits. 0 means "priority-tagged, no VLAN"; 4095 is reserved.
    pub vid: u16,
}

impl Dot1qTag {
    pub(crate) fn from_tci(tci: u16) -> Self {
        Dot1qTag {
            pcp: ((tci >> 13) & 0b111) as u8,
            dei: tci & 0x1000 != 0,
            vid: tci & 0x0FFF,
        }
    }

    // Fields are wire-width already (pcp: u8 can exceed 3 bits, vid: u16 can exceed
    // 12), so out-of-range values are masked here rather than rejected — validation
    // belongs at construction time, not at the point where bits hit the wire.
    pub(crate) fn to_tci(self) -> u16 {
        (u16::from(self.pcp & 0b111) << 13) | (u16::from(self.dei) << 12) | (self.vid & 0x0FFF)
    }
}
