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

/// Canonical form for account state keys and balances (`0x` + lowercase hex).
pub fn canonical_account_address(addr: &str) -> String {
    normalize_address_for_compare(addr)
}

/// Parse optional referrer from RLP (empty string → None, otherwise canonical `0x` form).
pub fn optional_canonical_referrer(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(canonical_account_address(&s))
    }
}

/// Legacy on-chain referrer keys stored without the `0x` prefix (pre-canonicalization).
pub fn legacy_account_address_hex(canonical: &str) -> String {
    canonical
        .strip_prefix("0x")
        .or_else(|| canonical.strip_prefix("0X"))
        .unwrap_or(canonical)
        .to_string()
}
