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

    /// Settles a cancelled trip.
    ///
    /// Before the auto-release window expires this refunds the unpaid remainder to the rider,
    /// which is what it has always done and is the rider's protection: they stop paying and cancel,
    /// losing only what they already released.
    ///
    /// After the window expires the same transaction pays that remainder to the **driver** instead.
    /// That closes the gap this mechanism exists for. Until now a rider could take the ride and
    /// simply never send the rest of the `RidePay` instalments; the driver's only move was to
    /// cancel, which refunded the money to the rider. The driver had performed and had no way to
    /// be paid. Inaction favoured the party who owed money, and now it favours the one who is owed.
    ///
    /// Deliberately the same transaction type rather than a new one. Nothing else needs to change:
    /// no new state machine, no settlement pass scanning every open trip each block, and every
    /// existing client keeps working.
    pub fn state_transaction(
        &self,
        from: &String,
        tx_hash: &String,
        db: &Database,
        fee: u64,
        auto_release_secs: u64,
        block_timestamp: u64,
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

        // Who the held remainder belongs to now. Past the window it is the driver's, whoever
        // submits the cancel — including the rider, for whom cancelling late no longer helps.
        let released_to_driver = RideAcceptance::auto_release_elapsed(
            ride_acceptance_tx_hash,
            db,
            auto_release_secs,
            block_timestamp,
        );

        let driver = RideOffer::get_from(&ride_acceptance.ride_offer_transaction_hash, db)
            .ok()
            .flatten();

        // A driver address that cannot be read is the one case where paying out would be guessing.
        // Fall back to the refund rather than send the money somewhere unverified.
        let (beneficiary, effect_kind) = match (released_to_driver, driver.as_ref()) {
            (true, Some(d)) => (d.clone(), BalanceEffectKind::RideAutoRelease),
            _ => (passenger.clone(), BalanceEffectKind::RideCancelRefund),
        };
        let beneficiary_is_sender =
            canonical_account_address(from) == canonical_account_address(&beneficiary);

        // ponytail: when the passenger cancels, refund credit and fee debit hit the SAME
        // account — merge into one write. Driver-cancel: driver's key is otherwise
        // untouched, standalone fee debit is safe.
        let mut updates = vec![
            StateUpdate::storage_only(ride_cancel_key, ride_cancel_value),
            StateUpdate::storage_only(ride_acceptance_cancel_key, ride_acceptance_cancel_value),
        ];
        // Merge the credit and the fee debit when they land on the same account, since two writes
        // to one balance in a single transaction is the shape that loses one of them.
        let _ = sender_is_passenger;
        if beneficiary_is_sender {
            updates.extend(AccountState::apply_balance_change_with_fee(
                &beneficiary,
                remaining_amount,
                fee,
                effect_kind,
                None,
                db,
            ));
        } else {
            updates.push(AccountState::apply_balance_change(
                &beneficiary,
                remaining_amount,
                effect_kind,
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
