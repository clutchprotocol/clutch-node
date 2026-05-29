use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize,Serialize};

use crate::node::balance_effect::StateUpdate;
use crate::node::database::Database;


#[derive(Clone, Debug,Serialize,Deserialize)]
pub struct ConfirmArrival{
    pub ride_acceptance_transaction_hash:String,
}

impl ConfirmArrival {
    pub fn verify_state(&self, _db: &Database) -> Result<(), String> {
        Ok(())
    }

    pub fn state_transaction(
        &self,       
        _db: &Database,
    ) -> Vec<StateUpdate> {
        vec![]
    }
}


impl Encodable for ConfirmArrival {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(1);
        stream.append(&self.ride_acceptance_transaction_hash);
    }
}

impl Decodable for ConfirmArrival {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 1 {
            return Err(DecoderError::RlpIncorrectListLen);
        }

        Ok(ConfirmArrival {
            ride_acceptance_transaction_hash: rlp.val_at(0)?,
        })
    }
}
