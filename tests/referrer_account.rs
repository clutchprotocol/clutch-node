use clutch_node::node::{
    account_state::AccountState,
    database::Database,
    transactions::address::{canonical_account_address, optional_canonical_referrer},
};

const LEGACY_REFERRER: &str = "0912514c7cc3eec2b2dab4e1d150c4b5eaee5a6f";
const CANONICAL_REFERRER: &str = "0x0912514c7cc3eec2b2dab4e1d150c4b5eaee5a6f";

fn referrer_fee_ceiling(percent: u8, fare: u64) -> u64 {
    if percent == 0 || fare == 0 {
        return 0;
    }
    (percent as u64 * fare + 99) / 100
}

#[test]
fn canonical_account_address_adds_prefix() {
    assert_eq!(
        canonical_account_address(LEGACY_REFERRER),
        CANONICAL_REFERRER
    );
}

#[test]
fn optional_canonical_referrer_normalizes() {
    assert_eq!(
        optional_canonical_referrer(LEGACY_REFERRER.to_string()).as_deref(),
        Some(CANONICAL_REFERRER)
    );
    assert!(optional_canonical_referrer(String::new()).is_none());
}

#[test]
fn referrer_fee_ceiling_pays_on_small_fare() {
    assert_eq!(referrer_fee_ceiling(2, 3), 1);
    assert_eq!(referrer_fee_ceiling(2, 0), 0);
}

#[test]
fn legacy_account_balance_readable_via_canonical_address() {
    let db = Database::new_db("clutch-node-test-referrer-legacy-read");
    let legacy_key = format!("account_state_{}", LEGACY_REFERRER);
    let legacy_state = serde_json::json!({
        "public_key": LEGACY_REFERRER,
        "balance": 12u64
    });
    db.put(
        "state",
        legacy_key.as_bytes(),
        legacy_state.to_string().as_bytes(),
    )
    .expect("put legacy account state");

    let state = AccountState::get_current_state(&CANONICAL_REFERRER.to_string(), &db);
    assert_eq!(state.balance, 12);
    assert_eq!(state.public_key, CANONICAL_REFERRER);
}

#[test]
fn update_account_state_writes_canonical_key() {
    let db = Database::new_db("clutch-node-test-referrer-canonical-write");
    let (key, value) = AccountState::update_account_state_key(
        &LEGACY_REFERRER.to_string(),
        5,
        &db,
    );
    db.put("state", &key, &value).expect("put account state");

    let canonical_key = format!("account_state_{}", CANONICAL_REFERRER);
    assert_eq!(String::from_utf8(key).unwrap(), canonical_key);

    let state = AccountState::get_current_state(&CANONICAL_REFERRER.to_string(), &db);
    assert_eq!(state.balance, 5);
}

#[test]
fn merged_referrer_ceiling_fees_for_same_address() {
    let fare = 3u64;
    let request_fee = referrer_fee_ceiling(2, fare);
    let offer_fee = referrer_fee_ceiling(2, fare);
    assert_eq!(request_fee, 1);
    assert_eq!(offer_fee, 1);
    assert_eq!(request_fee + offer_fee, 2);
}
