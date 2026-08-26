use super::port::{PortId, Vlan};
use std::collections::HashMap;
use std::time::{Duration, Instant};

struct Entry {
    port: PortId,
    /// When this entry was last (re)learned — i.e. the last time a frame
    /// with this MAC as its *source* arrived, not the last time it was
    /// looked up as a destination. Matches real switch behavior: a host
    /// that's gone quiet ages out even if other hosts keep trying to
    /// reach it.
    last_seen: Instant,
}

/// Learned MAC -> port mappings, scoped per VLAN so learning in one VLAN can
/// never resolve a lookup in another — that scoping *is* the isolation
/// guarantee, not an add-on check elsewhere.
#[derive(Default)]
pub(crate) struct MacTable {
    entries: HashMap<(Vlan, [u8; 6]), Entry>,
}

impl MacTable {
    /// `now` is supplied by the caller rather than read internally, so
    /// aging stays unit-testable without real time passing — advance a
    /// fake clock in a test the same way `Switch::forward` takes a frame
    /// instead of reading a socket itself.
    pub(crate) fn learn(&mut self, vlan: Vlan, mac: [u8; 6], port: PortId, now: Instant) {
        self.entries.insert(
            (vlan, mac),
            Entry {
                port,
                last_seen: now,
            },
        );
    }

    pub(crate) fn lookup(&self, vlan: Vlan, mac: [u8; 6]) -> Option<PortId> {
        self.entries.get(&(vlan, mac)).map(|e| e.port)
    }

    /// Purges every entry learned against `port`, regardless of VLAN.
    pub(crate) fn remove_port(&mut self, port: PortId) {
        self.entries.retain(|_, e| e.port != port);
    }

    /// Evicts every entry not relearned within `max_age` of `now`. Returns
    /// how many were evicted.
    pub(crate) fn evict_older_than(&mut self, max_age: Duration, now: Instant) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|_, e| now.saturating_duration_since(e.last_seen) < max_age);
        before - self.entries.len()
    }
}
