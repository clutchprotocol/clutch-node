//! Passenger concurrency rules: at most one pending ride request or active trip at a time.

use crate::node::database::Database;

use super::ride_acceptance::RideAcceptance;
use super::ride_request::RideRequest;

/// Normalize address strings for comparison (handles `0x` / `0X` prefix and casing).
pub fn normalize_address_for_compare(addr: &str) -> String {
    let t = addr.trim();
    let hex_part = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    format!("0x{}", hex_part.to_ascii_lowercase())
}

/// Returns true if the passenger already has a pending ride request or an active trip.
pub fn passenger_has_concurrent_request(
    db: &Database,
    passenger_address: &str,
) -> Result<bool, String> {
    let passenger_norm = normalize_address_for_compare(passenger_address);

    let pending = RideRequest::list_available_ride_requests(db, None)?;
    if pending.iter().any(|r| {
        normalize_address_for_compare(&r.passenger_address) == passenger_norm
    }) {
        return Ok(true);
    }

    let active = RideAcceptance::list_active_trips(db, None, None)?;
    if active.iter().any(|t| {
        normalize_address_for_compare(&t.passenger_address) == passenger_norm
    }) {
        return Ok(true);
    }

    Ok(false)
}
