use crate::node::{
    account_state::AccountState,
    database::Database,
    transactions::address::normalize_address_for_compare,
    transactions::ride_offer::RideOffer,
    transactions::ride_request::RideRequest,
};
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RideAcceptance {
    pub ride_offer_transaction_hash: String,
}

/// An active trip: RideAcceptance exists, fare not yet paid, not cancelled.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvailableActiveTrip {
    pub tx_hash: String,
    pub ride_offer_tx_hash: String,
    pub ride_request_tx_hash: String,
    pub pickup_location: crate::node::coordinate::Coordinates,
    pub dropoff_location: crate::node::coordinate::Coordinates,
    pub fare: u64,
    /// Total amount already paid to the driver via RidePay (partial payments supported).
    pub fare_paid: u64,
    pub driver_address: String,
    pub passenger_address: String,
}

/// A finished trip history entry: full fare paid, or cancelled (not active / in-progress).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvailableRecentTrip {
    pub tx_hash: String,
    pub ride_offer_tx_hash: String,
    pub ride_request_tx_hash: String,
    pub pickup_location: crate::node::coordinate::Coordinates,
    pub dropoff_location: crate::node::coordinate::Coordinates,
    pub fare: u64,
    pub fare_paid: u64,
    pub driver_address: String,
    pub passenger_address: String,
    /// `"completed"` (full fare paid, not cancelled) or `"cancelled"`.
    pub trip_status: String,
}

/// DB keys use the same form as [`crate::node::transactions::transaction::Transaction::calculate_hash`]:
/// `0x` + lowercase hex. Hub/SDK clients often RLP-encode hashes without `0x`; normalize everywhere.
fn normalize_transaction_hash(hash: &str) -> String {
    let t = hash.trim();
    let hex_part = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    format!("0x{}", hex_part.to_ascii_lowercase())
}

impl RideAcceptance {
    pub fn verify_state(&self, from: &String, db: &Database) -> Result<(), String> {
        let ride_offer_transaction_hash = &self.ride_offer_transaction_hash;

        if let Ok(Some(ride_offer)) = RideOffer::get_ride_offer(ride_offer_transaction_hash, db) {
            if let Ok(Some(passenger)) =
                RideRequest::get_from(&ride_offer.ride_request_transaction_hash, db)
            {
                if &passenger.to_string() != from {
                    return Err(format!(
                        "Ride request 'from' field does not match the transaction 'from' field. Expected: {}, found: {}.",
                        from, passenger
                    ));
                }
            } else {
                return Err(format!(
                    "Failed to retrieve 'from' field for ride request with transaction hash '{}'.",
                    ride_offer.ride_request_transaction_hash
                ));
            }

            let passenger_account_state = AccountState::get_current_state(from, db);
            if &passenger_account_state.balance < &ride_offer.fare {
                return Err(format!(
                    "The account balance is insufficient to cover the fare for the requested ride. \
                     Account balance is: {}, fare: {}",
                    passenger_account_state.balance, ride_offer.fare
                ));
            }

            // Check if there is any ride linked to this ride offer's request.
            if let Ok(Some(_)) =
                RideRequest::get_ride_acceptance(&ride_offer.ride_request_transaction_hash, db)
            {
                return Err("A ride for the requested ride offer already exists.".to_string());
            }

            // Check if this ride offer is already used in another ride.
            if let Ok(Some(_)) = RideOffer::get_ride_acceptance(&ride_offer_transaction_hash, db) {
                return Err("Ride offer is already linked to a ride.".to_string());
            }

            // At most one active trip per driver (same model as one concurrent request per passenger).
            let driver_address = match RideOffer::get_from(ride_offer_transaction_hash, db)? {
                Some(a) => a,
                None => {
                    return Err(
                        "Failed to retrieve driver address for the ride offer.".to_string(),
                    );
                }
            };
            let driver_norm = normalize_address_for_compare(&driver_address);
            let active = Self::list_active_trips(db, None, None)?;
            if active.iter().any(|t| {
                normalize_address_for_compare(&t.driver_address) == driver_norm
            }) {
                return Err(
                    "Driver already has an active trip. Complete or cancel it before another ride can be accepted."
                        .to_string(),
                );
            }
        } else {
            return Err("Ride offer does not exist or failed to retrieve.".to_string());
        }

        Ok(())
    }

