use tracing::{error, info, warn};

use super::blocks::block::Block;
use super::metric;
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
use crate::node::transactions::chain_init::ChainInit;
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
    chain_init: ChainInit,
    max_block_transactions: usize,
}

impl Blockchain {
    pub fn new(
        name: String,
        author_public_key: String,
        author_secret_key: String,
        developer_mode: bool,
        authorities: Vec<String>,
        chain_init: ChainInit,
    ) -> Blockchain {
        // Fail loudly at boot on inconsistent economics — spec §4.5. Genesis must never
        // be importable with a mainnet flag and a faucet pre-mint.
        assert!(
            chain_init.ride_request_referrer_fee_bps as u32
                + chain_init.ride_offer_referrer_fee_bps as u32
                <= 10_000,
            "referrer fee bps sum exceeds 100%"
        );
        assert!(
            chain_init.faucet_allocation <= i64::MAX as u64,
            "faucet_allocation exceeds i64::MAX (balance deltas are i64)"
        );
        assert!(
            chain_init.is_testnet || chain_init.faucet_allocation == 0,
            "non-testnet chain must have zero faucet_allocation (a surviving faucet pre-mint destroys the peg)"
        );

        let db = Database::new_db(&name);
        let step_duration = 60 / authorities.len() as u64;
        let blockchain = Blockchain {
            name,
            db,
            developer_mode,
            consensus: Aura::new(authorities, step_duration),
            author_public_key,
            author_secret_key,
            chain_init,
            // Uncapped by default, so a caller that never sets one behaves exactly as before.
            max_block_transactions: usize::MAX,
        };

        Block::genesis_import_block(&blockchain.db, &blockchain.chain_init);

        // A DB from before this release has a genesis block but no chain_params state key
        // (genesis_import_block no-ops when a genesis block already exists). Every later
        // add_block_to_chain would then fail quietly, forever. Fail loudly at boot instead.
        if let Err(e) = ChainInit::get(&blockchain.db) {
            panic!(
                "chain_params missing from state after genesis import ({}); this database predates \
                 the ChainInit release and must be wiped (delete the DB directory and restart)",
                e
            );
        }

        // Publish the stored height immediately.
        //
        // LATEST_BLOCK_INDEX is otherwise only set by add_block_to_chain, so it reads 0 from process
        // start until the next block arrives -- on a node that is synced and idle, indefinitely.
        // Every dashboard and every probe scraping it saw 0 after a restart and read that as an
        // empty chain; it is how a node holding 24,000 blocks reported height 0 while its database
        // sat there at several megabytes.
        match Block::get_latest_block(&blockchain.db) {
            // Braced to discard the return: Gauge::set hands back the PREVIOUS value, so bare arms
            // here are an i64 next to the error arm's unit and will not compile.
            Ok(Some(b)) => {
                metric::LATEST_BLOCK_INDEX.set(b.index as i64);
            }
            // No block yet is genuinely 0. A read FAILURE is not, so it is left alone rather than
            // published as an empty chain.
            Ok(None) => {
                metric::LATEST_BLOCK_INDEX.set(0);
            }
            Err(e) => error!("could not publish the stored block height at startup: {e}"),
        }

        blockchain
    }

