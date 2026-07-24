use crate::node::{
    account_state::AccountState,
    balance_effect::StateUpdate,
    database::Database,
    signature_keys::{self, SignatureKeys},
};

use rlp::RlpStream;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};
use std::vec;

use super::{function_call::FunctionCall, passenger_concurrent, transfer::Transfer};

const FROM_GENESIS: &str = "0xGENESIS";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Transaction {
    pub from: String,
    pub data: FunctionCall,
    pub nonce: u64,
    pub signature_r: String,
    pub signature_s: String,
    pub signature_v: i32,
    pub hash: String,
}

impl Transaction {
    pub fn new_transaction(from: String, nonce: u64, function_call: FunctionCall) -> Transaction {
        let mut transaction = Transaction {
            hash: String::new(),
            signature_r: String::new(),
            signature_s: String::new(),
            signature_v: 0,
            from: from,
            nonce: nonce,
            data: function_call,
        };
        transaction.hash = transaction.calculate_hash();
        transaction
    }

    pub fn new_genesis_transactions() -> Vec<Transaction> {
        let tx1 = Self::new_transaction(
            FROM_GENESIS.to_string(),
            0,
            FunctionCall::Transfer(Transfer {
                to: "0xdeb4cfb63db134698e1879ea24904df074726cc0".to_string(),
                // ponytail: i64::MAX, not u64::MAX. Balance deltas travel as i64
                // (transfer.rs `value as i64`), so funding u64::MAX only ever worked by
                // two's-complement wrap. i64::MAX (~9.2e18) is still effectively infinite
                // for a testnet faucet and keeps every balance representable in i64.
                value: i64::MAX as u64,
            }),
        );

        vec![tx1]
    }

    /// Canonical transaction hash. MUST stay byte-for-byte in agreement with the client
    /// hashing in clutch-hub-sdk-js (`signTransaction`) and clutch-hub-api's faucet:
    /// Keccak-256 over RLP `[from (no 0x prefix), nonce, data]`. `from` is stripped of any
    /// `0x` because the SDK RLP-encodes it without the prefix; the node's decoder re-adds the
    /// prefix, so it must be removed again here for the hash to match.
    fn calculate_hash(&self) -> String {
        let from_no_prefix = self.from.strip_prefix("0x").unwrap_or(&self.from);
        let mut stream = RlpStream::new();
        stream.begin_list(3);
        stream.append(&from_no_prefix.to_string());
        stream.append(&self.nonce);
        stream.append(&self.data);
        let rlp_bytes = stream.out();

        let mut hasher = Keccak256::new();
        hasher.update(&rlp_bytes);
        format!("0x{}", hex::encode(hasher.finalize()))
    }