    pub fn state_transaction(
        &self,
        from: &String,
        tx_hash: &String,
        db: &Database,
    ) -> Vec<Option<(Vec<u8>, Vec<u8>)>> {
        let ride_acceptance_tx_hash = &tx_hash;
        let ride_offer_tx_hash = &self.ride_offer_transaction_hash;
        let ride_request_tx_hash = &RideOffer::get_ride_offer(&ride_offer_tx_hash, db)
            .unwrap()
            .unwrap()
            .ride_request_transaction_hash;

        let ride_acceptance_key = Self::construct_ride_acceptance_key(&ride_acceptance_tx_hash);
        let ride_acceptance_value = serde_json::to_string(&self)
            .unwrap()
            .into_bytes();

        let ride_request_acceptance_key =
            RideRequest::construct_ride_request_acceptance_key(&ride_request_tx_hash);
        // Store plain hash (no JSON quotes); see `tx_hash_pointer::decode_acceptance_pointer_value`.
        let ride_request_acceptance_value = ride_acceptance_tx_hash.as_bytes().to_vec();

        let ride_offer_acceptance_key =
            RideOffer::construct_ride_offer_acceptance_key(&ride_offer_tx_hash);
        let ride_offer_acceptance_value = ride_acceptance_tx_hash.as_bytes().to_vec();

        let ride_offer = RideOffer::get_ride_offer(&ride_offer_tx_hash, db)
            .unwrap()
            .unwrap();

        let transfer_value: i64 = ride_offer.fare as i64;
        let (passenger_account_state_key, passenger_account_state_value) =
            AccountState::update_account_state_key(&from, -transfer_value, db);

        vec![
            Some((ride_acceptance_key, ride_acceptance_value)), //ride_acceptance_{}
            Some((ride_request_acceptance_key, ride_request_acceptance_value)), //ride_request_{}:ride_acceptance
            Some((ride_offer_acceptance_key, ride_offer_acceptance_value)), //"ride_offer_{}:ride_acceptance
            Some((passenger_account_state_key, passenger_account_state_value)),
        ]
    }

    pub fn get_ride_acceptance(
        ride_acceptance_tx_hash: &str,
        db: &Database,
    ) -> Result<Option<RideAcceptance>, String> {
        let key = Self::construct_ride_acceptance_key(ride_acceptance_tx_hash);
        match db.get("state", &key) {
            Ok(Some(value)) => {
                let ride_acceptance_str = match String::from_utf8(value) {
                    Ok(v) => v,
                    Err(_) => return Err("Failed to decode UTF-8 string".to_string()),
                };
                match serde_json::from_str(&ride_acceptance_str) {
                    Ok(ride_acceptance) => Ok(ride_acceptance),
                    Err(_) => Err("Failed to deserialize RideOffer".to_string()),
                }
            }
            Ok(None) => Ok(None),
            Err(_) => Err("Database error occurred".to_string()),
        }
    }

    pub fn get_fare_paid(
        ride_acceptance_tx_hash: &str,
        db: &Database,
    ) -> Result<Option<i64>, String> {
        let key = Self::construct_ride_acceptance_fare_paid_key(ride_acceptance_tx_hash);
        match db.get("state", &key) {
            Ok(Some(value)) => {
                let fare_paid_str = match String::from_utf8(value) {
                    Ok(v) => v,
                    Err(_) => return Err("Failed to decode UTF-8 string".to_string()),
                };
                match serde_json::from_str(&fare_paid_str) {
                    Ok(ride_acceptance) => Ok(ride_acceptance),
                    Err(_) => Err("Failed to deserialize RideOffer".to_string()),
                }
            }
            Ok(None) => Ok(None),
            Err(_) => Err("Database error occurred".to_string()),
        }
    }

    pub fn get_ride_cancel(
        ride_acceptance_tx_hash: &str,
        db: &Database,
    ) -> Result<Option<String>, String> {
        let key = Self::construct_ride_acceptance_cancel_key(ride_acceptance_tx_hash);
        match db.get("state", &key) {
            Ok(Some(value)) => match String::from_utf8(value) {
                Ok(v) => Ok(Some(v)),
                Err(_) => return Err("Failed to decode UTF-8 string".to_string()),
            },
            Ok(None) => Ok(None),
            Err(_) => Err("Database error occurred".to_string()),
        }
    }

    pub fn construct_ride_acceptance_key(ride_acceptance_tx_hash: &str) -> Vec<u8> {
        let h = normalize_transaction_hash(ride_acceptance_tx_hash);
        format!("ride_acceptance_{}", h).into_bytes()
    }

    pub fn construct_ride_acceptance_fare_paid_key(ride_acceptance_tx_hash: &str) -> Vec<u8> {
        let h = normalize_transaction_hash(ride_acceptance_tx_hash);
        format!("ride_acceptance_{}:fare_paid", h).into_bytes()
    }

    pub fn construct_ride_acceptance_cancel_key(ride_acceptance_tx_hash: &str) -> Vec<u8> {
        let h = normalize_transaction_hash(ride_acceptance_tx_hash);
        format!("ride_acceptance_{}:cancel", h).into_bytes()
    }

