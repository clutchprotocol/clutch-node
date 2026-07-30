use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::node::{
    account_state::AccountState,
    balance_effect::{BalanceEffectKind, StateUpdate},
    database::Database,
};

use super::{ride_acceptance::RideAcceptance, ride_offer::RideOffer, ride_request::RideRequest};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RideCancel {
    pub ride_acceptance_transaction_hash: String,
}

impl RideCancel {
    pub fn verify_state(&self, from: &String, db: &Database) -> Result<(), String> {
        let ride_acceptance_tx_hash = &self.ride_acceptance_transaction_hash;
        let ride_acceptance = RideAcceptance::get_ride_acceptance(ride_acceptance_tx_hash, db)
            .map_err(|_| "Ride acceptance does not exist or failed to retrieve.".to_string())?
            .ok_or_else(|| "Ride acceptance does not exist.".to_string())?;

        let ride_cancel_exists = match RideAcceptance::get_ride_cancel(ride_acceptance_tx_hash, db)
        {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(_) => {
                return Err(format!(
                    "Failed to retrieve ride cancel for transaction hash '{}'.",
                    ride_acceptance_tx_hash
                ));
            }
        };

        if ride_cancel_exists {
            return Err(
                "A ride cancel for the requested ride acceptance already exists.".to_string(),
            );
        }

        let ride_offer =
            RideOffer::get_ride_offer(&ride_acceptance.ride_offer_transaction_hash, db)
                .map_err(|_| {
                    format!(
                        "Failed to retrieve ride offer '{}'.",
                        &ride_acceptance.ride_offer_transaction_hash
                    )
                })?
                .ok_or_else(|| "Ride offer does not exist.".to_string())?;

        let passenger = RideRequest::get_from(&ride_offer.ride_request_transaction_hash, db)
            .map_err(|_| {
                format!(
                    "Failed to retrieve 'from' field for ride request with transaction hash '{}'.",
                    &ride_offer.ride_request_transaction_hash
                )
            })?
            .ok_or_else(|| "Ride request does not exist.".to_string())?;

        let driver = RideOffer::get_from(&ride_acceptance.ride_offer_transaction_hash, db)
            .map_err(|_| {
                format!(
                    "Failed to retrieve 'from' field for ride offer with transaction hash '{}'.",
                    &ride_acceptance.ride_offer_transaction_hash
                )
            })?
            .ok_or_else(|| "Ride offer does not exist.".to_string())?;

        let fare_paid = RideAcceptance::get_fare_paid(ride_acceptance_tx_hash, db)
            .map_err(|_| format!(
                "Failed to retrieve 'fare_paid' field for ride acceptance with transaction hash '{}'.",
                ride_acceptance_tx_hash
            ))?
            .unwrap_or(0);

        if (fare_paid as u64) == ride_offer.fare {
            return Err(format!(
                "The full fare for ride acceptance '{}' has been paid. No further payments are needed, and the ride cannot be cancelled.",
                ride_acceptance_tx_hash
            ));
        }

        if passenger.to_string() != from.to_string() && driver.to_string() != from.to_string() {
            return Err(format!(
                "Transaction 'from' field does not match the expected values. Expected either passenger: '{}' or driver: '{}', but found: '{}'.",
                passenger, driver, from
            ));
        }

        Ok(())
    }

    pub fn state_transaction(
        &self,
        from: &String,
        tx_hash: &String,
        db: &Database,
        fee: u64,
    ) -> Vec<StateUpdate> {
        let ride_cancel_key = Self::construct_ride_cancel_key(&tx_hash);
        let ride_cancel_value = serde_json::to_string(&self)
            .expect("Failed to serialize RidePay.")
            .into_bytes();

        let ride_acceptance_tx_hash = &self.ride_acceptance_transaction_hash;

        let ride_acceptance_cancel_key =
            RideAcceptance::construct_ride_acceptance_cancel_key(&ride_acceptance_tx_hash);
        let ride_acceptance_cancel_value = serde_json::to_string(&tx_hash).unwrap().into_bytes();

        let fare_paid = match RideAcceptance::get_fare_paid(&ride_acceptance_tx_hash, db) {
            Ok(Some(fare)) => fare,
            Ok(None) => 0,
            Err(_) => {
                error!(
                        "Failed to retrieve 'fare_paid' field for ride acceptace with transaction hash '{}'.",
                        &ride_acceptance_tx_hash
                    );
                0
            }
        };

        let ride_acceptance = RideAcceptance::get_ride_acceptance(ride_acceptance_tx_hash, db)
            .unwrap()
            .unwrap();

        let ride_offer =
            RideOffer::get_ride_offer(&ride_acceptance.ride_offer_transaction_hash, db)
                .unwrap()
                .unwrap();

        let passenger = RideRequest::get_from(&ride_offer.ride_request_transaction_hash, db)
            .unwrap()
            .unwrap();

        let remaining_amount = (ride_offer.fare as i64) - (fare_paid as i64);

        use crate::node::transactions::address::canonical_account_address;
        let sender_is_passenger =
            canonical_account_address(from) == canonical_account_address(&passenger);

        // ponytail: when the passenger cancels, refund credit and fee debit hit the SAME
        // account — merge into one write. Driver-cancel: driver's key is otherwise
        // untouched, standalone fee debit is safe.
        let mut updates = vec![
            StateUpdate::storage_only(ride_cancel_key, ride_cancel_value),
            StateUpdate::storage_only(ride_acceptance_cancel_key, ride_acceptance_cancel_value),
        ];
        if sender_is_passenger {
            updates.extend(AccountState::apply_balance_change_with_fee(
                &passenger,
                remaining_amount,
                fee,
                BalanceEffectKind::RideCancelRefund,
                None,
                db,
            ));
        } else {
            updates.push(AccountState::apply_balance_change(
                &passenger,
                remaining_amount,
                BalanceEffectKind::RideCancelRefund,
                None,
                db,
            ));
            if fee > 0 {
                updates.push(AccountState::apply_balance_change(
                    from,
                    -(fee as i64),
                    BalanceEffectKind::TxFeePaid,
                    None,
                    db,
                ));
            }
        }
        updates
    }

    pub fn construct_ride_cancel_key(tx_hash: &str) -> Vec<u8> {
        format!("ride_pay_{}", tx_hash).into_bytes()
    }
}

impl Encodable for RideCancel {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(1);
        stream.append(&self.ride_acceptance_transaction_hash);
    }
}

impl Decodable for RideCancel {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 1 {
            return Err(DecoderError::RlpIncorrectListLen);
        }

        Ok(RideCancel {
            ride_acceptance_transaction_hash: rlp.val_at(0)?,
        })
    }
}
