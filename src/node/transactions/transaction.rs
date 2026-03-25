use crate::node::{
    account_state::AccountState,
    database::Database,
    signature_keys::{self, SignatureKeys},
};

use rlp::RlpStream;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use sha3::Sha3_256;
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
                value: u64::MAX,
            }),
        );

        vec![tx1]
    }

    fn calculate_hash(&self) -> String {
        // Serialize only the unsigned transaction (from, nonce, data) using RLP
        let mut stream = RlpStream::new();
        stream.begin_list(3);
        stream.append(&self.from);
        stream.append(&self.nonce);
        stream.append(&self.data);
        let rlp_bytes = stream.out();

        // Initialize the SHA3-256 hasher
        let mut hasher = Sha3_256::new();
        hasher.update(&rlp_bytes);
        let result = hasher.finalize();

        // Convert the hash result to a hexadecimal string with "0x" prefix
        format!("0x{}", hex::encode(result))
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

        for tx in transactions.iter() {
            tx.validate_transaction(&db)?;
        }

        Ok(())
    }

    pub fn validate_transaction(&self, db: &Database) -> Result<(), String> {
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
            FunctionCall::ConfirmArrival(confirm_arrival) => confirm_arrival.verify_state(db),
            FunctionCall::ComplainArrival(complain_arrival) => complain_arrival.verify_state(db),
        }
    }

    pub fn state_transaction(&self, db: &Database) -> Vec<Option<(Vec<u8>, Vec<u8>)>> {
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
            FunctionCall::RidePay(ride_pay) => ride_pay.state_transaction(&self.hash, db),
            FunctionCall::RideCancel(ride_cancel) => ride_cancel.state_transaction(&self.hash, db),
            FunctionCall::RideRequestCancel(ride_request_cancel) => {
                ride_request_cancel.state_transaction(&self.hash, db)
            }
            FunctionCall::ConfirmArrival(confirm_arrival) => confirm_arrival.state_transaction(db),
            FunctionCall::ComplainArrival(complain_arrival) => {
                complain_arrival.state_transaction(db)
            }
        };

        match AccountState::increase_account_nonce_key(&self.from, db) {
            Ok((nonce_key, nonce_serialized)) => {
                states.push(Some((nonce_key, nonce_serialized)));
            }
            Err(_e) => {
                states.push(None);
            }
        }

        states
    }
}
