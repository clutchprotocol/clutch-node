use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::node::account_state::AccountState;
use crate::node::balance_effect::{BalanceEffectKind, StateUpdate};
use crate::node::database::Database;

pub const CHAIN_PARAMS_KEY: &[u8] = b"chain_params";
pub const TOTAL_SUPPLY_KEY: &[u8] = b"total_supply";

/// Consensus parameters, committed to by the genesis hash: this struct rides in the
/// genesis block's single ChainInit transaction, whose hash feeds the block hash that
/// peers compare at p2p handshake. Runtime reads them from state via `get`, never from
/// per-node config — a node with different values gets a different genesis and cannot
/// peer. This closes the block_reward-style consensus-divergence bug class.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChainInit {
    pub chain_id: u64,
    pub is_testnet: bool,
    pub tx_fee: u64,
    pub ride_request_referrer_fee_bps: u16,
    pub ride_offer_referrer_fee_bps: u16,
    pub mint_authority: String,
    pub faucet_address: String,
    pub faucet_allocation: u64,
}

impl ChainInit {
    pub fn get(db: &Database) -> Result<ChainInit, String> {
        match db.get("state", CHAIN_PARAMS_KEY) {
            Ok(Some(v)) => serde_json::from_slice(&v)
                .map_err(|e| format!("corrupt chain_params in state: {}", e)),
            Ok(None) => Err("chain_params missing from state (genesis not imported?)".to_string()),
            Err(e) => Err(format!("failed to read chain_params: {}", e)),
        }
    }

    pub fn get_total_supply(db: &Database) -> Result<u64, String> {
        match db.get("state", TOTAL_SUPPLY_KEY) {
            Ok(Some(v)) => serde_json::from_slice(&v)
                .map_err(|e| format!("corrupt total_supply in state: {}", e)),
            Ok(None) => Ok(0),
            Err(e) => Err(format!("failed to read total_supply: {}", e)),
        }
    }

    pub fn verify_state(&self, _from: &String, _db: &Database) -> Result<(), String> {
        // Genesis import bypasses validate_transaction entirely, so reaching this check
        // means the tx arrived via the pool or a non-genesis block — always reject.
        Err("ChainInit is only valid in the genesis block".to_string())
    }

    pub fn state_transaction(&self, db: &Database) -> Vec<StateUpdate> {
        let initial_supply = if self.is_testnet { self.faucet_allocation } else { 0 };
        let mut updates = vec![
            StateUpdate::storage_only(
                CHAIN_PARAMS_KEY.to_vec(),
                serde_json::to_vec(self).expect("serialize chain params"),
            ),
            StateUpdate::storage_only(
                TOTAL_SUPPLY_KEY.to_vec(),
                serde_json::to_vec(&initial_supply).expect("serialize supply"),
            ),
        ];
        if initial_supply > 0 {
            // faucet_allocation is validated <= i64::MAX at boot (Blockchain::new).
            updates.push(AccountState::apply_balance_change(
                &self.faucet_address,
                initial_supply as i64,
                BalanceEffectKind::Mint,
                None,
                db,
            ));
        }
        updates
    }
}

impl Encodable for ChainInit {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(8);
        stream.append(&self.chain_id);
        stream.append(&(self.is_testnet as u8));
        stream.append(&self.tx_fee);
        stream.append(&self.ride_request_referrer_fee_bps);
        stream.append(&self.ride_offer_referrer_fee_bps);
        stream.append(&self.mint_authority);
        stream.append(&self.faucet_address);
        stream.append(&self.faucet_allocation);
    }
}

impl Decodable for ChainInit {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 8 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        Ok(ChainInit {
            chain_id: rlp.val_at(0)?,
            is_testnet: rlp.val_at::<u8>(1)? != 0,
            tx_fee: rlp.val_at(2)?,
            ride_request_referrer_fee_bps: rlp.val_at(3)?,
            ride_offer_referrer_fee_bps: rlp.val_at(4)?,
            mint_authority: rlp.val_at(5)?,
            faucet_address: rlp.val_at(6)?,
            faucet_allocation: rlp.val_at(7)?,
        })
    }
}
