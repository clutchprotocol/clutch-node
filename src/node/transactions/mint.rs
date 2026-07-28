use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::node::account_state::AccountState;
use crate::node::balance_effect::{BalanceEffectKind, StateUpdate};
use crate::node::database::Database;

use super::address::canonical_account_address;
use super::chain_init::ChainInit;

/// Exactly-once ref marker: `processed_ref_{64-hex}` in the state CF, value = tx hash.
/// Shared by Mint (credit_ref) and Burn (redemption_ref) — refs are keccak256 hashes of
/// treasury intent ids, so one namespace cannot collide across the two uses.
pub fn processed_ref_key(reference: &str) -> Vec<u8> {
    format!("processed_ref_{}", reference).into_bytes()
}

pub fn ref_is_valid(reference: &str) -> bool {
    reference.len() == 64 && reference.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

pub fn ref_already_processed(db: &Database, reference: &str) -> Result<bool, String> {
    match db.get("state", &processed_ref_key(reference)) {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(false),
        Err(e) => Err(format!("failed to read processed ref: {}", e)),
    }
}

/// 20-byte-hex address, optional `0x`/`0X` prefix — same shape `canonical_account_address`
/// normalizes elsewhere. A malformed `to` here would mint into a garbage state key,
/// permanently inflating `total_supply` against CLT nobody can spend.
fn is_valid_address(addr: &str) -> bool {
    let hex_part = addr
        .strip_prefix("0x")
        .or_else(|| addr.strip_prefix("0X"))
        .unwrap_or(addr);
    hex_part.len() == 40 && hex_part.chars().all(|c| c.is_ascii_hexdigit())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mint {
    pub to: String,
    pub amount: u64,
    pub credit_ref: String,
}

impl Mint {
    pub fn verify_state(&self, from: &String, db: &Database) -> Result<(), String> {
        let params = ChainInit::get(db)?;
        if canonical_account_address(from) != canonical_account_address(&params.mint_authority) {
            return Err(format!(
                "Mint rejected: '{}' is not the mint authority",
                from
            ));
        }
        if !is_valid_address(&self.to) {
            return Err(format!(
                "Mint rejected: 'to' must be a 20-byte-hex address, got '{}'",
                self.to
            ));
        }
        if self.amount == 0 {
            return Err("Mint rejected: amount must be positive".to_string());
        }
        if self.amount > i64::MAX as u64 {
            return Err(
                "Mint rejected: amount exceeds i64::MAX (balance deltas are i64)".to_string(),
            );
        }
        if !ref_is_valid(&self.credit_ref) {
            return Err("Mint rejected: credit_ref must be 64 lowercase hex chars".to_string());
        }
        if ref_already_processed(db, &self.credit_ref)? {
            return Err(format!(
                "Mint rejected: credit_ref '{}' already processed (exactly-once)",
                self.credit_ref
            ));
        }
        Ok(())
    }

    pub fn state_transaction(&self, tx_hash: &String, db: &Database) -> Vec<StateUpdate> {
        vec![
            AccountState::apply_balance_change(
                &self.to,
                self.amount as i64,
                BalanceEffectKind::Mint,
                None,
                db,
            ),
            StateUpdate::storage_only(
                processed_ref_key(&self.credit_ref),
                tx_hash.clone().into_bytes(),
            ),
        ]
    }
}

impl Encodable for Mint {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(3);
        stream.append(&self.to);
        stream.append(&self.amount);
        stream.append(&self.credit_ref);
    }
}

impl Decodable for Mint {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 3 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        Ok(Mint {
            to: rlp.val_at(0)?,
            amount: rlp.val_at(1)?,
            credit_ref: rlp.val_at(2)?,
        })
    }
}
