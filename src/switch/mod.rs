mod error;
mod forwarding;
mod mac_table;
mod port;

pub use error::SwitchError;
pub use forwarding::{Forward, Switch};
pub use port::{PortId, Vlan};
