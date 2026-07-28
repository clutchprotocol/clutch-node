use crate::node::account_state::AccountState;
use crate::node::balance_effect::{BalanceEffectKind, StateUpdate};
use crate::node::database::Database;
use crate::node::transactions::address::{canonical_account_address, is_valid_address};

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transfer {
    pub to: String,
    pub value: u64,
}

impl Transfer {
    pub fn verify_state(&self, from: &String, db: &Database) -> Result<(), String> {
        // Same check Mint makes on its recipient, for the same reason: `to` is written into
        // the state key verbatim, so a malformed one credits `account_state_{garbage}` that
        // no key can spend. Backed CLT is stranded and circulating supply silently drifts
        // below `total_supply`, with no recovery path.
        if !is_valid_address(&self.to) {
            return Err(format!(
                "Error: Transfer 'to' must be a 20-byte-hex address, got '{}'",
                self.to
            ));
        }

        // A self-transfer moves nothing, but state_transaction would write the sender's
        // balance key twice — the merged debit first, then the plain `+value` credit, which
        // wins in the block's deferred batch. The fee vanishes while the block still credits
        // the author, and the `+value` is minted outright. No legitimate meaning: reject.
        if canonical_account_address(&self.to) == canonical_account_address(from) {
            return Err(format!(
                "Error: Transfer 'to' must differ from 'from' (self-transfer): {}",
                canonical_account_address(from)
            ));
        }

        let from_account_state = AccountState::get_current_state(from, db);

        if from_account_state.balance < self.value {
            return Err(format!(
                "Error: Insufficient balance. From: {} Required: {}, Available: {}",
                from, self.value, from_account_state.balance
            ));
        }

        Ok(())
    }

    pub fn state_transaction(&self, from: &String, db: &Database, fee: u64) -> Vec<StateUpdate> {
        let transfer_value: i64 = self.value as i64;
        let to = self.to.clone();

        let mut updates = AccountState::apply_balance_change_with_fee(
            from,
            -transfer_value,
            fee,
            BalanceEffectKind::TransferOut,
            Some(to.clone()),
            db,
        );
        updates.push(AccountState::apply_balance_change(
            &to,
            transfer_value,
            BalanceEffectKind::TransferIn,
            Some(from.clone()),
            db,
        ));
        updates
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
