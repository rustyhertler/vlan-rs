use super::counters::Counters;
use super::error::SwitchError;
use super::loop_guard;
use super::mac_table::MacTable;
use super::port::{PortId, PortMode, Vlan};
use crate::frame::{Dot1qTag, EthernetFrame};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const BROADCAST: [u8; 6] = [0xFF; 6];

/// The I/G bit is the least-significant bit of a MAC address's first octet.
/// Set, it marks a multicast/broadcast (group) address — never valid as a
/// frame's source, so a forged one must not be learned.
fn is_group_address(mac: [u8; 6]) -> bool {
    mac[0] & 0x01 != 0
}

/// A frame ready to hand to `port`'s writer, already encoded exactly as
/// that port's mode requires — tagged for a trunk carrying a non-native
/// VLAN, untagged for an access port or a trunk's native VLAN.
#[derive(Debug, PartialEq, Eq)]
pub struct Delivery {
    pub port: PortId,
    pub bytes: Vec<u8>,
}

/// A port's VLAN membership plus whether the loop guard has shut it down.
/// Deliberately not folded into `PortMode` itself — VLAN membership and
/// "is this port currently allowed to pass traffic" are orthogonal, and
/// blocking would otherwise need to remember the prior mode to restore it.
struct PortEntry {
    mode: PortMode,
    blocked: bool,
}

/// The switch core: per-VLAN MAC learning, access *and* trunk ports, a
/// lightweight loop guard, zero I/O. Ports are just handles the caller
/// assigns meaning to — actually moving bytes per the `Delivery` list
/// `forward` returns is the caller's job.
pub struct Switch {
    ports: HashMap<PortId, PortEntry>,
    mac_table: MacTable,
    port_counters: HashMap<PortId, Counters>,
    vlan_counters: HashMap<Vlan, Counters>,
    probe_id: u64,
}

impl Default for Switch {
    fn default() -> Self {
        Self::new()
    }
}

impl Switch {
    #[must_use]
    pub fn new() -> Self {
        Self::with_probe_id(loop_guard::random_probe_id())
    }

    /// Like [`Switch::new`], but with an explicit loop-guard probe id
    /// instead of a random one — for tests that need to construct a probe
    /// frame and know in advance whether `forward` will recognize it as
    /// this switch's own.
    #[must_use]
    pub fn with_probe_id(probe_id: u64) -> Self {
        Switch {
            ports: HashMap::new(),
            mac_table: MacTable::default(),
            port_counters: HashMap::new(),
            vlan_counters: HashMap::new(),
            probe_id,
        }
    }

    /// Registers `port` in `mode`, unblocked. Calling this again for a
    /// `port` that's already registered (e.g. to change its mode) purges
    /// any MAC-table entries learned against it, so a stale route can't
    /// leak traffic into whatever VLANs its new mode carries, and clears
    /// any loop-guard block — a reconfigured port starts clean.
    pub fn add_port(&mut self, port: PortId, mode: PortMode) {
        self.mac_table.remove_port(port);
        self.ports.insert(
            port,
            PortEntry {
                mode,
                blocked: false,
            },
        );
    }

    /// Deregisters `port` and purges its learned MAC-table entries and
    /// counters. A later `forward` call using this `port` as ingress
    /// returns `UnknownPort`, and it drops out of every VLAN's flood set.
    pub fn remove_port(&mut self, port: PortId) {
        self.ports.remove(&port);
        self.mac_table.remove_port(port);
        self.port_counters.remove(&port);
    }

    /// Evicts every learned MAC entry not relearned within `max_age` of
    /// `now` — real switches do this so a host that's moved to a different
    /// port, or gone away, doesn't leave a stale route behind. Returns how
    /// many entries were evicted.
    pub fn age_out(&mut self, max_age: Duration, now: Instant) -> usize {
        self.mac_table.evict_older_than(max_age, now)
    }

    /// This switch instance's loop-guard probe identity. The caller is
    /// expected to broadcast [`Switch::build_loop_probe`]'s bytes out
    /// every port periodically; if one ever comes back on a port,
    /// `forward` recognizes it (by this same id) and blocks that port.
    #[must_use]
    pub fn probe_id(&self) -> u64 {
        self.probe_id
    }

    /// Builds this switch's loop-guard probe frame, ready to send
    /// unmodified out every port regardless of VLAN/trunk mode — like a
    /// real switch's BPDUs, probes bypass VLAN/tag processing entirely
    /// (see `forward`'s doc comment) specifically so they aren't rejected
    /// by, say, a trunk with no native VLAN.
    #[must_use]
    pub fn build_loop_probe(&self) -> Vec<u8> {
        loop_guard::build_probe(self.probe_id)
    }

