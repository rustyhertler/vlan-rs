#![no_main]

use libfuzzer_sys::fuzz_target;
use vlan_rs::frame::EthernetFrame;

// EthernetFrame::parse takes arbitrary untrusted bytes off the wire by
// design; this feeds it exactly that. Any panic here would take down the
// daemon's entire forwarding-loop task, not just drop one bad frame — see
// daemon.rs's handle_frame_event, which assumes parse() can't panic.
fuzz_target!(|data: &[u8]| {
    if let Ok(frame) = EthernetFrame::parse(data) {
        // A frame that came from parse() should always be re-encodable —
        // write_into only rejects a hand-constructed frame with an
        // impossible tag/EtherType combination, which parse() never
        // produces from real bytes. Exercises write_into for panics too.
        let mut out = Vec::new();
        let _ = frame.write_into(&mut out);
    }
});
