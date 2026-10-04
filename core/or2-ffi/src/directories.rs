//! Pure directory-list projection (API 22). No protocols, queries or persistence here.

/// Current live paths first, then recent history, exact-deduplicated and capped. Rust applies
/// the same literal-path validation used when opening ShellIn; unsafe paths are never sanitized.
#[uniffi::export]
pub fn merge_directory_paths(live: Vec<String>, recent: Vec<String>) -> Vec<String> {
    or2_core::directories::merge(
        live.iter().map(String::as_str),
        recent.iter().map(String::as_str),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_paths_keeps_live_order_and_rejects_unsafe_paths_from_either_source() {
        assert_eq!(
            merge_directory_paths(
                vec!["/live".into(), "/bad\npath".into(), "/live".into()],
                vec!["/live".into(), "/history".into(), "~/relative".into()],
            ),
            ["/live", "/history"]
        );
    }
}
