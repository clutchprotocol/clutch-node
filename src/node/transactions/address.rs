//! Shared address string normalization for state comparisons.

/// Normalize address strings for comparison (handles `0x` / `0X` prefix and casing).
pub fn normalize_address_for_compare(addr: &str) -> String {
    let t = addr.trim();
    let hex_part = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    format!("0x{}", hex_part.to_ascii_lowercase())
}
