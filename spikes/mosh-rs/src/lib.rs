use mosh_rs::Base64Key;

/// Checks whether `key` is a printable mosh session key.
///
/// # Safety
///
/// `key` must point to `len` readable bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn or2_mosh_key_is_valid(key: *const u8, len: usize) -> bool {
    if key.is_null() {
        return false;
    }
    // SAFETY: the FFI caller promises `key` points to `len` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(key, len) };
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|value| Base64Key::from_printable(value).ok())
        .is_some()
}
