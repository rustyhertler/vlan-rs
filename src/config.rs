use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::switch::{PortMode, PortModeError, Vlan};

#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
enum PortConfigMode {
    Access {
        vlan: Vlan,
    },
    Trunk {
        #[serde(default)]
        native: Option<Vlan>,
        #[serde(default)]
        allowed: Vec<Vlan>,
    },
}

#[derive(Debug, Deserialize)]
struct PortConfig {
    name: String,
    #[serde(flatten)]
    mode: PortConfigMode,
}

/// A switch topology: one entry per port. Deserializes from TOML shaped
/// like:
///
/// ```toml
/// [[port]]
/// name = "tap0"
/// mode = "access"
/// vlan = 10
///
/// [[port]]
/// name = "tap1"
/// mode = "trunk"
/// native = 10
/// allowed = [10, 20]
/// ```
#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(rename = "port", default)]
    ports: Vec<PortConfig>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("parsing TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("port {name:?}: {source}")]
    InvalidPort {
        name: String,
        #[source]
        source: PortModeError,
    },
    #[error("duplicate port name: {0:?}")]
    DuplicateName(String),
}

impl From<ConfigError> for io::Error {
    fn from(e: ConfigError) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, e.to_string())
    }
}

impl Config {
    /// # Errors
    ///
    /// Returns [`ConfigError::Toml`] if `toml_str` isn't valid TOML, or
    /// doesn't match this schema.
    pub fn from_toml_str(toml_str: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(toml_str)?)
    }

    /// Reads and parses `path`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] if `path` can't be read, or
    /// [`ConfigError::Toml`] if its contents don't parse.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&contents)
    }

    /// Validates and converts this config into the same `(name, mode)`
    /// pairs [`crate::daemon::run`] takes regardless of whether they came
    /// from a config file or inline CLI args.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidPort`] if any port's VLAN(s) are out
    /// of range or a trunk has neither a native nor any allowed VLAN, or
    /// [`ConfigError::DuplicateName`] if a port name is repeated.
    pub fn into_specs(self) -> Result<Vec<(String, PortMode)>, ConfigError> {
        let mut seen = HashSet::new();
        let mut specs = Vec::with_capacity(self.ports.len());
        for port in self.ports {
            if !seen.insert(port.name.clone()) {
                return Err(ConfigError::DuplicateName(port.name));
            }
            let mode = match port.mode {
                PortConfigMode::Access { vlan } => PortMode::access(vlan).map_err(Into::into),
                PortConfigMode::Trunk { native, allowed } => PortMode::trunk(native, allowed),
            }
            .map_err(|source| ConfigError::InvalidPort {
                name: port.name.clone(),
                source,
            })?;
            specs.push((port.name, mode));
        }
        Ok(specs)
    }
}
