use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};

use crate::node::balance_effect::StateUpdate;
use crate::node::blocks::block::Block;
use crate::node::database::Database;

use super::address::{canonical_account_address, is_valid_address};
use super::function_call::FunctionCall;

/// Lets an account name one other key that may sign its ride transactions until a block height,
/// so an app can sign in the background after the wallet approved once.
///
/// The authorization itself must be signed by the account's own key (a wallet), never by a
/// session key. `expires_at_block == 0` revokes. One session key per account: a new
/// authorization replaces the old one.
///
/// What the key may sign is fixed here, not chosen by the user: the ride types only
/// (`allows`). It can never move CLT out with a Transfer or Burn, and it cannot authorize
/// another key. A ride still spends the account's balance (RideAcceptance escrows the fare).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SessionKey {
    pub session_key: String,
    pub expires_at_block: u64,
}

impl SessionKey {
    pub fn state_key(account: &str) -> Vec<u8> {
        format!("session_key_{}", canonical_account_address(account)).into_bytes()
    }

    /// The function calls a session key may sign.
    pub fn allows(call: &FunctionCall) -> bool {
        matches!(
            call,
            FunctionCall::RideRequest(_)
                | FunctionCall::RideOffer(_)
                | FunctionCall::RideAcceptance(_)
                | FunctionCall::RidePay(_)
                | FunctionCall::RideCancel(_)
                | FunctionCall::RideRequestCancel(_)
        )
    }

    /// The session key `account` has authorized and that has not expired at the next block.
    pub fn active_for(account: &str, db: &Database) -> Result<Option<String>, String> {
        let Some(raw) = db.get("state", &Self::state_key(account))? else {
            return Ok(None);
        };
        let text = String::from_utf8(raw).map_err(|_| "Corrupt session key record".to_string())?;
        let Some((key, expires)) = text.split_once(':') else {
            return Ok(None);
        };
        let expires: u64 = expires.parse().map_err(|_| "Corrupt session key record".to_string())?;
        let next_height = match Block::get_latest_block(db)? {
            Some(b) => b.index as u64 + 1,
            None => 0,
        };
        if expires == 0 || expires <= next_height {
            return Ok(None);
        }
        Ok(Some(key.to_string()))
    }

    pub fn verify_state(&self, from: &str) -> Result<(), String> {
        if self.expires_at_block == 0 {
            return Ok(());
        }
        if !is_valid_address(&self.session_key) {
            return Err("Session key is not a valid address.".to_string());
        }
        if canonical_account_address(&self.session_key) == canonical_account_address(from) {
            return Err("The session key must differ from the account itself.".to_string());
        }
        Ok(())
    }

    pub fn state_transaction(&self, from: &str) -> Vec<StateUpdate> {
        let value = if self.expires_at_block == 0 {
            String::new()
        } else {
            format!(
                "{}:{}",
                canonical_account_address(&self.session_key),
                self.expires_at_block
            )
        };
        vec![StateUpdate::storage_only(Self::state_key(from), value.into_bytes())]
    }
}

impl Encodable for SessionKey {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(2);
        stream.append(&self.session_key);
        stream.append(&self.expires_at_block);
    }
}

impl Decodable for SessionKey {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        Ok(SessionKey {
            session_key: rlp.val_at(0)?,
            expires_at_block: rlp.val_at(1)?,
        })
    }
}
