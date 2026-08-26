use super::port::PortId;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchError {
    /// `forward` was called with a port never registered via `add_port`.
    UnknownPort(PortId),
}

impl fmt::Display for SwitchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SwitchError::UnknownPort(port) => write!(f, "unknown port: {port:?}"),
        }
    }
}

impl std::error::Error for SwitchError {}
