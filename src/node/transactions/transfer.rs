use crate::node::account_state::AccountState;
use crate::node::balance_effect::{BalanceEffectKind, StateUpdate};
use crate::node::database::Database;

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transfer {
    pub to: String,
    pub value: u64,
}

impl Transfer {
    pub fn verify_state(&self, from: &String, db: &Database) -> Result<(), String> {
        let from_account_state = AccountState::get_current_state(from, db);

        if from_account_state.balance < self.value {
            return Err(format!(
                "Error: Insufficient balance. From: {} Required: {}, Available: {}",
                from, self.value, from_account_state.balance
            ));
        }

        Ok(())
    }

    pub fn state_transaction(&self, from: &String, db: &Database) -> Vec<StateUpdate> {
        let transfer_value: i64 = self.value as i64;
        let to = self.to.clone();

        vec![
            AccountState::apply_balance_change(
                from,
                -transfer_value,
                BalanceEffectKind::TransferOut,
                Some(to.clone()),
                db,
            ),
            AccountState::apply_balance_change(
                &to,
                transfer_value,
                BalanceEffectKind::TransferIn,
                Some(from.clone()),
                db,
            ),
        ]
    }
}

impl Encodable for Transfer {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(2);
        stream.append(&self.to);
        stream.append(&self.value);
    }
}

impl Decodable for Transfer {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }

        Ok(Transfer {
            to: rlp.val_at(0)?,
            value: rlp.val_at(1)?,
        })
    }
}