    /// Rejects a transaction whose `hash` field was not honestly derived from
    /// `(from, nonce, data)`. Without this the hash is attacker-controlled and doubles as a
    /// storage key (`ride_request_{hash}`, etc.), letting a caller collide/shadow another
    /// ride's state. Comparison is 0x- and case-insensitive because the wire hash arrives
    /// without a `0x` prefix while node-built hashes carry one.
    fn verify_hash(&self) -> Result<(), String> {
        let claimed = self.hash.strip_prefix("0x").unwrap_or(&self.hash).to_lowercase();
        let computed = self.calculate_hash();
        let computed = computed.strip_prefix("0x").unwrap_or(&computed).to_lowercase();
        if claimed != computed {
            return Err(format!(
                "Transaction hash mismatch: claimed '{}', expected '{}'",
                self.hash, computed
            ));
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn sign(&mut self, secret_key: &str) {
        let hash_bytes = self.hash.as_bytes();
        let (r, s, v) = signature_keys::SignatureKeys::sign(secret_key, hash_bytes);

        self.signature_r = r;
        self.signature_s = s;
        self.signature_v = v;
    }

    fn verify_signature(&self) -> Result<(), String> {
        let from_address = &self.from;
        let data = self.hash.as_bytes();
        let r = &self.signature_r;
        let s = &self.signature_s;
        let v = self.signature_v;

        match SignatureKeys::verify(from_address, data, r, s, v) {
            Ok(true) => Ok(()),
            Ok(false) => Err(
                "Verification failed: transaction signature does not match the from address"
                    .to_string(),
            ),
            Err(e) => Err(e),
        }
    }

    pub fn validate_transactions(
        db: &Database,
        transactions: &Vec<Transaction>,
    ) -> Result<(), String> {
        if transactions.is_empty() {
            return Err("No transactions to validate.".to_string());
        }

        // Reject a block carrying more than one transaction from the same account. Block
        // state is validated against, and applied to, one deferred RocksDB batch that only
        // commits at the end of `add_block_to_chain` — so a second tx from the same account
        // both validates and applies against the *pre-block* balance/nonce. Two Transfers
        // from one account would each read the full pre-block balance, both debit it, and
        // the last-write-wins batch collapses the two debits into one while both credits
        // land — minting CLT. Until intra-block state is applied incrementally, one tx per
        // account per block is the safe ceiling (the author drains the rest into later
        // blocks; see `Blockchain::one_tx_per_sender`).
        // ponytail: lift this cap once per-tx state is visible to the next tx in the block.
        if let Some(dup) = Self::first_duplicate_sender(transactions) {
            return Err(format!(
                "Block contains multiple transactions from the same account '{}'; only one per block is allowed.",
                dup
            ));
        }

        for tx in transactions.iter() {
            tx.validate_transaction(&db)?;
        }

        Ok(())
    }

    /// First account that appears more than once in `transactions`, if any. Reads only
    /// `from`, so it's pure/DB-free and unit-testable.
    fn first_duplicate_sender(transactions: &[Transaction]) -> Option<String> {
        let mut seen = std::collections::HashSet::new();
        for tx in transactions {
            if !seen.insert(tx.from.as_str()) {
                return Some(tx.from.clone());
            }
        }
        None
    }

    pub fn validate_transaction(&self, db: &Database) -> Result<(), String> {
        self.verify_hash()?;
        self.verify_signature()?;
        self.verify_nonce(db)?;
        self.verify_state(db)?;

        Ok(())
    }

    fn verify_nonce(&self, db: &Database) -> Result<bool, String> {
        match AccountState::get_current_nonce(&self.from, db) {
            Ok(last_nonce) => {
                let nonce = self.nonce;
                if nonce != last_nonce + 1 {
                    return Err(format!(
                        "Verification failed: Incorrect nonce for transaction from '{}'. Expected: {}, got: {}.",
                        self.from, last_nonce + 1, nonce
                    ));
                }
                Ok(true)
            }
            Err(e) => Err(format!(
                "Verification failed: Unable to retrieve nonce for transaction from '{}'. Error: {}",
                self.from, e
            )),
        }
    }

    fn verify_state(&self, db: &Database) -> Result<(), String> {
        match &self.data {
            FunctionCall::Transfer(transfer) => transfer.verify_state(&self.from, db),
            FunctionCall::RideRequest(ride_request) => {
                ride_request.verify_state(&self.from, db)?;
                if passenger_concurrent::passenger_has_concurrent_request(db, &self.from)? {
                    return Err(
                        "Passenger already has an active ride request. Cancel or complete it before requesting a new ride."
                            .to_string(),
                    );
                }
                Ok(())
            }
            FunctionCall::RideOffer(ride_offer) => ride_offer.verify_state(db),
            FunctionCall::RideAcceptance(ride_acceptance) => {
                ride_acceptance.verify_state(&self.from, db)
            }
            FunctionCall::RidePay(ride_pay) => ride_pay.verify_state(&self.from, db),
            FunctionCall::RideCancel(ride_cancel) => ride_cancel.verify_state(&self.from, db),
            FunctionCall::RideRequestCancel(ride_request_cancel) => {
                ride_request_cancel.verify_state(&self.from, db)
            }
        }
    }

    pub fn function_call_type(&self) -> &'static str {
        match &self.data {
            FunctionCall::Transfer(_) => "Transfer",
            FunctionCall::RideRequest(_) => "RideRequest",
            FunctionCall::RideOffer(_) => "RideOffer",
            FunctionCall::RideAcceptance(_) => "RideAcceptance",
            FunctionCall::RidePay(_) => "RidePay",
            FunctionCall::RideCancel(_) => "RideCancel",
            FunctionCall::RideRequestCancel(_) => "RideRequestCancel",
        }
    }

    pub fn state_transaction(
        &self,
        db: &Database,
        ride_request_referrer_fee_percent: u8,
        ride_offer_referrer_fee_percent: u8,
    ) -> Vec<StateUpdate> {
        let mut states = match &self.data {
            FunctionCall::Transfer(transfer) => transfer.state_transaction(&self.from, db),
            FunctionCall::RideRequest(ride_request) => {
                ride_request.state_transaction(&self.from, &self.hash, db)
            }
            FunctionCall::RideOffer(ride_offer) => {
                ride_offer.state_transaction(&self.from, &self.hash, db)
            }
            FunctionCall::RideAcceptance(ride_acceptance) => {
                ride_acceptance.state_transaction(&self.from, &self.hash, db)
            }
            FunctionCall::RidePay(ride_pay) => ride_pay.state_transaction(
                &self.hash,
                db,
                ride_request_referrer_fee_percent,
                ride_offer_referrer_fee_percent,
                &self.from,
            ),
            FunctionCall::RideCancel(ride_cancel) => ride_cancel.state_transaction(&self.hash, db),
            FunctionCall::RideRequestCancel(ride_request_cancel) => {
                ride_request_cancel.state_transaction(&self.hash, db)
            }
        };

        match AccountState::increase_account_nonce_key(&self.from, db) {
            Ok((nonce_key, nonce_serialized)) => {
                states.push(StateUpdate::storage_only(nonce_key, nonce_serialized));
            }
            Err(_e) => {}
        }

        states
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tf(from: &str, nonce: u64, to: &str) -> Transaction {
        Transaction::new_transaction(
            from.to_string(),
            nonce,
            FunctionCall::Transfer(Transfer {
                to: to.to_string(),
                value: 1,
            }),
        )
    }

    #[test]
    fn first_duplicate_sender_detects_repeat() {
        let a = tf("0xA", 1, "0xB");
        let b = tf("0xB", 1, "0xA");
        // Distinct senders: allowed.
        assert_eq!(
            Transaction::first_duplicate_sender(&[a.clone(), b.clone()]),
            None
        );
        // The mint vector: two txs from 0xA (same nonce, different recipient) — caught.
        let a2 = tf("0xA", 1, "0xC");
        assert_eq!(
            Transaction::first_duplicate_sender(&[a, a2, b]),
            Some("0xA".to_string())
        );
    }

    #[test]
    fn accepts_sdk_generated_ride_acceptance_hash() {
        // Fixture: real clutch-hub-sdk-js signTransaction() output for a RideAcceptance.
        // Pins node calculate_hash byte-for-byte against the SDK's Keccak/RLP encoding.
        let raw = "f90138a83962366538616666663833323937343363616337336462656638336361336362663961373463323007b84034616630393332613765356263356435643662313065643333613638353861376437333330306131333536646664316137643733333936323932366132613366b840333634383362373936323562326566613037333337616633666638393430623163393936666133663463633035636533656535366434666433323136636436651cb84062643737323039366235663965313038333437316339313137633564653736336363623131666334386535333562366439633631336263306662323763393862f84503f842b84061626162616261626162616261626162616261626162616261626162616261626162616261626162616261626162616261626162616261626162616261626162";
        let bytes = hex::decode(raw).expect("fixture hex");
        let tx: Transaction =
            crate::node::rlp_encoding::decode(&bytes).expect("decode SDK tx");
        assert!(
            tx.verify_hash().is_ok(),
            "node rejected a hash the SDK actually produced: {:?}",
            tx.verify_hash()
        );
        assert_eq!(
            tx.hash.strip_prefix("0x").unwrap_or(&tx.hash),
            "bd772096b5f9e1083471c9117c5de763ccb11fc48e535b6d9c613bc0fb27c98b"
        );
    }

    #[test]
    fn accepts_faucet_style_transfer_hash() {
        // Rebuild a Transfer exactly as clutch-hub-api's faucet does (Rust rlp + Keccak),
        // then confirm the node recomputes the same hash. Guards the faucet money path.
        let from_clean = "9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
        let nonce: u64 = 3;
        let to = "0x8f19077627cde4848b090c53c83b12956837d5e9";
        let value: u64 = 100;

        let mut transfer = RlpStream::new_list(2);
        transfer.append(&to.to_string());
        transfer.append(&value);
        let transfer_out = transfer.out();

        let mut fc = RlpStream::new_list(2);
        fc.append(&0u8);
        fc.append_raw(transfer_out.as_ref(), 1);
        let data_rlp = fc.out();

        let mut unsigned = RlpStream::new_list(3);
        unsigned.append(&from_clean.to_string());
        unsigned.append(&nonce);
        unsigned.append_raw(data_rlp.as_ref(), 1);
        let mut hasher = Keccak256::new();
        hasher.update(unsigned.out().as_ref());
        let hash_hex = hex::encode(hasher.finalize());

        let dummy = "cd".repeat(32);
        let mut full = RlpStream::new_list(7);
        full.append(&from_clean.to_string());
        full.append(&nonce);
        full.append(&dummy);
        full.append(&dummy);
        full.append(&28u64);
        full.append(&hash_hex);
        full.append_raw(data_rlp.as_ref(), 1);
        let raw = full.out();

        let tx: Transaction =
            crate::node::rlp_encoding::decode(raw.as_ref()).expect("decode faucet-style tx");
        assert!(
            tx.verify_hash().is_ok(),
            "node rejected a faucet-style Transfer hash: {:?}",
            tx.verify_hash()
        );
    }
}
