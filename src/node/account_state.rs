use crate::node::database::Database;
use crate::node::transactions::address::{
    canonical_account_address, legacy_account_address_hex,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct AccountState {
    pub public_key: String,
    pub balance: u64,
}

impl AccountState {
    fn new_account_state(public_key: &str) -> AccountState {
        AccountState {
            public_key: public_key.to_string(),
            balance: 0,
        }
    }

    fn parse_account_state_value(value: Vec<u8>, canonical: &str) -> AccountState {
        let account_state_str = String::from_utf8(value).unwrap();
        let mut account_state: AccountState =
            serde_json::from_str(&account_state_str).unwrap();
        account_state.public_key = canonical.to_string();
        account_state
    }

    fn load_account_state(canonical: &str, db: &Database) -> Option<AccountState> {
        let canonical_key = Self::construct_account_state_key(canonical);
        if let Ok(Some(value)) = db.get("state", &canonical_key) {
            return Some(Self::parse_account_state_value(value, canonical));
        }

        let legacy = legacy_account_address_hex(canonical);
        if legacy == canonical {
            return None;
        }

        let legacy_key = Self::construct_account_state_key(&legacy);
        if let Ok(Some(value)) = db.get("state", &legacy_key) {
            return Some(Self::parse_account_state_value(value, canonical));
        }

        None
    }

    pub fn get_current_state(public_key: &String, db: &Database) -> AccountState {
        let canonical = canonical_account_address(public_key);
        Self::load_account_state(&canonical, db)
            .unwrap_or_else(|| Self::new_account_state(&canonical))
    }

    fn construct_account_state_key(public_key: &str) -> Vec<u8> {
        format!("account_state_{}", public_key).into_bytes()
    }

    pub fn update_account_state_key(
        public_key: &String,
        balance_change: i64,
        db: &Database,
    ) -> (Vec<u8>, Vec<u8>) {
        let canonical = canonical_account_address(public_key);
        let mut account_state = Self::get_current_state(&canonical, db);
        account_state.public_key = canonical.clone();
        account_state.balance =
            (account_state.balance as i64 + balance_change) as u64;

        let key = Self::construct_account_state_key(&canonical);
        let serialized = serde_json::to_string(&account_state)
            .unwrap()
            .into_bytes();
        (key, serialized)
    }

    fn load_nonce(canonical: &str, db: &Database) -> Option<u64> {
        let canonical_key = Self::construct_account_nonce_key(canonical);
        if let Ok(Some(value)) = db.get("state", &canonical_key) {
            return Self::parse_nonce_value(value);
        }

        let legacy = legacy_account_address_hex(canonical);
        if legacy == canonical {
            return None;
        }

        let legacy_key = Self::construct_account_nonce_key(&legacy);
        if let Ok(Some(value)) = db.get("state", &legacy_key) {
            return Self::parse_nonce_value(value);
        }

        None
    }

    fn parse_nonce_value(value: Vec<u8>) -> Option<u64> {
        if value.len() == 8 {
            let bytes_array: [u8; 8] = value.try_into().expect("Slice with incorrect length");
            Some(u64::from_be_bytes(bytes_array))
        } else {
            None
        }
    }

    pub fn get_current_nonce(public_key: &String, db: &Database) -> Result<u64, String> {
        let canonical = canonical_account_address(public_key);
        match Self::load_nonce(&canonical, db) {
            Some(nonce) => Ok(nonce),
            None => Ok(0),
        }
    }

    fn construct_account_nonce_key(public_key: &str) -> Vec<u8> {
        format!("account_nonce_{}", public_key).into_bytes()
    }

    pub fn increase_account_nonce_key(
        public_key: &String,
        db: &Database,
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        let canonical = canonical_account_address(public_key);
        let current_nonce = AccountState::get_current_nonce(public_key, db)?;
        let nonce = current_nonce + 1;
        let account_nonce_key = Self::construct_account_nonce_key(&canonical);
        let account_nonce_serlized = nonce.to_be_bytes().to_vec();
        Ok((account_nonce_key, account_nonce_serlized))
    }
}
