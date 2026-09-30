//! Terminal geometry shared by the session and renderer contracts.

use std::num::NonZeroU16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    columns: NonZeroU16,
    rows: NonZeroU16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyDimension;

impl TerminalSize {
    pub fn new(columns: u16, rows: u16) -> Result<Self, EmptyDimension> {
        Ok(Self {
            columns: NonZeroU16::new(columns).ok_or(EmptyDimension)?,
            rows: NonZeroU16::new(rows).ok_or(EmptyDimension)?,
        })
    }

    pub fn columns(self) -> u16 {
        self.columns.get()
    }

    pub fn rows(self) -> u16 {
        self.rows.get()
    }

    pub fn cell_count(self) -> u32 {
        u32::from(self.columns()) * u32::from(self.rows())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_asymmetric_dimensions() {
        let size = TerminalSize::new(97, 31).unwrap();
        assert_eq!(size.columns(), 97);
        assert_eq!(size.rows(), 31);
        assert_eq!(size.cell_count(), 3007);
    }

    #[test]
    fn rejects_either_zero_dimension_but_accepts_one() {
        assert_eq!(TerminalSize::new(0, 31), Err(EmptyDimension));
        assert_eq!(TerminalSize::new(97, 0), Err(EmptyDimension));
        assert_eq!(TerminalSize::new(1, 1).unwrap().cell_count(), 1);
    }

    #[test]
    fn cell_count_does_not_overflow_u16() {
        let size = TerminalSize::new(u16::MAX, u16::MAX).unwrap();
        assert_eq!(size.cell_count(), 4_294_836_225);
    }
}
