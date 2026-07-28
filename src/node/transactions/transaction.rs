use crate::node::{
    account_state::AccountState,
    balance_effect::{BalanceEffectKind, StateUpdate},
    database::Database,
    signature_keys::{self, SignatureKeys},
};

use rlp::RlpStream;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};
use std::vec;

use super::chain_init::ChainInit;
use super::{function_call::FunctionCall, passenger_concurrent};
#[cfg(test)]
use super::transfer::Transfer;

const FROM_GENESIS: &str = "0xGENESIS";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Transaction {
    pub from: String,
    pub data: FunctionCall,
    pub nonce: u64,
    pub chain_id: u64,
    pub signature_r: String,
    pub signature_s: String,
    pub signature_v: i32,
    pub hash: String,
}

impl Transaction {
    pub fn new_transaction(
        from: String,
        nonce: u64,
        chain_id: u64,
        function_call: FunctionCall,
    ) -> Transaction {
        let mut transaction = Transaction {
            hash: String::new(),
            signature_r: String::new(),
            signature_s: String::new(),
            signature_v: 0,
            from: from,
            nonce: nonce,
            chain_id: chain_id,
            data: function_call,
        };
        transaction.hash = transaction.calculate_hash();
        transaction
    }

    pub fn new_genesis_transactions(params: &ChainInit) -> Vec<Transaction> {
        vec![Self::new_transaction(
            FROM_GENESIS.to_string(),
            0,
            params.chain_id,
            FunctionCall::ChainInit(params.clone()),
        )]
    }

    /// Canonical transaction hash. MUST stay byte-for-byte in agreement with the client
    /// hashing in clutch-hub-sdk-js (`signTransaction`) and clutch-hub-api's faucet:
    /// Keccak-256 over RLP `[from (no 0x prefix), nonce, chain_id, data]`. `from` is stripped
    /// of any `0x` because the SDK RLP-encodes it without the prefix; the node's decoder
    /// re-adds the prefix, so it must be removed again here for the hash to match. `chain_id`
    /// makes the signature network-specific — a transaction signed for one chain hashes (and
    /// therefore verifies) differently on any other, closing a replay path across networks.
    fn calculate_hash(&self) -> String {
        let from_no_prefix = self.from.strip_prefix("0x").unwrap_or(&self.from);
        let mut stream = RlpStream::new();
        stream.begin_list(4);
        stream.append(&from_no_prefix.to_string());
        stream.append(&self.nonce);
        stream.append(&self.chain_id);
        stream.append(&self.data);
        let rlp_bytes = stream.out();

        let mut hasher = Keccak256::new();
        hasher.update(&rlp_bytes);
        format!("0x{}", hex::encode(hasher.finalize()))
    }

    /// Rejects a transaction whose `hash` field was not honestly derived from
    /// `(from, nonce, chain_id, data)`. Without this the hash is attacker-controlled and doubles as a
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
    ///
    /// This is the same-block exactly-once backstop for Mint: two Mints sharing a
    /// `credit_ref` in one block are caught here, not by the ref marker (which only
    /// exists in state *after* the block commits — `verify_state` for both sees
    /// pre-block state). Canonicalizing (rather than comparing raw `from` strings)
    /// keeps that guarantee self-contained instead of depending on `SignatureKeys::verify`
    /// happening to reject case-variant signers elsewhere — a distant invariant, not a
    /// nonce-ordering nicety.
    fn first_duplicate_sender(transactions: &[Transaction]) -> Option<String> {
        use super::address::canonical_account_address;
        let mut seen = std::collections::HashSet::new();
        for tx in transactions {
            if !seen.insert(canonical_account_address(&tx.from)) {
                return Some(tx.from.clone());
            }
        }
        None
    }

