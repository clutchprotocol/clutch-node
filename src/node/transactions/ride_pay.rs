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

fn referrer_fee_ceiling(percent: u8, fare: u64) -> u64 {
    if percent == 0 || fare == 0 {
        return 0;
    }
    // saturating so an absurd fare can't overflow-panic (debug) or wrap (release).
    ((percent as u64).saturating_mul(fare).saturating_add(99)) / 100
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

        // Cap referrer fees so request + offer can never exceed the fare being paid; the
        // driver gets the remainder. Prevents the `fare - total_deducted` underflow.
        let request_fee = match &request_referrer {
            Some(_) => referrer_fee_ceiling(request_fee_percent, self.fare),
            None => 0,
        };
        let offer_fee = match &offer_referrer {
            Some(_) => referrer_fee_ceiling(offer_fee_percent, self.fare),
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
    use super::{referrer_fee_ceiling, split_fare};

    #[test]
    fn split_fare_never_exceeds_fare() {
        // Normal fares: fees fit, driver gets the rest.
        assert_eq!(split_fare(100, 2, 2), (2, 2, 96));
        // Ceiling overshoot on tiny fare: 2% of 1 rounds to 1 on each side (sum 2 > 1).
        // Capped so the total stays 1 and the driver amount never underflows.
        assert_eq!(split_fare(1, 1, 1), (1, 0, 0));
        // Misconfigured fees summing to > 100%: still capped at the fare.
        assert_eq!(split_fare(10, 8, 8), (8, 2, 0));
        // No referrers: driver gets the whole fare.
        assert_eq!(split_fare(50, 0, 0), (0, 0, 50));
        // Invariant across a range: request + offer + driver == fare, no overflow.
        for fare in [0u64, 1, 2, 3, 100, u64::MAX] {
            let fee = referrer_fee_ceiling(60, fare);
            let (r, o, d) = split_fare(fare, fee, fee);
            assert_eq!(r + o + d, fare, "fare {}", fare);
            assert!(r + o <= fare);
        }
    }

    #[test]
    fn referrer_fee_ceiling_saturates() {
        assert_eq!(referrer_fee_ceiling(0, 100), 0);
        assert_eq!(referrer_fee_ceiling(2, 0), 0);
        assert_eq!(referrer_fee_ceiling(2, 100), 2);
        assert_eq!(referrer_fee_ceiling(2, 1), 1); // ceiling rounds up
        let _ = referrer_fee_ceiling(100, u64::MAX); // must not overflow-panic
    }
}
