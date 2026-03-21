use super::tx_hash_pointer::decode_acceptance_pointer_value;
use crate::node::account_state::AccountState;
use crate::node::coordinate::Coordinates;
use crate::node::database::Database;

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RideRequest {
    pub pickup_location: Coordinates,
    pub dropoff_location: Coordinates,
    pub fare: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvailableRideRequest {
    pub tx_hash: String,
    pub pickup_location: Coordinates,
    pub dropoff_location: Coordinates,
    pub fare: u64,
    pub passenger_address: String,
}

/// Map viewport bounds for filtering ride requests by pickup location.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapBounds {
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lng: f64,
    pub max_lng: f64,
}

impl RideRequest {
    pub fn verify_state(&self, from: &String, db: &Database) -> Result<(), String> {
        let passenger_account_state = AccountState::get_current_state(from, &db);

        if passenger_account_state.balance < self.fare {
            return Err(format!(
                "The account balance is insufficient to cover the fare for the requested ride. \
                 Account balance is: {}, fare: {}",
                passenger_account_state.balance, self.fare
            ));
        }

        Ok(())
    }

    pub fn state_transaction(
        &self,
        from: &String,
        tx_hash :&String,
        _db: &Database,
    ) -> Vec<Option<(Vec<u8>, Vec<u8>)>> {

        let ride_request_key = Self::construct_ride_request_key(tx_hash);
        let ride_request_value = serde_json::to_string(&self).unwrap().into_bytes();

        let ride_request_from_key = Self::construct_ride_request_from_key(&tx_hash);
        let ride_request_from_value = from.clone().into_bytes();

        vec![
            Some((ride_request_key, ride_request_value)),
            Some((ride_request_from_key, ride_request_from_value)),
        ]
    }

    pub fn get_ride_request(
        ride_request_tx_hash: &str,
        db: &Database,
    ) -> Result<Option<RideRequest>, String> {
        let key = Self::construct_ride_request_key(ride_request_tx_hash);
        match db.get("state", &key) {
            Ok(Some(value)) => {
                let ride_request_str = match String::from_utf8(value) {
                    Ok(v) => v,
                    Err(_) => return Err("Failed to decode UTF-8 string".to_string()),
                };
                match serde_json::from_str(&ride_request_str) {
                    Ok(ride_request) => Ok(ride_request),
                    Err(_) => Err("Failed to deserialize RideRequest".to_string()),
                }
            }
            Ok(None) => Ok(None),
            Err(_) => Err("Database error occurred".to_string()),
        }
    }

    pub fn get_ride_acceptance(
        ride_request_tx_hash: &str,
        db: &Database,
    ) -> Result<Option<String>, String> {
        let key = Self::construct_ride_request_acceptance_key(ride_request_tx_hash);
        match db.get("state", &key) {
            Ok(Some(value)) => match decode_acceptance_pointer_value(&value) {
                Ok(v) if !v.is_empty() => Ok(Some(v)),
                Ok(_) => Ok(None),
                Err(e) => return Err(e),
            },
            Ok(None) => Ok(None),
            Err(_) => Err("Database error occurred".to_string()),
        }
    }

    /// Returns the cancel transaction hash if this ride request has been cancelled by the passenger.
    pub fn get_ride_request_cancel(
        ride_request_tx_hash: &str,
        db: &Database,
    ) -> Result<Option<String>, String> {
        let key = Self::construct_ride_request_cancel_key(ride_request_tx_hash);
        match db.get("state", &key) {
            Ok(Some(value)) => match String::from_utf8(value) {
                Ok(s) if !s.is_empty() => Ok(Some(s)),
                _ => Ok(None),
            },
            Ok(None) => Ok(None),
            Err(_) => Err("Database error occurred".to_string()),
        }
    }

