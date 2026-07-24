use tracing::{error, info};

use super::blocks::block::Block;
use super::configuration::AppConfig;
use super::consensus::Consensus;
use super::p2p_server::handshake::Handshake;
use super::transactions::transaction_pool::TransactionPool;
use crate::node::account_state::AccountState;
use crate::node::aura::Aura;
use crate::node::balance_effect::{get_account_balance_effects, load_block_effects, load_tx_effects, StoredBalanceEffect};
use crate::node::database::Database;
use crate::node::file_utils::write_to_file;
use crate::node::node_services::NodeServices;
use crate::node::transactions::ride_acceptance::{AvailableActiveTrip, AvailableRecentTrip, RideAcceptance};
use crate::node::transactions::ride_offer::{AvailableRideOffer, RideOffer};
use crate::node::transactions::ride_request::{AvailableRideRequest, MapBounds, RideRequest};
use crate::node::transactions::transaction::Transaction;

pub struct Blockchain {
    pub name: String,
    db: Database,
    developer_mode: bool,
    consensus: Aura,
    author_public_key: String,
    author_secret_key: String,
    block_reward_amount: u64,
    ride_request_referrer_fee_percent: u8,
    ride_offer_referrer_fee_percent: u8,
}

impl Blockchain {
    pub fn new(
        name: String,
        author_public_key: String,
        author_secret_key: String,
        developer_mode: bool,
        authorities: Vec<String>,
        block_reward_amount: u64,
        ride_request_referrer_fee_percent: u8,
        ride_offer_referrer_fee_percent: u8,
    ) -> Blockchain {
        let db = Database::new_db(&name);
        let step_duration = 60 / authorities.len() as u64;
        let blockchain = Blockchain {
            name,
            db,
            developer_mode,
            consensus: Aura::new(authorities, step_duration),
            author_public_key,
            author_secret_key,
            block_reward_amount,
            ride_request_referrer_fee_percent,
            ride_offer_referrer_fee_percent,
        };

        Block::genesis_import_block(&blockchain.db);
        blockchain
    }

    pub fn get_latest_block(&self) -> Result<Option<Block>, String> {
        Block::get_latest_block(&self.db)
    }

    pub fn get_genesis_block(&self) -> Result<Option<Block>, String> {
        Block::get_genesis_block(&self.db)
    }

    #[allow(dead_code)]
    pub fn get_account_state(&self, public_key: &String) -> AccountState {
        AccountState::get_current_state(public_key, &self.db)
    }

    pub fn get_account_balance(&self, public_key: &String) -> u64 {
        self.get_account_state(public_key).balance
    }

    pub fn get_tx_balance_effects(&self, tx_hash: &str) -> Vec<StoredBalanceEffect> {
        load_tx_effects(&self.db, tx_hash)
    }

    pub fn get_block_balance_effects(&self, block_height: u64) -> Vec<StoredBalanceEffect> {
        load_block_effects(&self.db, block_height)
    }

    pub fn get_account_balance_effects(
        &self,
        address: &str,
        limit: usize,
        offset: usize,
    ) -> Vec<StoredBalanceEffect> {
        get_account_balance_effects(&self.db, address, limit, offset)
    }

    pub fn get_current_nonce(&self, public_key: &String) -> Result<u64, String> {
        AccountState::get_current_nonce(public_key, &self.db)
    }

    pub fn shutdown_blockchain(&mut self) {
        if self.developer_mode {
            self.blockchain_write_to_file();
            self.cleanup_db();
        }
    }

    fn cleanup_db(&mut self) {
        self.db.close();
        match self.db.delete_database(self.name.as_str()) {
            Ok(_) => {
                info!("Developer mode: Database cleaned up successfully.");               
            }
            Err(e) => error!("Error cleaning up database: {}", e),
        }
    }

    pub fn import_block(&self, block: &Block) -> Result<(), String> {
        self.consensus.verify_block_author(&block)?;
        block.validate_block(&self.db)?;
        Transaction::validate_transactions(&self.db, &block.transactions)?;
        Block::add_block_to_chain(
            &self.db,
            block,
            self.block_reward_amount,
            self.ride_request_referrer_fee_percent,
            self.ride_offer_referrer_fee_percent,
        )?;

        Ok(())
    }

    pub fn get_blocks(&self) -> Result<Vec<Block>, String> {
        Block::get_blocks(&self.db)
    }

    pub fn get_blocks_with_limit_and_skip(
        &self,
        start_index: usize,
        skip: usize,
        limit: usize,
    ) -> Result<Vec<Block>, String> {
        Block::get_blocks_with_limit_and_skip(&self.db, start_index, skip, limit)
    }

    pub fn get_blocks_by_indexes(&self, indexes: Vec<usize>) -> Result<Vec<Block>, String> {
        Block::get_blocks_by_indexes(&self.db, indexes)
    }

    pub fn block_reward_amount(&self) -> u64 {
        self.block_reward_amount
    }

    pub fn ride_request_referrer_fee_percent(&self) -> u8 {
        self.ride_request_referrer_fee_percent
    }

    pub fn ride_offer_referrer_fee_percent(&self) -> u8 {
        self.ride_offer_referrer_fee_percent
    }

    #[allow(dead_code)]
    pub fn current_author(&self) -> &String {
        self.consensus.current_author()
    }

    pub fn handshake(&self) -> Result<Handshake, String> {
        let latest_block = self
            .get_latest_block()?
            .ok_or_else(|| "Failed to get latest block".to_string())?;

        let genesis_block = self
            .get_genesis_block()?
            .ok_or_else(|| "Failed to get genesis block".to_string())?;

        Ok(Handshake {
            genesis_block_hash: genesis_block.hash,
            latest_block_hash: latest_block.hash,
            latest_block_index: latest_block.index,
        })
    }

