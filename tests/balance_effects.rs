use clutch_node::node::{
    account_state::AccountState,
    balance_effect::{
        get_account_balance_effects, load_tx_effects, persist_tx_effects, BalanceEffectKind,
        StateUpdate,
    },
    blocks::block::Block,
    coordinate,
    database::Database,
    transactions::{
        function_call::FunctionCall, ride_acceptance::RideAcceptance, ride_offer::RideOffer,
        ride_pay::RidePay, ride_request::RideRequest, transaction::Transaction,
    },
};
use serial_test::serial;

const REFERRER_FEE_PERCENT: u8 = 2;

const PASSENGER: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
const PASSENGER_SK: &str =
    "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
const DRIVER: &str = "0x8f19077627cde4848b090c53c83b12956837d5e9";
const DRIVER_SK: &str = "e74e3f87268132c7b3ddb24600716fc362f4519bf9986a9436aa8a1be58c7150";
const REFERRER: &str = "0x0912514c7cc3eec2b2dab4e1d150c4b5eaee5a6f";

fn fresh_db() -> Database {
    let name = format!(
        "clutch-node-balance-effects-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    Database::new_db(&name)
}

fn apply_state_updates(db: &Database, updates: Vec<StateUpdate>) {
    for update in updates {
        if let Some((key, value)) = update.storage {
            db.put("state", &key, &value).expect("put state");
        }
    }
}

#[test]
#[serial]
fn ride_pay_emits_referrer_request_fee_effect() {
    let db = fresh_db();
    Block::genesis_import_block(&db);

    let mut ride_request_tx = Transaction::new_transaction(
        PASSENGER.to_string(),
        1,
        FunctionCall::RideRequest(RideRequest {
            fare: 20,
            pickup_location: coordinate::Coordinates {
                latitude: 35.55,
                longitude: 51.23,
            },
            dropoff_location: coordinate::Coordinates {
                latitude: 26.64,
                longitude: 55.85,
            },
            referrer: Some(REFERRER.to_string()),
        }),
    );
    ride_request_tx.sign(PASSENGER_SK);
    let ride_request_hash = ride_request_tx.hash.clone();
    if let FunctionCall::RideRequest(ride_request) = &ride_request_tx.data {
        apply_state_updates(
            &db,
            ride_request.state_transaction(&PASSENGER.to_string(), &ride_request_hash, &db),
        );
    }

    let mut ride_offer_tx = Transaction::new_transaction(
        DRIVER.to_string(),
        1,
        FunctionCall::RideOffer(RideOffer {
            fare: 20,
            ride_request_transaction_hash: ride_request_hash.clone(),
            referrer: None,
        }),
    );
    ride_offer_tx.sign(DRIVER_SK);
    let ride_offer_hash = ride_offer_tx.hash.clone();
    if let FunctionCall::RideOffer(ride_offer) = &ride_offer_tx.data {
        apply_state_updates(
            &db,
            ride_offer.state_transaction(&DRIVER.to_string(), &ride_offer_hash, &db),
        );
    }

    let mut ride_acceptance_tx = Transaction::new_transaction(
        PASSENGER.to_string(),
        2,
        FunctionCall::RideAcceptance(RideAcceptance {
            ride_offer_transaction_hash: ride_offer_hash.clone(),
        }),
    );
    ride_acceptance_tx.sign(PASSENGER_SK);
    let ride_acceptance_hash = ride_acceptance_tx.hash.clone();
    if let FunctionCall::RideAcceptance(ride_acceptance) = &ride_acceptance_tx.data {
        apply_state_updates(
            &db,
            ride_acceptance.state_transaction(&PASSENGER.to_string(), &ride_acceptance_hash, &db),
        );
    }

    let fare = 10u64;
    let ride_pay = RidePay {
        ride_acceptance_transaction_hash: ride_acceptance_hash,
        fare,
    };
    let ride_pay_hash = Transaction::new_transaction(
        PASSENGER.to_string(),
        3,
        FunctionCall::RidePay(ride_pay.clone()),
    )
    .hash;

    let pay_updates = ride_pay.state_transaction(
        &ride_pay_hash,
        &db,
        REFERRER_FEE_PERCENT,
        REFERRER_FEE_PERCENT,
        &PASSENGER.to_string(),
    );
    let mut effects = Vec::new();
    for update in pay_updates {
        if let Some(effect) = update.effect {
            effects.push(effect);
        }
        if let Some((key, value)) = update.storage {
            db.put("state", &key, &value).expect("put state");
        }
    }

    for (key, value) in persist_tx_effects(
        &ride_pay_hash,
        4,
        0,
        1_700_000_000,
        "RidePay",
        &effects,
    ) {
        db.put("state", &key, &value).expect("persist effects");
    }

    let tx_effects = load_tx_effects(&db, &ride_pay_hash);
    let referrer_effects: Vec<_> = tx_effects
        .iter()
        .filter(|e| e.effect.kind == BalanceEffectKind::ReferrerRequestFee)
        .collect();
    assert_eq!(referrer_effects.len(), 1);
    assert_eq!(referrer_effects[0].effect.address, REFERRER);
    assert_eq!(referrer_effects[0].effect.delta, 1);

    let account_effects = get_account_balance_effects(&db, REFERRER, 20, 0);
    assert!(
        account_effects
            .iter()
            .any(|e| e.effect.kind == BalanceEffectKind::ReferrerRequestFee && e.effect.delta == 1),
        "expected referrer_request_fee in account effects"
    );

    assert_eq!(
        AccountState::get_current_state(&REFERRER.to_string(), &db).balance,
        1
    );
}
