use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::error;

use crate::node::account_state::AccountState;
use crate::node::balance_effect::{BalanceEffectKind, StateUpdate};
use crate::node::database::Database;

use super::{
    address::canonical_account_address,
    ride_acceptance::RideAcceptance,
    ride_offer::RideOffer,
    ride_request::RideRequest,
};

fn referrer_fee_ceiling(percent: u8, fare: u64) -> u64 {
    if percent == 0 || fare == 0 {
        return 0;
    }
    (percent as u64 * fare + 99) / 100
}

#[derive(Default)]
struct ReferrerFeeParts {
    request: u64,
    offer: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RidePay {
    pub ride_acceptance_transaction_hash: String,
    pub fare: u64,
}

impl RidePay {
    pub fn verify_state(&self, from: &String, db: &Database) -> Result<(), String> {
        let ride_acceptance_tx_hash = &self.ride_acceptance_transaction_hash;
        let ride_acceptance = RideAcceptance::get_ride_acceptance(ride_acceptance_tx_hash, db)
            .map_err(|_| "Ride acceptance does not exist or failed to retrieve.".to_string())?
            .ok_or("Ride acceptance does not exist.")?;

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
                .ok_or("Ride offer does not exist.")?;

        let passenger = RideRequest::get_from(&ride_offer.ride_request_transaction_hash, db)
            .map_err(|_| {
                format!(
                    "Failed to retrieve 'from' field for ride request with transaction hash '{}'.",
                    &ride_offer.ride_request_transaction_hash
                )
            })?
            .ok_or("Ride request does not exist.")?;

        let fare_paid = RideAcceptance::get_fare_paid(ride_acceptance_tx_hash, db)
            .map_err(|_| format!(
                "Failed to retrieve 'fare_paid' field for ride acceptance with transaction hash '{}'.",
                &ride_acceptance_tx_hash
            ))?
            .unwrap_or(0);

        if passenger.to_string() != from.to_string() {
            return Err(format!(
                "Ride request 'from' field does not match the transaction 'from' field. Expected: {}, found: {}.",
                from, passenger
            ));
        }

        let total_fare = (fare_paid as u64) + self.fare;
        if total_fare > ride_offer.fare {
            return Err(format!(
                "The total fare in the ride pay ({}) is greater than the fare in the ride offer ({}).",
                total_fare, ride_offer.fare
            ));
        }

        Ok(())
    }

    pub fn state_transaction(
        &self,
        tx_hash: &String,
        db: &Database,
        request_fee_percent: u8,
        offer_fee_percent: u8,
        passenger: &String,
    ) -> Vec<StateUpdate> {
        let ride_acceptance_tx_hash = &self.ride_acceptance_transaction_hash;

        let ride_pay_key = Self::construct_ride_pay_key(tx_hash);
        let ride_pay_value = serde_json::to_string(self)
            .expect("Failed to serialize RidePay.")
            .into_bytes();

        let ride_acceptance = RideAcceptance::get_ride_acceptance(ride_acceptance_tx_hash, db)
            .unwrap()
            .unwrap();

        let ride_offer_tx_hash = &ride_acceptance.ride_offer_transaction_hash;
        let driver = RideOffer::get_from(ride_offer_tx_hash, db)
            .unwrap()
            .unwrap();

        let ride_offer = RideOffer::get_ride_offer(ride_offer_tx_hash, db)
            .unwrap()
            .unwrap();
        let ride_request_tx_hash = &ride_offer.ride_request_transaction_hash;
        let ride_request = RideRequest::get_ride_request(ride_request_tx_hash, db)
            .unwrap()
            .unwrap();

        let request_referrer = ride_request.referrer;
        let offer_referrer = ride_offer.referrer;

        let fare_paid = match RideAcceptance::get_fare_paid(ride_acceptance_tx_hash, db) {
            Ok(Some(fare)) => fare,
            Ok(None) => 0,
            Err(_) => {
                error!(
                    "Failed to retrieve 'fare_paid' field for ride acceptace with transaction hash '{}'.",
                    ride_acceptance_tx_hash
                );
                0
            }
        };

        let total_fare = (fare_paid as u64) + self.fare;
        let fare_paid_key =
            RideAcceptance::construct_ride_acceptance_fare_paid_key(ride_acceptance_tx_hash);
        let fare_paid_value = serde_json::to_string(&total_fare).unwrap().into_bytes();

        let mut updates: Vec<StateUpdate> = vec![
            StateUpdate::storage_only(ride_pay_key, ride_pay_value),
            StateUpdate::storage_only(fare_paid_key, fare_paid_value),
        ];

        let mut referrer_fees: HashMap<String, ReferrerFeeParts> = HashMap::new();

        if let Some(ref req_ref) = request_referrer {
            let fee = referrer_fee_ceiling(request_fee_percent, self.fare);
            if fee > 0 {
                let canonical = canonical_account_address(req_ref);
                referrer_fees.entry(canonical).or_default().request += fee;
            }
        }

        if let Some(ref off_ref) = offer_referrer {
            let fee = referrer_fee_ceiling(offer_fee_percent, self.fare);
            if fee > 0 {
                let canonical = canonical_account_address(off_ref);
                referrer_fees.entry(canonical).or_default().offer += fee;
            }
        }

        let passenger_cp = Some(passenger.clone());
        let mut total_deducted: u64 = 0;

        for (referrer, parts) in referrer_fees {
            if parts.request > 0 {
                updates.push(AccountState::apply_balance_change(
                    &referrer,
                    parts.request as i64,
                    BalanceEffectKind::ReferrerRequestFee,
                    passenger_cp.clone(),
                    db,
                ));
                total_deducted += parts.request;
            }
            if parts.offer > 0 {
                updates.push(AccountState::apply_balance_change(
                    &referrer,
                    parts.offer as i64,
                    BalanceEffectKind::ReferrerOfferFee,
                    passenger_cp.clone(),
                    db,
                ));
                total_deducted += parts.offer;
            }
        }

        let driver_amount = self.fare - total_deducted;
        updates.push(AccountState::apply_balance_change(
            &driver,
            driver_amount as i64,
            BalanceEffectKind::RidePayDriverCredit,
            passenger_cp,
            db,
        ));

        updates
    }

    pub fn construct_ride_pay_key(tx_hash: &str) -> Vec<u8> {
        format!("ride_pay_{}", tx_hash).into_bytes()
    }
}

impl Encodable for RidePay {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(2);
        stream.append(&self.ride_acceptance_transaction_hash);
        stream.append(&self.fare);
    }
}

impl Decodable for RidePay {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }

        Ok(RidePay {
            ride_acceptance_transaction_hash: rlp.val_at(0)?,
            fare: rlp.val_at(1)?,
        })
    }
}
