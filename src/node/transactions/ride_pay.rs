use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};
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

/// Referrer fee in base units: floor(fare * bps / 10_000). Stored as basis points so
/// fractional percentages need no config migration (spec §4a). u128 intermediate —
/// the product can exceed u64 but the result never does (result <= fare).
fn referrer_fee_floor(bps: u16, fare: u64) -> u64 {
    ((fare as u128 * bps as u128) / 10_000) as u64
}

/// Split `fare` into (request-referrer fee, offer-referrer fee, driver remainder),
/// capping the two fees so their sum can never exceed `fare`. Without the cap, ceiling
/// rounding on tiny fares (2% of 1 rounds up to 1 on each side) makes the fees sum to
/// more than the fare, and `fare - total_deducted` underflows the driver's u64 amount
/// (wrapping to ~u64::MAX in release builds — a money mint).
fn split_fare(fare: u64, request_fee: u64, offer_fee: u64) -> (u64, u64, u64) {
    let request = request_fee.min(fare);
    let offer = offer_fee.min(fare - request);
    let driver = fare - request - offer;
    debug_assert_eq!(request + offer + driver, fare, "fee split must sum exactly");
    (request, offer, driver)
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
        request_fee_bps: u16,
        offer_fee_bps: u16,
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

        // Cap referrer fees so request + offer can never exceed the fare being paid; the
        // driver gets the remainder. Prevents the `fare - total_deducted` underflow.
        let request_fee = match &request_referrer {
            Some(_) => referrer_fee_floor(request_fee_bps, self.fare),
            None => 0,
        };
        let offer_fee = match &offer_referrer {
            Some(_) => referrer_fee_floor(offer_fee_bps, self.fare),
            None => 0,
        };
        let (request_fee, offer_fee, driver_amount) =
            split_fare(self.fare, request_fee, offer_fee);

        let passenger_cp = Some(passenger.clone());

        if request_fee > 0 {
            if let Some(ref req_ref) = request_referrer {
                updates.push(AccountState::apply_balance_change(
                    &canonical_account_address(req_ref),
                    request_fee as i64,
                    BalanceEffectKind::ReferrerRequestFee,
                    passenger_cp.clone(),
                    db,
                ));
            }
        }

        if offer_fee > 0 {
            if let Some(ref off_ref) = offer_referrer {
                updates.push(AccountState::apply_balance_change(
                    &canonical_account_address(off_ref),
                    offer_fee as i64,
                    BalanceEffectKind::ReferrerOfferFee,
                    passenger_cp.clone(),
                    db,
                ));
            }
        }

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

#[cfg(test)]
mod tests {
    use super::{referrer_fee_floor, split_fare};
    use proptest::prelude::*;

    #[test]
    fn referrer_fee_floor_bps() {
        assert_eq!(referrer_fee_floor(0, 100), 0);
        assert_eq!(referrer_fee_floor(200, 0), 0);
        assert_eq!(referrer_fee_floor(200, 100), 2); // 2% of 100
        // Floor kills the old ceiling distortion (2% of 3 ceiling-rounded to 33%).
        assert_eq!(referrer_fee_floor(200, 3), 0);
        assert_eq!(referrer_fee_floor(200, 49), 0);
        assert_eq!(referrer_fee_floor(200, 50), 1);
        assert_eq!(referrer_fee_floor(10_000, u64::MAX), u64::MAX); // 100%, no overflow
        assert_eq!(referrer_fee_floor(1, 10_000), 1); // 1 bp granularity
    }

    #[test]
    fn split_fare_never_exceeds_fare() {
        assert_eq!(split_fare(100, 2, 2), (2, 2, 96));
        assert_eq!(split_fare(1, 1, 1), (1, 0, 0));
        assert_eq!(split_fare(10, 8, 8), (8, 2, 0));
        assert_eq!(split_fare(50, 0, 0), (0, 0, 50));
    }

    proptest! {
        // Spec §4a: request + offer + driver == fare, exactly, for every input.
        #[test]
        fn fee_split_sums_exactly(fare in any::<u64>(), rbps in 0u16..=10_000, obps in 0u16..=10_000) {
            let (r, o, d) = split_fare(
                fare,
                referrer_fee_floor(rbps, fare),
                referrer_fee_floor(obps, fare),
            );
            prop_assert!(r <= fare && o <= fare - r);
            prop_assert_eq!(r + o + d, fare);
        }

        #[test]
        fn floor_fee_bounded_by_fare(fare in any::<u64>(), bps in 0u16..=10_000) {
            prop_assert!(referrer_fee_floor(bps, fare) <= fare);
        }
    }
}