    pub fn add_transaction_to_pool(&self, transaction: &Transaction) -> Result<(), String> {
        transaction.validate_transaction(&self.db)?;
        TransactionPool::add_transaction(&self.db, &transaction)
    }

    pub fn get_transactions_from_pool(&self) -> Result<Vec<Transaction>, String> {
        TransactionPool::get_transactions(&self.db)
    }

    pub fn list_available_ride_requests(&self, bounds: Option<MapBounds>) -> Result<Vec<AvailableRideRequest>, String> {
        RideRequest::list_available_ride_requests(&self.db, bounds)
    }

    pub fn list_ride_offers_for_request(&self, ride_request_tx_hash: Option<&str>) -> Result<Vec<AvailableRideOffer>, String> {
        RideOffer::list_ride_offers_for_request(&self.db, ride_request_tx_hash)
    }

    pub fn list_active_trips(
        &self,
        driver_address: Option<&str>,
        passenger_address: Option<&str>,
    ) -> Result<Vec<AvailableActiveTrip>, String> {
        RideAcceptance::list_active_trips(&self.db, driver_address, passenger_address)
    }

    pub fn list_completed_trips(
        &self,
        driver_address: Option<&str>,
        passenger_address: Option<&str>,
    ) -> Result<Vec<AvailableActiveTrip>, String> {
        RideAcceptance::list_completed_trips(&self.db, driver_address, passenger_address)
    }

    pub fn list_recent_trips(
        &self,
        driver_address: Option<&str>,
        passenger_address: Option<&str>,
    ) -> Result<Vec<AvailableRecentTrip>, String> {
        RideAcceptance::list_recent_trips(&self.db, driver_address, passenger_address)
    }

    pub fn author_new_block(&self) -> Result<Block, String> {
        let latest_block = match self.get_latest_block()? {
            Some(block) => block,
            None => return Err("Failed to get the latest block in author_new_block".to_string()),
        };

        let index = latest_block.index + 1;
        let previous_hash = latest_block.hash;
        let transactions = match TransactionPool::get_transactions(&self.db) {
            Ok(transactions) => Self::one_tx_per_sender(transactions),
            Err(e) => return Err(format!("Failed to get transactions from pool: {}", e)),
        };

        let mut new_block = Block::new_block(index, previous_hash, transactions);
        new_block.sign(&self.author_public_key, &self.author_secret_key);
        self.import_block(&new_block)?;
        Ok(new_block)
    }

    /// Keep at most one pending tx per sender — the lowest nonce, tie-broken by hash for
    /// determinism — so an authored block never contains two txs from the same account.
    /// `Transaction::validate_transactions` rejects such blocks (deferred-batch staleness
    /// mints CLT); without this the author would keep drafting a block the pool makes
    /// invalid and never make progress. Extra same-account txs stay in the pool for later
    /// blocks. ponytail: one tx/account/block; lift with incremental intra-block state.
    fn one_tx_per_sender(mut transactions: Vec<Transaction>) -> Vec<Transaction> {
        transactions.sort_by(|a, b| a.nonce.cmp(&b.nonce).then_with(|| a.hash.cmp(&b.hash)));
        let mut seen = std::collections::HashSet::new();
        transactions.retain(|tx| seen.insert(tx.from.clone()));
        transactions
    }

    pub async fn start_network_services(self, config: &AppConfig) {
        NodeServices::start_services(config, self).await;
    }

    fn blockchain_write_to_file(&mut self) {
        match self.get_blocks() {
            Ok(blocks) => match serde_json::to_string_pretty(&blocks) {
                Ok(json_str) => {
                    let file_name = format!("{}_blockchain_blocks", &self.name);
                    if let Err(e) = write_to_file(&json_str, &file_name) {
                        error!("{}", e);
                    }
                }
                Err(e) => error!("Failed to serialize blocks: {}", e),
            },
            Err(e) => error!("Failed to retrieve blocks: {}", e),
        }

        match self.get_transactions_from_pool() {
            Ok(transactions) => match serde_json::to_string_pretty(&transactions) {
                Ok(json_str) => {
                    let file_name = format!("{}_tx_pool", &self.name);
                    if let Err(e) = write_to_file(&json_str, &file_name) {
                        error!("{}", e);
                    }
                }
                Err(e) => error!("Failed to serialize transactions: {}", e),
            },
            Err(e) => error!("Failed to retrieve transactions in transaction pool: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::transactions::function_call::FunctionCall;
    use crate::node::transactions::transfer::Transfer;

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
    fn one_tx_per_sender_keeps_lowest_nonce() {
        let kept = Blockchain::one_tx_per_sender(vec![
            tf("0xA", 2, "0xC"),
            tf("0xB", 5, "0xA"),
            tf("0xA", 1, "0xB"),
        ]);
        assert_eq!(kept.len(), 2);
        let a = kept.iter().find(|t| t.from == "0xA").unwrap();
        assert_eq!(a.nonce, 1, "lowest-nonce tx kept per sender");
        assert!(kept.iter().any(|t| t.from == "0xB"));
    }

    #[test]
    fn one_tx_per_sender_collapses_duplicate_nonce_mint_vector() {
        // Same account, same nonce, different recipients — the double-spend/mint input.
        let kept = Blockchain::one_tx_per_sender(vec![tf("0xA", 1, "0xB"), tf("0xA", 1, "0xC")]);
        assert_eq!(kept.len(), 1, "only one tx per sender survives block building");
    }
}