    /// Consensus params + total supply, read from state (post-genesis truth).
    pub fn get_chain_info(&self) -> Result<(ChainInit, u64), String> {
        let params = ChainInit::get(&self.db)?;
        let supply = ChainInit::get_total_supply(&self.db)?;
        Ok((params, supply))
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
        if !self.developer_mode {
            return;
        }
        self.blockchain_write_to_file();

        // developer_mode DELETES the database. That is fine for a scratch chain in a working
        // directory and catastrophic for one on a mounted volume, so DB_PATH is treated as the
        // signal that this data is meant to outlive the process.
        //
        // Stage ran with developer_mode = true and per-node volumes for a month. Every deploy
        // erased the chain of whichever node finished its graceful stop inside the grace period,
        // while the ones SIGKILLed first survived -- so the loss moved between nodes and was
        // blamed on the volumes, on resyncing, and on the deploy script in turn. A node that
        // deletes durable storage because a boolean says so should at least say no.
        if let Ok(path) = std::env::var("DB_PATH") {
            warn!(
                "developer_mode is set but DB_PATH={path} points at durable storage. REFUSING to delete the database. Unset DB_PATH for a throwaway chain, or set developer_mode = false for a real one."
            );
            return;
        }
        self.cleanup_db();
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
        Block::add_block_to_chain(&self.db, block)?;

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

    /// Cap on how many transactions this node puts in a block **it authors**.
    ///
    /// Authoring only, and deliberately so. `Block::validate_block` checks the signature, the
    /// index and the previous hash — not the transaction count — so a peer accepts whatever size
    /// block it is handed. That makes this local policy rather than a consensus rule: two nodes
    /// can disagree about it and still agree on the chain. Turning it into a consensus rule means
    /// putting it in `ChainInit`, and that is a genesis change, which forks the chain and costs a
    /// reset.
    ///
    /// The ceiling, stated plainly: this bounds the blocks this node PRODUCES, not the ones it
    /// ACCEPTS. A malicious or broken author can still emit an arbitrarily large block and every
    /// node will import it. That is a reasonable trade while the authority set is three nodes the
    /// operator runs. It stops being reasonable the moment that set opens to anyone else, and the
    /// fix at that point is the consensus rule and the reset it costs.
    pub fn with_max_block_transactions(mut self, max: usize) -> Self {
        assert!(max > 0, "max_block_transactions must be at least 1");
        self.max_block_transactions = max;
        self
    }

    pub fn author_new_block(&self) -> Result<Block, String> {
        let latest_block = match self.get_latest_block()? {
            Some(block) => block,
            None => return Err("Failed to get the latest block in author_new_block".to_string()),
        };

        let index = latest_block.index + 1;
        let previous_hash = latest_block.hash.clone();
        let mut transactions = match TransactionPool::get_transactions(&self.db) {
            Ok(transactions) => Self::drop_intra_block_conflicts(&self.db, transactions),
            Err(e) => return Err(format!("Failed to get transactions from pool: {}", e)),
        };

        // Applied AFTER conflict-dropping, so the cap counts transactions that would actually
        // have been included rather than candidates most of which were about to be discarded.
        //
        // The remainder is not dropped: nothing removes a transaction from the pool until a block
        // carrying it is imported, so whatever the cap defers is still there for the next block —
        // and this loop ticks every second while a slot lasts `step_duration` seconds, which is
        // how a busy pool drains across several blocks in one slot.
        //
        // `drop_intra_block_conflicts` has already sorted by nonce, so truncation keeps the
        // oldest work and defers the newest, rather than picking arbitrarily.
        transactions.truncate(self.max_block_transactions);

        // Empty blocks are legal and necessary — confirmation depth is counted in blocks, so a
        // chain that stops producing them when idle can never confirm what is already on it (see
        // `Transaction::validate_transactions` for how that stalled the mint credit path). But
        // they are a heartbeat, not throughput: this loop ticks every second while a slot lasts
        // `step_duration` seconds, so emit at most ONE empty block per slot.
        //
        // Blocks WITH transactions are deliberately NOT rate-limited here — draining a busy pool
        // across several blocks within one slot is how throughput is achieved at all, given the
        // one-tx-per-sender-per-block ceiling.
        if transactions.is_empty() && self.consensus.block_is_in_current_slot(&latest_block) {
            return Err("Nothing to author: this slot already has a block".to_string());
        }

        let mut new_block = Block::new_block(index, previous_hash, transactions);
        new_block.sign(&self.author_public_key, &self.author_secret_key);
        self.import_block(&new_block)?;
        Ok(new_block)
    }

    /// Authoring-time counterpart to the block-level guards in
    /// `Transaction::validate_transactions`: drop pending txs that cannot legally share a
    /// block, keeping at most one per sender (deferred-batch staleness on the balance/nonce
    /// mints CLT), at most one per exactly-once ref (two identical `processed_ref_{ref}`
    /// writes collapse, breaking exactly-once across Mint and Burn), and at most one writer
    /// per account balance (two txs from different senders writing one account collapse the
    /// same way — the Burn reserve drain). Without this the author would keep drafting a block
    /// its own validation rejects and never make progress.
    ///
    /// Ordering is lowest nonce, tie-broken by hash, so every node keeps the same winner;
    /// the losers stay in the pool for a later block.
    /// ponytail: one tx/account/block; lift with incremental intra-block state.
    fn drop_intra_block_conflicts(
        db: &Database,
        mut transactions: Vec<Transaction>,
    ) -> Vec<Transaction> {
        use crate::node::transactions::address::canonical_account_address;
        transactions.sort_by(|a, b| a.nonce.cmp(&b.nonce).then_with(|| a.hash.cmp(&b.hash)));
        let mut senders = std::collections::HashSet::new();
        let mut refs = std::collections::HashSet::new();
        let mut accounts = std::collections::HashSet::new();
        transactions.retain(|tx| {
            // Claim the slots only when the tx is actually kept: a dropped tx that had
            // reserved its sender or its accounts would cascade into dropping innocent txs.
            let sender = canonical_account_address(&tx.from);
            let written = tx.written_accounts(db);
            let keep = !senders.contains(&sender)
                && tx.exactly_once_ref().map_or(true, |r| !refs.contains(r))
                && written.iter().all(|a| !accounts.contains(a));
            if keep {
                senders.insert(sender);
                if let Some(r) = tx.exactly_once_ref() {
                    refs.insert(r.to_string());
                }
                accounts.extend(written);
            }
            keep
        });
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
            2077,
            FunctionCall::Transfer(Transfer {
                to: to.to_string(),
                value: 1,
            }),
        )
    }

