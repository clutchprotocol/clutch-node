//! Acceptance tx hashes stored at `ride_offer_*:ride_acceptance` and
//! `ride_request_*:ride_acceptance` were written with `serde_json::to_string(&String)`,
//! which adds JSON quotes around the hex. Reading the raw UTF-8 then surfaced
//! those quotes to GraphQL clients, corrupting RLP payloads (e.g. RidePay).

/// Decode a pointer value: JSON string (`"0xab..."`) or plain hash bytes.
pub fn decode_acceptance_pointer_value(raw: &[u8]) -> Result<String, String> {
    let s = String::from_utf8(raw.to_vec())
        .map_err(|_| "Invalid UTF-8 in stored acceptance pointer".to_string())?;
    let t = s.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    match serde_json::from_str::<String>(t) {
        Ok(decoded) => Ok(decoded),
        Err(_) => Ok(t.to_string()),
    }
}
