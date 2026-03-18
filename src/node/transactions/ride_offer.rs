use super::ride_request::RideRequest;
use crate::node::database::Database;
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};
use tracing::error;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RideOffer {
    pub ride_request_transaction_hash: String,
    pub fare: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvailableRideOffer {
    pub tx_hash: String,
    pub ride_request_tx_hash: String,
    pub fare: u64,
    pub driver_address: String,
}

impl RideOffer {
    pub fn verify_state(&self, db: &Database) -> Result<(), String> {
        let ride_request_tx_hash = &self.ride_request_transaction_hash;

        if let Ok(Some(_)) = RideRequest::get_ride_request(&ride_request_tx_hash, db) {
            if let Ok(Some(_)) = RideRequest::get_ride_acceptance(&ride_request_tx_hash, db) {
                return Err("A ride for the requested ride offer already exists.".to_string());
            }
        } else {
            return Err("Ride request does not exist or failed to retrieve.".to_string());
        }

        Ok(())
    }

    pub fn state_transaction(
        &self,
        from: &String,
        tx_hash: &String,
        _db: &Database,
    ) -> Vec<Option<(Vec<u8>, Vec<u8>)>> {
        let ride_offer_key = Self::construct_ride_offer_key(tx_hash);
        let ride_offer_value = serde_json::to_string(&self).unwrap().into_bytes();

        let ride_offer_from_key = Self::construct_ride_offer_from_key(&tx_hash);
        let ride_offer_from_value = from.clone().into_bytes();

        vec![
            Some((ride_offer_key, ride_offer_value)),
            Some((ride_offer_from_key, ride_offer_from_value)),
        ]
    }

    pub fn get_ride_offer(
        ride_offer_tx_hash: &str,
        db: &Database,
    ) -> Result<Option<RideOffer>, String> {
        let key = Self::construct_ride_offer_key(ride_offer_tx_hash);
        match db.get("state", &key) {
            Ok(Some(value)) => {
                let ride_offer_str = match String::from_utf8(value) {
                    Ok(v) => v,
                    Err(_) => return Err("Failed to decode UTF-8 string".to_string()),
                };
                match serde_json::from_str(&ride_offer_str) {
                    Ok(ride_offer) => Ok(ride_offer),
                    Err(_) => Err("Failed to deserialize RideOffer".to_string()),
                }
            }
            Ok(None) => Ok(None),
            Err(_) => Err("Database error occurred".to_string()),
        }
    }

    pub fn get_ride_acceptance(
        ride_offer_tx_hash: &str,
        db: &Database,
    ) -> Result<Option<String>, String> {
        let key = Self::construct_ride_offer_acceptance_key(ride_offer_tx_hash);
        match db.get("state", &key) {
            Ok(Some(value)) => match String::from_utf8(value) {
                Ok(v) => Ok(Some(v)),
                Err(_) => return Err("Failed to decode UTF-8 string".to_string()),
            },
            Ok(None) => {
                error!(" No data found.{}", &ride_offer_tx_hash);
                Ok(None)
            }
            Err(_) => Err("Database error occurred".to_string()),
        }
    }

    pub fn get_from(ride_offer_tx_hash: &str, db: &Database) -> Result<Option<String>, String> {
        let key = Self::construct_ride_offer_from_key(ride_offer_tx_hash);
        match db.get("state", &key) {
            Ok(Some(value)) => match String::from_utf8(value) {
                Ok(from) => Ok(Some(from)),
                Err(_) => return Err("Failed to decode UTF-8 string".to_string()),
            },
            Ok(None) => Ok(None),
            Err(_) => Err("Database error occurred".to_string()),
        }
    }

    /// Lists ride offers for a specific ride request.
    pub fn list_ride_offers_for_request(
        db: &Database,
        ride_request_tx_hash: Option<&str>,
    ) -> Result<Vec<AvailableRideOffer>, String> {
        const PREFIX: &str = "ride_offer_";

        let entries = db.prefix_scan("state", PREFIX.as_bytes())?;
        let mut result = Vec::new();

        for (key, value) in entries {
            let key_str = match String::from_utf8(key) {
                Ok(k) => k,
                Err(_) => continue,
            };

            // Only process main ride offer keys (no ":" in key)
            if key_str.contains(':') {
                continue;
            }

            let tx_hash = match key_str.strip_prefix(PREFIX) {
                Some(h) => h.to_string(),
                None => continue,
            };

            let ride_offer_str = match String::from_utf8(value) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let ride_offer: RideOffer = match serde_json::from_str(&ride_offer_str) {
                Ok(o) => o,
                Err(_) => continue,
            };

            // Filter for the requested ride_request_tx_hash if provided
            if let Some(req_hash) = ride_request_tx_hash {
                if ride_offer.ride_request_transaction_hash != req_hash {
                    continue;
                }
            }

            // Skip if this specific offer has been accepted
            // (Optional: if we only want "active" offers. Usually we do)
            if Self::get_ride_acceptance(&tx_hash, db)?.is_some() {
                continue;
            }

            let driver_address = Self::get_from(&tx_hash, db)?.unwrap_or_else(String::new);

            result.push(AvailableRideOffer {
                tx_hash,
                ride_request_tx_hash: ride_offer.ride_request_transaction_hash,
                fare: ride_offer.fare,
                driver_address,
            });
        }

        Ok(result)
    }

    fn construct_ride_offer_key(ride_offer_tx_hash: &str) -> Vec<u8> {
        format!("ride_offer_{}", ride_offer_tx_hash).into_bytes()
    }

    pub fn construct_ride_offer_from_key(ride_request_tx_hash: &str) -> Vec<u8> {
        format!("ride_offer_{}:from", ride_request_tx_hash).into_bytes()
    }

    pub fn construct_ride_offer_acceptance_key(ride_offer_tx_hash: &str) -> Vec<u8> {
        format!("ride_offer_{}:ride_acceptance", ride_offer_tx_hash).into_bytes()
    }
}

impl Encodable for RideOffer {
    fn rlp_append(&self, stream: &mut RlpStream) {
        // Begin an RLP list with two elements: ride_request_transaction_hash and fare
        stream.begin_list(2);
        // Append the ride_request_transaction_hash field
        stream.append(&self.ride_request_transaction_hash);
        // Append the fare field
        stream.append(&self.fare);
    }
}

impl Decodable for RideOffer {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        // Ensure the RLP data is a list of exactly two items
        if !rlp.is_list() || rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }

        Ok(RideOffer {
            // Extract the ride_request_transaction_hash field from the first element
            ride_request_transaction_hash: rlp.val_at(0)?,
            // Extract the fare field from the second element
            fare: rlp.val_at(1)?,
        })
    }
}