    /// Whether the loop guard has shut `port` down. A blocked port's
    /// `forward` calls fail with `SwitchError::PortBlocked`, and it's
    /// excluded from every VLAN's flood set — but it still stays
    /// registered, and still processes incoming loop-guard probes (should
    /// a future version add automatic recovery once a loop clears).
    #[must_use]
    pub fn is_blocked(&self, port: PortId) -> bool {
        self.ports.get(&port).is_some_and(|entry| entry.blocked)
    }

    /// Shuts `port` down: no traffic in or out until [`Switch::unblock_port`]
    /// or a fresh [`Switch::add_port`] call. There's no automatic recovery
    /// — this is a lightweight self-loop guard, not full spanning tree.
    pub fn block_port(&mut self, port: PortId) {
        if let Some(entry) = self.ports.get_mut(&port) {
            entry.blocked = true;
        }
    }

    /// Reverses [`Switch::block_port`].
    pub fn unblock_port(&mut self, port: PortId) {
        if let Some(entry) = self.ports.get_mut(&port) {
            entry.blocked = false;
        }
    }

    /// `port`'s frame/byte counters, or all-zero if `port` isn't registered
    /// or hasn't seen any traffic yet.
    #[must_use]
    pub fn port_counters(&self, port: PortId) -> Counters {
        self.port_counters.get(&port).copied().unwrap_or_default()
    }