    pub fn get_from(ride_request_tx_hash: &str, db: &Database) -> Result<Option<String>, String> {
        let key = Self::construct_ride_request_from_key(ride_request_tx_hash);
        match db.get("state", &key) {
            Ok(Some(value)) => match String::from_utf8(value) {
                Ok(from) => Ok(Some(from)),
                Err(_) => return Err("Failed to decode UTF-8 string".to_string()),
            },
            Ok(None) => Ok(None),
            Err(_) => Err("Database error occurred".to_string()),
        }
    }

    /// Lists ride requests that are still available (no RideAcceptance yet).
    /// Uses prefix iteration over `ride_request_*` keys and filters out accepted requests.
    /// Optionally filters by map bounds (pickup_location must be inside the bounding box).
    pub fn list_available_ride_requests(
        db: &Database,
        bounds: Option<MapBounds>,
    ) -> Result<Vec<AvailableRideRequest>, String> {
        const PREFIX: &str = "ride_request_";

        let entries = db.prefix_scan("state", PREFIX.as_bytes())?;
        let mut result = Vec::new();

        for (key, value) in entries {
            // Only main ride request keys (no ":from" or ":ride_acceptance" suffix)
            let key_str = String::from_utf8(key.clone()).map_err(|_| "Invalid UTF-8 in key".to_string())?;
            if key_str.contains(':') {
                continue;
            }

            let tx_hash = key_str
                .strip_prefix(PREFIX)
                .ok_or("Invalid ride request key")?
                .to_string();

            // Skip if already accepted
            if Self::get_ride_acceptance(&tx_hash, db)?.is_some() {
                continue;
            }

            // Skip if cancelled by passenger
            if Self::get_ride_request_cancel(&tx_hash, db)?.is_some() {
                continue;
            }

            let ride_request_str =
                String::from_utf8(value).map_err(|_| "Failed to decode UTF-8 string".to_string())?;
            let ride_request: RideRequest =
                serde_json::from_str(&ride_request_str).map_err(|_| "Failed to deserialize RideRequest".to_string())?;

            // Filter by map bounds (pickup_location) if provided
            if let Some(ref b) = bounds {
                let lat = ride_request.pickup_location.latitude;
                let lng = ride_request.pickup_location.longitude;
                if lat < b.min_lat || lat > b.max_lat || lng < b.min_lng || lng > b.max_lng {
                    continue;
                }
            }

            let passenger_address = Self::get_from(&tx_hash, db)?
                .unwrap_or_else(|| String::new());

            result.push(AvailableRideRequest {
                tx_hash,
                pickup_location: ride_request.pickup_location,
                dropoff_location: ride_request.dropoff_location,
                fare: ride_request.fare,
                passenger_address,
            });
        }

        Ok(result)
    }

    fn construct_ride_request_key(ride_request_tx_hash: &str) -> Vec<u8> {
        format!("ride_request_{}", ride_request_tx_hash).into_bytes()
    }

    pub fn construct_ride_request_from_key(ride_request_tx_hash: &str) -> Vec<u8> {
        format!("ride_request_{}:from", ride_request_tx_hash).into_bytes()
    }

    pub fn construct_ride_request_acceptance_key(ride_request_tx_hash: &str) -> Vec<u8> {
        format!("ride_request_{}:ride_acceptance", ride_request_tx_hash).into_bytes()
    }

    pub fn construct_ride_request_cancel_key(ride_request_tx_hash: &str) -> Vec<u8> {
        format!("ride_request_{}:cancelled", ride_request_tx_hash).into_bytes()
    }
}

impl Encodable for RideRequest {
    fn rlp_append(&self, stream: &mut RlpStream) {
        // Begin an RLP list with three elements: pickup_location, dropoff_location, and fare
        stream.begin_list(3);
        stream.append(&self.pickup_location);
        stream.append(&self.dropoff_location);
        stream.append(&self.fare);
    }
}

impl Decodable for RideRequest {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 3 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        
        Ok(RideRequest {
            pickup_location: rlp.val_at(0)?,
            dropoff_location: rlp.val_at(1)?,
            fare: rlp.val_at(2)?,
        })
    }
}
