use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::node::account_state::AccountState;
use crate::node::balance_effect::{BalanceEffectKind, StateUpdate};
use crate::node::database::Database;

use super::mint::{processed_ref_key, ref_already_processed, ref_is_valid};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Burn {
    pub amount: u64,
    /// hex(keccak256(intent_id)) for treasury redemptions; None for a plain burn.
    pub redemption_ref: Option<String>,
}

impl Burn {
    pub fn verify_state(&self, _from: &String, db: &Database) -> Result<(), String> {
        if self.amount == 0 {
            return Err("Burn rejected: amount must be positive".to_string());
        }
        if self.amount > i64::MAX as u64 {
            return Err("Burn rejected: amount exceeds i64::MAX".to_string());
        }
        if let Some(r) = &self.redemption_ref {
            if !ref_is_valid(r) {
                return Err("Burn rejected: redemption_ref must be 64 lowercase hex chars".to_string());
            }
            if ref_already_processed(db, r)? {
                return Err(format!(
                    "Burn rejected: redemption_ref '{}' already processed",
                    r
                ));
            }
        }
        // Balance sufficiency (amount + fee) is enforced centrally in validate_transaction.
        Ok(())
    }

    pub fn state_transaction(
        &self,
        from: &String,
        tx_hash: &String,
        db: &Database,
        fee: u64,
    ) -> Vec<StateUpdate> {
        let mut updates = AccountState::apply_balance_change_with_fee(
            from,
            -(self.amount as i64),
            fee,
            BalanceEffectKind::Burn,
            None,
            db,
        );
        if let Some(r) = &self.redemption_ref {
            updates.push(StateUpdate::storage_only(
                processed_ref_key(r),
                tx_hash.clone().into_bytes(),
            ));
        }
        updates
    }
}

impl Encodable for Burn {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(2);
        stream.append(&self.amount);
        // Same optional-string convention as referrers: empty string = None.
        let ref_str = self.redemption_ref.clone().unwrap_or_default();
        stream.append(&ref_str);
    }
}

impl Decodable for Burn {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let ref_str: String = rlp.val_at(1)?;
        Ok(Burn {
            amount: rlp.val_at(0)?,
            redemption_ref: if ref_str.is_empty() { None } else { Some(ref_str) },
        })
    }
}