    /// Every port with at least one counter update, most-recently-touched
    /// order not guaranteed.
    pub fn all_port_counters(&self) -> impl Iterator<Item = (PortId, Counters)> + '_ {
        self.port_counters.iter().map(|(&p, &c)| (p, c))
    }

    /// `vlan`'s frame/byte counters, or all-zero if it's never been the
    /// resolved VLAN of any frame `forward` has handled.
    #[must_use]
    pub fn vlan_counters(&self, vlan: Vlan) -> Counters {
        self.vlan_counters.get(&vlan).copied().unwrap_or_default()
    }

    /// Every VLAN with at least one counter update.
    pub fn all_vlan_counters(&self) -> impl Iterator<Item = (Vlan, Counters)> + '_ {
        self.vlan_counters.iter().map(|(&v, &c)| (v, c))
    }

    /// Learns `frame`'s source MAC against `ingress`'s resolved VLAN, then
    /// decides where the frame goes — returning it pre-encoded for each
    /// egress port's mode. Empty means drop. `now` timestamps the learn for
    /// [`Switch::age_out`] — supplied by the caller rather than read
    /// internally, so aging stays testable without real time passing.
    ///
    /// A loop-guard probe (see [`Switch::build_loop_probe`]) is recognized
    /// by its `EtherType` alone and handled before any of the above: it's
    /// never VLAN-resolved, learned, counted, or forwarded/flooded, the
    /// same way a real switch's BPDUs bypass normal data-plane rules. If
    /// its payload matches this switch's own probe id, `ingress` gets
    /// [`Switch::block_port`]'ed; either way the call returns `Ok(vec![])`.
    ///
    /// # Errors
    ///
    /// Returns [`SwitchError::UnknownPort`] if `ingress` was never
    /// registered via [`Switch::add_port`], [`SwitchError::PortBlocked`] if
    /// the loop guard has shut `ingress` down, [`SwitchError::TaggedFrameOnAccessPort`]
    /// if a tagged frame arrived on an access port, or a trunk-specific
    /// error if `frame` doesn't resolve to a VLAN that trunk carries — see
    /// [`SwitchError`].
    pub fn forward(
        &mut self,
        ingress: PortId,
        frame: &EthernetFrame,
        now: Instant,
    ) -> Result<Vec<Delivery>, SwitchError> {
        if loop_guard::is_any_probe(frame) {
            return self.handle_loop_probe(ingress, frame);
        }

        let vlan = match self.ingress_vlan(ingress, frame) {
            Ok(vlan) => vlan,
            Err(e) => {
                self.port_counters.entry(ingress).or_default().drops += 1;
                return Err(e);
            }
        };

        let wire_len = frame.wire_len() as u64;
        let port_in = self.port_counters.entry(ingress).or_default();
        port_in.frames_in += 1;
        port_in.bytes_in += wire_len;
        let vlan_in = self.vlan_counters.entry(vlan).or_default();
        vlan_in.frames_in += 1;
        vlan_in.bytes_in += wire_len;

        // A forged multicast/broadcast source is never learned — which, as a
        // side effect, also keeps a later multicast/broadcast *destination*
        // lookup from ever matching a learned unicast entry.
        if !is_group_address(frame.src) {
            self.mac_table.learn(vlan, frame.src, ingress, now);
        }

        let egress_ports = if frame.dst == BROADCAST {
            self.flood_targets(vlan, ingress)
        } else {
            match self.mac_table.lookup(vlan, frame.dst) {
                Some(egress) if egress == ingress => Vec::new(),
                Some(egress) => vec![egress],
                None => self.flood_targets(vlan, ingress),
            }
        };

        let deliveries: Vec<Delivery> = egress_ports
            .into_iter()
            .filter_map(|port| self.encode_for_egress(port, vlan, frame))
            .collect();

        for delivery in &deliveries {
            let len = delivery.bytes.len() as u64;
            let port_out = self.port_counters.entry(delivery.port).or_default();
            port_out.frames_out += 1;
            port_out.bytes_out += len;
            let vlan_out = self.vlan_counters.entry(vlan).or_default();
            vlan_out.frames_out += 1;
            vlan_out.bytes_out += len;
        }

        Ok(deliveries)
    }

    fn handle_loop_probe(
        &mut self,
        ingress: PortId,
        frame: &EthernetFrame,
    ) -> Result<Vec<Delivery>, SwitchError> {
        if !self.ports.contains_key(&ingress) {
            return Err(SwitchError::UnknownPort(ingress));
        }
        if loop_guard::is_own_probe(frame, self.probe_id) {
            self.block_port(ingress);
        }
        Ok(Vec::new())
    }

    /// Resolves the VLAN a frame arriving on `port` belongs to, per that
    /// port's mode. An access port ignores the wire entirely (it's always
    /// that one VLAN) but rejects a tagged frame outright rather than
    /// silently accepting one — a tag there means the two ends disagree
    /// about what kind of link this is.
    fn ingress_vlan(&self, port: PortId, frame: &EthernetFrame) -> Result<Vlan, SwitchError> {
        let entry = self
            .ports
            .get(&port)
            .ok_or(SwitchError::UnknownPort(port))?;
        if entry.blocked {
            return Err(SwitchError::PortBlocked(port));
        }
        match (&entry.mode, &frame.tag) {
            (PortMode::Access { .. }, Some(_)) => {
                Err(SwitchError::TaggedFrameOnAccessPort { port })
            }
            (PortMode::Trunk { native, allowed }, Some(tag))
                if allowed.contains(&tag.vid) || *native == Some(tag.vid) =>
            {
                Ok(tag.vid)
            }
            (PortMode::Trunk { .. }, Some(tag)) => Err(SwitchError::VlanNotAllowedOnTrunk {
                port,
                vlan: tag.vid,
            }),
            (
                PortMode::Access { vlan }
                | PortMode::Trunk {
                    native: Some(vlan), ..
                },
                None,
            ) => Ok(*vlan),
            (PortMode::Trunk { native: None, .. }, None) => {
                Err(SwitchError::UntaggedFrameOnTrunkWithoutNative { port })
            }
        }
    }

    /// Builds the frame `port` should actually receive on the wire for
    /// `vlan`: untagged for an access port or a trunk's native VLAN,
    /// 802.1Q-tagged otherwise. `None` means the frame couldn't be encoded,
    /// and is silently dropped for that one target rather than failing the
    /// whole `forward` call over it — deliberately unlogged, since the
    /// switch core is zero-I/O by design (see the module docs) and both
    /// ways this can actually happen are effectively unreachable today:
    /// `port` not being registered can't happen (every caller of this
    /// method derives `port` from `self.ports` itself), and the
    /// untagged/`EtherType`-0x8100 ambiguity (see
    /// [`crate::frame::WriteError`]) needs a frame whose *inner* `EtherType`
    /// happens to be 0x8100 — only reachable via a QinQ-shaped frame, which
    /// is out of scope (this crate has no `QinQ` support to produce or even
    /// recognize one).
    fn encode_for_egress(
        &self,
        port: PortId,
        vlan: Vlan,
        frame: &EthernetFrame,
    ) -> Option<Delivery> {
        let mode = &self.ports.get(&port)?.mode;
        let tag = match mode {
            PortMode::Access { .. } => None,
            PortMode::Trunk { native, .. } if *native == Some(vlan) => None,
            PortMode::Trunk { .. } => Some(Dot1qTag {
                // Best-effort round-trip of the original priority bits when
                // the frame already carried a tag; a sane default otherwise.
                pcp: frame.tag.map_or(0, |t| t.pcp),
                dei: frame.tag.is_some_and(|t| t.dei),
                vid: vlan,
            }),
        };
        let out = EthernetFrame {
            dst: frame.dst,
            src: frame.src,
            tag,
            ethertype: frame.ethertype,
            payload: frame.payload,
        };
        let mut bytes = Vec::new();
        out.write_into(&mut bytes).ok()?;
        Some(Delivery { port, bytes })
    }

    fn flood_targets(&self, vlan: Vlan, exclude: PortId) -> Vec<PortId> {
        let mut targets: Vec<PortId> = self
            .ports
            .iter()
            .filter_map(|(&id, entry)| {
                (id != exclude && !entry.blocked && entry.mode.carries(vlan)).then_some(id)
            })
            .collect();
        targets.sort_by_key(|p| p.0);
        targets
    }
}
