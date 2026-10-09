//! The only Kotlin-facing API. Core types are converted here, never exported.

uniffi::setup_scaffolding!();

pub mod directories;
pub mod frame;
pub mod herdr;
pub mod host;
pub mod keys;
pub mod pair;
pub mod probe;
pub mod session;
pub mod wake;

#[derive(Debug, uniffi::Record)]
pub struct BuildInfo {
    pub version: String,
    pub api_version: u32,
}

/// Bumped whenever an exported signature or record changes shape.
pub const API_VERSION: u32 = 23;

#[uniffi::export]
pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        api_version: API_VERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_names_this_build_and_its_api() {
        let info = build_info();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.api_version, API_VERSION);
    }
}