    fn burn(from: &str, nonce: u64, redemption_ref: Option<&str>) -> Transaction {
        Transaction::new_transaction(
            from.to_string(),
            nonce,
            2077,
            FunctionCall::Burn(crate::node::transactions::burn::Burn {
                amount: 1,
                redemption_ref: redemption_ref.map(|r| r.to_string()),
            }),
        )
    }

    /// The filter needs a `Database` to resolve RidePay/RideCancel counterparties. None of
    /// these cases reads state, so any empty DB will do — one per test so they can still run
    /// in parallel, deleted at the end so re-runs start clean.
    fn scratch_db(name: &str) -> Database {
        let _ = std::fs::remove_dir_all(format!("{}.db", name));
        Database::new_db(name)
    }

    fn drop_scratch(mut db: Database, name: &str) {
        db.close();
        db.delete_database(name).ok();
    }

    #[test]
    fn drops_extra_tx_per_sender_keeping_lowest_nonce() {
        let name = "clutch-node-test-conflicts-sender";
        let db = scratch_db(name);
        // Recipients are disjoint from every sender: two senders may share a block only if
        // no account is written twice, which is a separate guard exercised below.
        let kept = Blockchain::drop_intra_block_conflicts(
            &db,
            vec![tf("0xA", 2, "0xC"), tf("0xB", 5, "0xD"), tf("0xA", 1, "0xC")],
        );
        drop_scratch(db, name);
        assert_eq!(kept.len(), 2);
        let a = kept.iter().find(|t| t.from == "0xA").unwrap();
        assert_eq!(a.nonce, 1, "lowest-nonce tx kept per sender");
        assert!(kept.iter().any(|t| t.from == "0xB"));
    }

    #[test]
    fn drops_duplicate_nonce_mint_vector() {
        // Same account, same nonce, different recipients — the double-spend/mint input.
        let name = "clutch-node-test-conflicts-nonce";
        let db = scratch_db(name);
        let kept = Blockchain::drop_intra_block_conflicts(
            &db,
            vec![tf("0xA", 1, "0xB"), tf("0xA", 1, "0xC")],
        );
        drop_scratch(db, name);
        assert_eq!(kept.len(), 1, "only one tx per sender survives block building");
    }

    #[test]
    fn drops_second_claim_on_an_exactly_once_ref() {
        // Two *different* senders, so the per-sender filter never fires — but one ref, whose
        // marker write would collapse in the deferred batch. Without this the author drafts
        // a block `validate_transactions` then rejects, and never makes progress.
        let name = "clutch-node-test-conflicts-ref";
        let db = scratch_db(name);
        let r = "a".repeat(64);
        let kept = Blockchain::drop_intra_block_conflicts(
            &db,
            vec![burn("0xA", 1, Some(&r)), burn("0xB", 1, Some(&r))],
        );
        drop_scratch(db, name);
        assert_eq!(kept.len(), 1, "one claim per ref survives block building");
    }

    #[test]
    fn keeps_every_ref_less_burn() {
        // `None` is the absence of a ref, not a shared one — collapsing these would break
        // the plain-burn path. Two burners only ever write their own balances, so the
        // written-account guard must not fire either.
        let name = "clutch-node-test-conflicts-plain-burn";
        let db = scratch_db(name);
        let kept = Blockchain::drop_intra_block_conflicts(
            &db,
            vec![burn("0xA", 1, None), burn("0xB", 1, None)],
        );
        drop_scratch(db, name);
        assert_eq!(kept.len(), 2, "ref-less burns never conflict");
    }

    #[test]
    fn defers_the_second_writer_of_one_account() {
        // The reserve-drain shape, at the authoring layer: a Burn by 0xA and a Transfer TO
        // 0xA from another sender both write `account_state_0xa`. The author must keep one
        // and leave the other pooled, or it drafts a block its own validation rejects.
        let name = "clutch-node-test-conflicts-shared-account";
        let db = scratch_db(name);
        let kept = Blockchain::drop_intra_block_conflicts(
            &db,
            vec![burn("0xA", 1, None), tf("0xB", 2, "0xA")],
        );
        drop_scratch(db, name);
        assert_eq!(kept.len(), 1, "only one writer of 0xA may land");
        assert_eq!(kept[0].from, "0xA", "lowest nonce wins, deterministically");
    }
}
