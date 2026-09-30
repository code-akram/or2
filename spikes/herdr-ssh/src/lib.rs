// SPDX-License-Identifier: GPL-3.0-or-later

/// Minimal ABI proof that the crate can be packaged as an Android cdylib.
#[unsafe(no_mangle)]
pub extern "C" fn or2_herdr_spike_protocol() -> u32 {
    22
}
