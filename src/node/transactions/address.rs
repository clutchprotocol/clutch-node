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

/// Wire form of an optional referrer for RLP: no `0x` prefix, empty string when absent.
///
/// The SDK RLP-encodes referrers with the prefix stripped, while `optional_canonical_referrer`
/// adds it back on decode. `Transaction::verify_hash` recomputes the hash by re-encoding the
/// decoded transaction, so encoding must strip the prefix again — otherwise the preimage is
/// longer than the bytes the client signed and every referred ride is rejected as a hash
/// mismatch. Mirrors the same `0x` stripping `Transaction::calculate_hash` does for `from`.
pub fn referrer_for_rlp(referrer: &Option<String>) -> String {
    referrer
        .as_deref()
        .map(legacy_account_address_hex)
        .unwrap_or_default()
}