    pub fn validate_transaction(&self, db: &Database) -> Result<(), String> {
        self.verify_hash()?;
        self.verify_signature()?;
        let params = ChainInit::get(db)?;
        if self.chain_id != params.chain_id {
            return Err(format!(
                "Verification failed: transaction chain_id {} does not match chain {}",
                self.chain_id, params.chain_id
            ));
        }
        if !self.fee_exempt() {
            let required = self
                .sender_direct_debit()
                .checked_add(params.tx_fee)
                .ok_or("Verification failed: amount + fee overflows u64")?;
            let balance = AccountState::get_current_state(&self.from, db).balance;
            if balance < required {
                return Err(format!(
                    "Verification failed: insufficient balance for amount + fee. Required: {}, available: {}",
                    required, balance
                ));
            }
        }
        self.verify_nonce(db)?;
        self.verify_state(db)?;
        Ok(())
    }

    /// Mint is exempt: the treasury authority mints TO users and may itself hold zero
    /// balance. ChainInit is genesis-only. Everything else pays the flat fee.
    fn fee_exempt(&self) -> bool {
        matches!(&self.data, FunctionCall::Mint(_) | FunctionCall::ChainInit(_))
    }

    /// CLT the sender's balance is directly debited by this tx (excluding the fee).
    fn sender_direct_debit(&self) -> u64 {
        match &self.data {
            FunctionCall::Transfer(t) => t.value,
            // Task 7 adds: FunctionCall::Burn(b) => b.amount,
            _ => 0,
        }
    }