    /// Lists active trips: accepted ride, not cancelled, and total RidePay amount &lt; offer fare
    /// (supports partial payments until the full fare is paid).
    /// Optionally filter by driver_address and/or passenger_address.
    pub fn list_active_trips(
        db: &Database,
        driver_address: Option<&str>,
        passenger_address: Option<&str>,
    ) -> Result<Vec<AvailableActiveTrip>, String> {
        const PREFIX: &str = "ride_offer_";
        let entries = db.prefix_scan("state", PREFIX.as_bytes())?;
        let mut result = Vec::new();

        for (key, _value) in entries {
            let key_str = match String::from_utf8(key) {
                Ok(k) => k,
                Err(_) => continue,
            };
            if key_str.contains(':') {
                continue;
            }

            let ride_offer_tx_hash = match key_str.strip_prefix(PREFIX) {
                Some(h) => h.to_string(),
                None => continue,
            };

            let acceptance_tx_hash = match RideOffer::get_ride_acceptance(&ride_offer_tx_hash, db) {
                Ok(Some(h)) => h,
                _ => continue,
            };

            if Self::get_ride_cancel(&acceptance_tx_hash, db)?.is_some() {
                continue;
            }

            let ride_offer = match RideOffer::get_ride_offer(&ride_offer_tx_hash, db)
                .map_err(|e| e.to_string())?
            {
                Some(o) => o,
                None => continue,
            };

            let fare_paid_so_far: u64 = match Self::get_fare_paid(&acceptance_tx_hash, db)? {
                Some(v) if v >= 0 => v as u64,
                Some(_) => 0,
                None => 0,
            };

            if fare_paid_so_far >= ride_offer.fare {
                continue;
            }

            let driver_address_val = RideOffer::get_from(&ride_offer_tx_hash, db)
                .ok()
                .flatten()
                .unwrap_or_default();
            if let Some(filter) = driver_address {
                if driver_address_val != filter {
                    continue;
                }
            }

            let passenger_address_val = RideRequest::get_from(&ride_offer.ride_request_transaction_hash, db)
                .ok()
                .flatten()
                .unwrap_or_default();
            if let Some(filter) = passenger_address {
                if passenger_address_val != filter {
                    continue;
                }
            }

            let ride_request = match RideRequest::get_ride_request(&ride_offer.ride_request_transaction_hash, db)
                .map_err(|e| e.to_string())?
            {
                Some(r) => r,
                None => continue,
            };

            result.push(AvailableActiveTrip {
                tx_hash: acceptance_tx_hash,
                ride_offer_tx_hash,
                ride_request_tx_hash: ride_offer.ride_request_transaction_hash,
                pickup_location: ride_request.pickup_location,
                dropoff_location: ride_request.dropoff_location,
                fare: ride_offer.fare,
                fare_paid: fare_paid_so_far,
                driver_address: driver_address_val,
                passenger_address: passenger_address_val,
            });
        }

        Ok(result)
    }

    /// Lists completed trips: accepted, not cancelled, and total RidePay amount &gt;= offer fare.
    /// Optionally filter by driver_address and/or passenger_address.
    pub fn list_completed_trips(
        db: &Database,
        driver_address: Option<&str>,
        passenger_address: Option<&str>,
    ) -> Result<Vec<AvailableActiveTrip>, String> {
        const PREFIX: &str = "ride_offer_";
        let entries = db.prefix_scan("state", PREFIX.as_bytes())?;
        let mut result = Vec::new();

        for (key, _value) in entries {
            let key_str = match String::from_utf8(key) {
                Ok(k) => k,
                Err(_) => continue,
            };
            if key_str.contains(':') {
                continue;
            }

            let ride_offer_tx_hash = match key_str.strip_prefix(PREFIX) {
                Some(h) => h.to_string(),
                None => continue,
            };

            let acceptance_tx_hash = match RideOffer::get_ride_acceptance(&ride_offer_tx_hash, db) {
                Ok(Some(h)) => h,
                _ => continue,
            };

            if Self::get_ride_cancel(&acceptance_tx_hash, db)?.is_some() {
                continue;
            }

            let ride_offer = match RideOffer::get_ride_offer(&ride_offer_tx_hash, db)
                .map_err(|e| e.to_string())?
            {
                Some(o) => o,
                None => continue,
            };

            let fare_paid_so_far: u64 = match Self::get_fare_paid(&acceptance_tx_hash, db)? {
                Some(v) if v >= 0 => v as u64,
                Some(_) => 0,
                None => 0,
            };

            if fare_paid_so_far < ride_offer.fare {
                continue;
            }

            let driver_address_val = RideOffer::get_from(&ride_offer_tx_hash, db)
                .ok()
                .flatten()
                .unwrap_or_default();
            if let Some(filter) = driver_address {
                if driver_address_val != filter {
                    continue;
                }
            }

            let passenger_address_val = RideRequest::get_from(&ride_offer.ride_request_transaction_hash, db)
                .ok()
                .flatten()
                .unwrap_or_default();
            if let Some(filter) = passenger_address {
                if passenger_address_val != filter {
                    continue;
                }
            }

            let ride_request = match RideRequest::get_ride_request(&ride_offer.ride_request_transaction_hash, db)
                .map_err(|e| e.to_string())?
            {
                Some(r) => r,
                None => continue,
            };

            result.push(AvailableActiveTrip {
                tx_hash: acceptance_tx_hash,
                ride_offer_tx_hash,
                ride_request_tx_hash: ride_offer.ride_request_transaction_hash,
                pickup_location: ride_request.pickup_location,
                dropoff_location: ride_request.dropoff_location,
                fare: ride_offer.fare,
                fare_paid: fare_paid_so_far,
                driver_address: driver_address_val,
                passenger_address: passenger_address_val,
            });
        }

        Ok(result)
    }

