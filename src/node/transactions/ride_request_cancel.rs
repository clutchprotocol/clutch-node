use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};

use crate::node::database::Database;

use super::ride_request::RideRequest;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RideRequestCancel {
    pub ride_request_transaction_hash: String,
}

impl RideRequestCancel {
    pub fn verify_state(&self, from: &str, db: &Database) -> Result<(), String> {
        let ride_request_tx_hash = &self.ride_request_transaction_hash;

        RideRequest::get_ride_request(ride_request_tx_hash, db)
            .map_err(|_| "Ride request does not exist or failed to retrieve.".to_string())?
            .ok_or_else(|| "Ride request does not exist.".to_string())?;

        // Cannot cancel if already accepted by a driver
        if RideRequest::get_ride_acceptance(ride_request_tx_hash, db)
            .map_err(|_| "Failed to check ride acceptance.".to_string())?
            .is_some()
        {
            return Err(
                "This ride request has already been accepted by a driver. Use RideCancel to cancel the active trip."
                    .to_string(),
            );
        }

        // Cannot cancel if already cancelled
        if RideRequest::get_ride_request_cancel(ride_request_tx_hash, db)
            .map_err(|_| "Failed to check ride request cancel status.".to_string())?
            .is_some()
        {
            return Err("This ride request has already been cancelled.".to_string());
        }

        let passenger = RideRequest::get_from(ride_request_tx_hash, db)
            .map_err(|_| {
                format!(
                    "Failed to retrieve passenger for ride request '{}'.",
                    ride_request_tx_hash
                )
            })?
            .ok_or_else(|| "Ride request passenger not found.".to_string())?;

        if super::address::normalize_address_for_compare(&passenger)
            != super::address::normalize_address_for_compare(from)
        {
            return Err(format!(
                "Only the passenger who created the ride request can cancel it. Expected: '{}', found: '{}'.",
                passenger, from
            ));
        }

        Ok(())
    }

    pub fn state_transaction(
        &self,
        cancel_tx_hash: &str,
        _db: &Database,
    ) -> Vec<Option<(Vec<u8>, Vec<u8>)>> {
        let ride_request_tx_hash = &self.ride_request_transaction_hash;

        let cancel_key = RideRequest::construct_ride_request_cancel_key(ride_request_tx_hash);
        let cancel_value = cancel_tx_hash.as_bytes().to_vec();

        vec![Some((cancel_key, cancel_value))]
    }
}

impl Encodable for RideRequestCancel {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(1);
        stream.append(&self.ride_request_transaction_hash);
    }
}

impl Decodable for RideRequestCancel {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 1 {
            return Err(DecoderError::RlpIncorrectListLen);
        }

        Ok(RideRequestCancel {
            ride_request_transaction_hash: rlp.val_at(0)?,
        })
    }
}