    /// ponytail: author's own tx nets zero fee — a debit and an aggregate credit on the
    /// same account in one block collide in the deferred batch (last write wins), so we
    /// skip both sides instead. Lift with incremental intra-block state.
    pub fn effective_fee(&self, block_author: &str, params: &ChainInit) -> u64 {
        use crate::node::transactions::address::canonical_account_address;
        if self.fee_exempt()
            || canonical_account_address(&self.from) == canonical_account_address(block_author)
        {
            0
        } else {
            params.tx_fee
        }
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
            FunctionCall::Mint(mint) => mint.verify_state(&self.from, db),
            FunctionCall::RideRequestCancel(ride_request_cancel) => {
                ride_request_cancel.verify_state(&self.from, db)
            }
            FunctionCall::ChainInit(chain_init) => chain_init.verify_state(&self.from, db),
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
            FunctionCall::Mint(_) => "Mint",
            FunctionCall::RideRequestCancel(_) => "RideRequestCancel",
            FunctionCall::ChainInit(_) => "ChainInit",
        }
    }

    pub fn state_transaction(
        &self,
        db: &Database,
        params: &ChainInit,
        block_author: &str,
    ) -> Vec<StateUpdate> {
        let fee = self.effective_fee(block_author, params);
        let mut states = match &self.data {
            FunctionCall::Transfer(transfer) => transfer.state_transaction(&self.from, db, fee),
            FunctionCall::RideRequest(ride_request) => {
                ride_request.state_transaction(&self.from, &self.hash, db)
            }
            FunctionCall::RideOffer(ride_offer) => {
                ride_offer.state_transaction(&self.from, &self.hash, db)
            }
            FunctionCall::RideAcceptance(ride_acceptance) => {
                ride_acceptance.state_transaction(&self.from, &self.hash, db, fee)
            }
            FunctionCall::RidePay(ride_pay) => ride_pay.state_transaction(
                &self.hash,
                db,
                params.ride_request_referrer_fee_bps,
                params.ride_offer_referrer_fee_bps,
                &self.from,
                fee,
            ),
            FunctionCall::RideCancel(ride_cancel) => {
                ride_cancel.state_transaction(&self.from, &self.hash, db, fee)
            }
            FunctionCall::Mint(mint) => mint.state_transaction(&self.hash, db),
            FunctionCall::RideRequestCancel(ride_request_cancel) => {
                ride_request_cancel.state_transaction(&self.hash, db)
            }
            FunctionCall::ChainInit(chain_init) => chain_init.state_transaction(db),
        };

        // Standalone fee debit ONLY for types that never write the sender's balance
        // in-type (see routing table). Types that do (Transfer, Burn, RideAcceptance,
        // RideCancel, RidePay) merge the fee themselves — two writes to one account key in
        // a tx collide in the deferred batch (last write wins). RidePay belongs here
        // because the payer can also be the driver or a referrer it credits.
        let fee_handled_in_type = matches!(
            &self.data,
            FunctionCall::Transfer(_)
                | FunctionCall::RideAcceptance(_)
                | FunctionCall::RideCancel(_)
                | FunctionCall::RidePay(_)
            // Task 7 adds: | FunctionCall::Burn(_)
        );
        if fee > 0 && !fee_handled_in_type {
            states.push(AccountState::apply_balance_change(
                &self.from,
                -(fee as i64),
                BalanceEffectKind::TxFeePaid,
                None,
                db,
            ));
        }

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
            2077,
            FunctionCall::Transfer(Transfer {
                to: to.to_string(),
                value: 1,
            }),
        )
    }

    #[test]
    fn hash_commits_to_chain_id() {
        let a = Transaction::new_transaction(
            "0xdeb4cfb63db134698e1879ea24904df074726cc0".to_string(),
            1,
            2077,
            FunctionCall::Transfer(Transfer { to: "0xA".to_string(), value: 10 }),
        );
        let b = Transaction::new_transaction(
            "0xdeb4cfb63db134698e1879ea24904df074726cc0".to_string(),
            1,
            1,
            FunctionCall::Transfer(Transfer { to: "0xA".to_string(), value: 10 }),
        );
        assert_ne!(a.hash, b.hash, "same tx on a different chain must hash differently");
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
    fn first_duplicate_sender_catches_case_variant() {
        // Same-block exactly-once backstop for Mint: a case-variant `from` (e.g. two
        // same-ref mints signed to look like `0xAB...` and `0xab...`) is still the same
        // account canonically, and must be caught here independent of whatever
        // `SignatureKeys::verify` happens to accept. Raw string comparison (the
        // pre-fix behavior) would return `None` for this pair.
        let a = tf("0xABCDEF", 1, "0x1");
        let a_variant = tf("0xabcdef", 1, "0x2");
        assert!(
            Transaction::first_duplicate_sender(&[a, a_variant]).is_some(),
            "case-variant senders must canonicalize to the same account"
        );
    }

    #[test]
    fn accepts_sdk_style_ride_acceptance_hash() {
        // TODO(sdk-v3): no test currently pins the node's hashing against externally-produced
        // (real clutch-hub-sdk-js) bytes. The old pinned fixture predates chain_id and cannot
        // be regenerated until the SDK adds chain_id; this builds an equivalent RideAcceptance
        // via sdk_style_tx instead. Re-pin against real SDK output once the SDK supports chain_id.
        let ride_offer_hash = "ab".repeat(32);
        let mut args = RlpStream::new_list(1);
        args.append(&ride_offer_hash);
        let args = args.out();

        let mut fc = RlpStream::new_list(2);
        fc.append(&3u8); // RideAcceptance
        fc.append_raw(args.as_ref(), 1);

        let tx = sdk_style_tx(
            "9b6e8afff8329743cac73dbef83ca3cbf9a74c20",
            7,
            2077,
            fc.out().as_ref(),
        );
        assert!(
            tx.verify_hash().is_ok(),
            "node rejected an SDK-style RideAcceptance hash: {:?}",
            tx.verify_hash()
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

        let mut unsigned = RlpStream::new_list(4);
        unsigned.append(&from_clean.to_string());
        unsigned.append(&nonce);
        unsigned.append(&2077u64);
        unsigned.append_raw(data_rlp.as_ref(), 1);
        let mut hasher = Keccak256::new();
        hasher.update(unsigned.out().as_ref());
        let hash_hex = hex::encode(hasher.finalize());

        let dummy = "cd".repeat(32);
        let mut full = RlpStream::new_list(8);
        full.append(&from_clean.to_string());
        full.append(&nonce);
        full.append(&2077u64);
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

    /// Builds a full signed tx for a `data` payload the way the SDK will in v3: the hash is
    /// Keccak-256 over the unsigned `[from (no 0x), nonce, chain_id, data]` preimage, so these
    /// bytes are self-consistent by construction.
    fn sdk_style_tx(from_clean: &str, nonce: u64, chain_id: u64, data_rlp: &[u8]) -> Transaction {
        let mut unsigned = RlpStream::new_list(4);
        unsigned.append(&from_clean.to_string());
        unsigned.append(&nonce);
        unsigned.append(&chain_id);
        unsigned.append_raw(data_rlp, 1);
        let mut hasher = Keccak256::new();
        hasher.update(unsigned.out().as_ref());
        let hash_hex = hex::encode(hasher.finalize());

        let dummy = "cd".repeat(32);
        let mut full = RlpStream::new_list(8);
        full.append(&from_clean.to_string());
        full.append(&nonce);
        full.append(&chain_id);
        full.append(&dummy);
        full.append(&dummy);
        full.append(&28u64);
        full.append(&hash_hex);
        full.append_raw(data_rlp, 1);
        crate::node::rlp_encoding::decode(full.out().as_ref()).expect("decode sdk-style tx")
    }

    /// The referrer clutch-hub-api injects (clutch-deploy `config/api/default.toml`) is
    /// canonical `0x…`, but the SDK RLP-encodes it with the prefix stripped and the node's
    /// decoder canonicalizes it back. If `rlp_append` re-emits the prefixed form, the hash
    /// preimage is longer than the wire bytes and every referred ride is rejected with
    /// "Transaction hash mismatch" — as seen on app-stage.clutchprotocol.io.
    const WIRE_REFERRER: &str = "0912514c7cc3eec2b2dab4e1d150c4b5eaee5a6f";
    const WIRE_FROM: &str = "eec2b2dab4e1d150c4b5eaee5a6f091251400384";

    #[test]
    fn accepts_sdk_ride_request_with_injected_referrer() {
        let mut pickup = RlpStream::new_list(2);
        pickup.append(&35.7f64.to_bits());
        pickup.append(&51.4f64.to_bits());
        let pickup = pickup.out();

        let mut dropoff = RlpStream::new_list(2);
        dropoff.append(&35.8f64.to_bits());
        dropoff.append(&51.5f64.to_bits());
        let dropoff = dropoff.out();

        let mut args = RlpStream::new_list(4);
        args.append_raw(pickup.as_ref(), 1);
        args.append_raw(dropoff.as_ref(), 1);
        args.append(&3u64);
        args.append(&WIRE_REFERRER.to_string());
        let args = args.out();

        let mut fc = RlpStream::new_list(2);
        fc.append(&1u8); // RideRequest
        fc.append_raw(args.as_ref(), 1);

        let tx = sdk_style_tx(WIRE_FROM, 4, 2077, fc.out().as_ref());
        assert!(
            tx.verify_hash().is_ok(),
            "node rejected an SDK RideRequest carrying the Hub-API-injected referrer: {:?}",
            tx.verify_hash()
        );
    }

    #[test]
    fn accepts_sdk_ride_offer_with_injected_referrer() {
        let mut args = RlpStream::new_list(3);
        args.append(&"ab".repeat(32));
        args.append(&3u64);
        args.append(&WIRE_REFERRER.to_string());
        let args = args.out();

        let mut fc = RlpStream::new_list(2);
        fc.append(&2u8); // RideOffer
        fc.append_raw(args.as_ref(), 1);

        let tx = sdk_style_tx(WIRE_FROM, 5, 2077, fc.out().as_ref());
        assert!(
            tx.verify_hash().is_ok(),
            "node rejected an SDK RideOffer carrying the Hub-API-injected referrer: {:?}",
            tx.verify_hash()
        );
    }
}
