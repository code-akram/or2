//! The only Kotlin-facing API. Core types are converted here, never exported.

uniffi::setup_scaffolding!();

pub mod frame;
pub mod herdr;
pub mod host;
pub mod keys;
pub mod pair;
pub mod probe;
pub mod session;

#[derive(Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Renderer {
    Canvas,
}

#[derive(Debug, uniffi::Record)]
pub struct BuildInfo {
    pub version: String,
    pub api_version: u32,
    pub minimum_android_sdk: u32,
    pub renderer: Renderer,
}

#[derive(Debug, PartialEq, Eq, uniffi::Record)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
    pub cell_count: u32,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum TerminalError {
    #[error("terminal columns and rows must both be nonzero")]
    EmptyDimension,
}

/// Bumped whenever an exported signature or record changes shape.
pub const API_VERSION: u32 = 17;

#[uniffi::export]
pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        api_version: API_VERSION,
        minimum_android_sdk: 34,
        renderer: Renderer::Canvas,
    }
}

#[uniffi::export]
pub fn terminal_size(columns: u16, rows: u16) -> Result<TerminalSize, TerminalError> {
    let size = or2_core::term::TerminalSize::new(columns, rows)
        .map_err(|_| TerminalError::EmptyDimension)?;
    Ok(TerminalSize {
        columns: size.columns(),
        rows: size.rows(),
        cell_count: size.cell_count(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_core_geometry_without_swapping_axes() {
        assert_eq!(
            terminal_size(97, 31).unwrap(),
            TerminalSize {
                columns: 97,
                rows: 31,
                cell_count: 3007,
            }
        );
    }

    #[test]
    fn maps_core_validation_to_ffi_error() {
        assert!(matches!(
            terminal_size(0, 31),
            Err(TerminalError::EmptyDimension)
        ));
        assert!(matches!(
            terminal_size(97, 0),
            Err(TerminalError::EmptyDimension)
        ));
    }
}