    /// Lists recent finished trips: **completed** (full fare paid, not cancelled) or **cancelled**.
    /// Excludes active in-progress trips (those appear in [`Self::list_active_trips`]).
    /// Optionally filter by driver_address and/or passenger_address.
    pub fn list_recent_trips(
        db: &Database,
        driver_address: Option<&str>,
        passenger_address: Option<&str>,
    ) -> Result<Vec<AvailableRecentTrip>, String> {
        const PREFIX: &str = "ride_offer_";
        let entries = db.prefix_scan("state", PREFIX.as_bytes())?;
        let mut result = Vec::new();

        for (key, _value) in entries {
            let key_str = match String::from_utf8(key) {
                Ok(k) => k,
                Err(_) => continue,
            };
            if key_str.contains(':') {
                continue;
            }

            let ride_offer_tx_hash = match key_str.strip_prefix(PREFIX) {
                Some(h) => h.to_string(),
                None => continue,
            };

            let acceptance_tx_hash = match RideOffer::get_ride_acceptance(&ride_offer_tx_hash, db) {
                Ok(Some(h)) => h,
                _ => continue,
            };

            let cancelled = Self::get_ride_cancel(&acceptance_tx_hash, db)?.is_some();

            let ride_offer = match RideOffer::get_ride_offer(&ride_offer_tx_hash, db)
                .map_err(|e| e.to_string())?
            {
                Some(o) => o,
                None => continue,
            };

            let fare_paid_so_far: u64 = match Self::get_fare_paid(&acceptance_tx_hash, db)? {
                Some(v) if v >= 0 => v as u64,
                Some(_) => 0,
                None => 0,
            };

            if !cancelled && fare_paid_so_far < ride_offer.fare {
                continue;
            }

            let driver_address_val = RideOffer::get_from(&ride_offer_tx_hash, db)
                .ok()
                .flatten()
                .unwrap_or_default();
            if let Some(filter) = driver_address {
                if driver_address_val != filter {
                    continue;
                }
            }

            let passenger_address_val = RideRequest::get_from(&ride_offer.ride_request_transaction_hash, db)
                .ok()
                .flatten()
                .unwrap_or_default();
            if let Some(filter) = passenger_address {
                if passenger_address_val != filter {
                    continue;
                }
            }

            let ride_request = match RideRequest::get_ride_request(&ride_offer.ride_request_transaction_hash, db)
                .map_err(|e| e.to_string())?
            {
                Some(r) => r,
                None => continue,
            };

            let trip_status = if cancelled {
                "cancelled".to_string()
            } else {
                "completed".to_string()
            };

            result.push(AvailableRecentTrip {
                tx_hash: acceptance_tx_hash,
                ride_offer_tx_hash,
                ride_request_tx_hash: ride_offer.ride_request_transaction_hash,
                pickup_location: ride_request.pickup_location,
                dropoff_location: ride_request.dropoff_location,
                fare: ride_offer.fare,
                fare_paid: fare_paid_so_far,
                driver_address: driver_address_val,
                passenger_address: passenger_address_val,
                trip_status,
            });
        }

        Ok(result)
    }
}

impl Encodable for RideAcceptance {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(1);
        stream.append(&self.ride_offer_transaction_hash);
    }
}

impl Decodable for RideAcceptance {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        Ok(RideAcceptance {
            ride_offer_transaction_hash: rlp.val_at(0)?,
        })
    }
}
