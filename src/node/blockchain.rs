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
use crate::node::transactions::address::is_valid_address;
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

/// Check an authority set against the arithmetic Aura builds on it.
///
/// Two of these are latent panics rather than misconfigurations, because both the slot duration
/// (`60 / len`) and the slot-to-author mapping (`authorities[slot % len]`) divide by that length:
///
/// - **Empty**: `60 / 0` panics while constructing the chain, with a divide-by-zero and no hint
///   about which value was wrong.
/// - **More than 60**: the slot duration truncates to `0`, the node starts and peers fine, and
///   then `slot_at_time` divides by zero the first time it computes a slot. A panic after a
///   successful boot is much worse to diagnose than one during it.
/// - **Duplicates**: not a panic. The repeated authority quietly takes two slots per round, which
///   reads as bad luck in block distribution rather than as a typo in a config file.
///
/// This is a hard ceiling on validator-set size that nothing else states, so it matters when the
/// set grows beyond the three of the current testnet.
/// Check the mint authority set against the threshold configured with it.
///
/// A threshold nothing can satisfy is the dangerous direction: minting would be permanently
/// impossible, and because `mint_threshold` is genesis-committed the only fix is a new chain.
/// Catching it at boot costs a restart; catching it after launch costs the chain.
///
/// Only the multi-signature fields are validated. `mint_authority` itself is deliberately left
/// alone, because this runs on every boot of an already-running chain and a new check on an
/// existing value could refuse to start a node that has been fine for months.
fn validate_mint_authority_set(chain_init: &ChainInit) -> Result<(), String> {
    if !chain_init.uses_multisig_mint() {
        if !chain_init.mint_cosigners.is_empty() {
            return Err(format!(
                "{} mint cosigner(s) configured with mint_threshold {}; the cosigners would never \
                 be consulted, which reads as multi-signature minting while being single-signer",
                chain_init.mint_cosigners.len(),
                chain_init.mint_threshold
            ));
        }
        return Ok(());
    }

    for c in &chain_init.mint_cosigners {
        if !is_valid_address(c) {
            return Err(format!(
                "mint cosigner {c:?} is not a 20-byte hex address; no signature could ever \
                 recover to it, so it would silently never count toward the threshold"
            ));
        }
    }

    let set_size = chain_init.mint_authority_set().len();
    let threshold = chain_init.effective_mint_threshold();
    if threshold > set_size {
        return Err(format!(
            "mint_threshold {threshold} exceeds the {set_size} distinct authorities available; \
             no Mint could ever be authorised and the field is genesis-committed"
        ));
    }
    Ok(())
}

fn validate_authorities(authorities: &[String]) -> Result<(), String> {
    if authorities.is_empty() {
        return Err("the authority set is empty; there would be no slot to author in".to_string());
    }
    // 60 is the numerator of the step-duration division, so past it the duration truncates to zero.
    if authorities.len() > 60 {
        return Err(format!(
            "{} authorities exceeds the maximum of 60: step duration is `60 / len` seconds, which              truncates to 0 above that and makes every slot calculation divide by zero",
            authorities.len()
        ));
    }
    let mut seen = std::collections::HashSet::with_capacity(authorities.len());
    for a in authorities {
        let key = a.trim().to_lowercase();
        if key.is_empty() {
            return Err("an authority entry is empty".to_string());
        }
        if !seen.insert(key) {
            return Err(format!(
                "authority {a} appears more than once; it would take two slots per round"
            ));
        }
    }
    Ok(())
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

        // The authority set has to survive the arithmetic built on it before anything else runs.
        validate_authorities(&authorities).unwrap_or_else(|e| panic!("invalid authority set: {e}"));
        validate_mint_authority_set(&chain_init)
            .unwrap_or_else(|e| panic!("invalid mint authority set: {e}"));

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

    /// The nonce a caller should put on its NEXT transaction.
    ///
    /// Not simply `confirmed + 1`. A transaction sitting in the pool has not moved the confirmed
    /// nonce yet, so a caller that submits twice in quick succession — which is ordinary, and is
    /// exactly what the treasury's outbox does when it has several mints to send — got the same
    /// answer both times and signed two transactions with one nonce. The first to be mined wins and
    /// the second becomes permanently invalid.
    ///
    /// That is not a lost transaction, it is a stopped chain: an invalid transaction cannot leave
    /// the pool, and every candidate block carries the whole pool. Stage halted at block 84 on
    /// 2026-09-14 this way.
    ///
    /// Walking upward from the confirmed nonce also fills gaps rather than skipping past them: if
    /// the pool holds N+1 and N+3, the answer is N+2, which is the transaction that would let both
    /// of the others become valid.
    pub fn get_next_nonce(&self, public_key: &String) -> Result<u64, String> {
        Self::next_nonce_for(&self.db, public_key)
    }

    /// Split from the method so it can be tested against a scratch database, like the pool filters
    /// above, rather than needing a whole Blockchain.
    fn next_nonce_for(db: &Database, public_key: &String) -> Result<u64, String> {
        use crate::node::transactions::address::canonical_account_address;

        let confirmed = AccountState::get_current_nonce(public_key, db)?;
        let sender = canonical_account_address(public_key);

        // A pool read failure must not silently downgrade this to `confirmed + 1`: that is the
        // colliding answer this function exists to stop giving.
        let pooled = TransactionPool::get_transactions(db)
            .map_err(|e| format!("could not read the transaction pool for {}: {}", public_key, e))?;

        let taken: std::collections::HashSet<u64> = pooled
            .iter()
            .filter(|tx| canonical_account_address(&tx.from) == sender)
            .map(|tx| tx.nonce)
            .collect();

        let mut next = confirmed + 1;
        while taken.contains(&next) {
            next += 1;
        }
        Ok(next)
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
            // Evict first: a transaction that can never be valid would otherwise be carried into
            // the candidate block and fail the whole thing, every second, for ever.
            Ok(transactions) => Self::drop_intra_block_conflicts(
                &self.db,
                Self::evict_permanently_invalid(&self.db, transactions),
            ),
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
    /// Transactions that can never become valid again, removed from the pool rather than retried
    /// until the end of time.
    ///
    /// A pooled transaction is deleted only by the import of a block carrying it. So a transaction
    /// that fails validation is never included, never imported, and never removed — and because
    /// `author_new_block` puts the whole pool into its candidate block, one such transaction makes
    /// every block fail validation. The chain stops. That is not hypothetical: stage halted at
    /// block 84 on 2026-09-14 behind a single mint whose nonce the chain had already consumed, and
    /// it stayed there for hours.
    ///
    /// Only the provably-permanent case is dropped. An account's nonce never decreases, so
    /// `nonce <= last` can never satisfy `nonce == last + 1` again. A nonce ABOVE the next one is
    /// a gap, which the transaction that fills it may still close, so those are left alone.
    ///
    /// A nonce that cannot be read is not evidence of anything and the transaction is kept.
    fn evict_permanently_invalid(db: &Database, transactions: Vec<Transaction>) -> Vec<Transaction> {
        transactions
            .into_iter()
            .filter(|tx| {
                match AccountState::get_current_nonce(&tx.from, db) {
                    Ok(last) if tx.nonce <= last => {
                        warn!(
                            "evicting permanently invalid transaction {} from {}: nonce {}, chain is already at {}",
                            tx.hash, tx.from, tx.nonce, last
                        );
                        if let Err(e) = TransactionPool::remove_transaction(db, &tx.hash) {
                            error!("could not evict {} from the pool: {}", tx.hash, e);
                        }
                        false
                    }
                    _ => true,
                }
            })
            .collect()
    }

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
    /// The two cases here are latent panics, not preferences: both the slot duration and the
    /// slot-to-author mapping divide by the authority count.
    #[test]
    fn authority_set_edges_are_refused_before_the_arithmetic_panics() {
        let a = |n: usize| -> Vec<String> { (0..n).map(|i| format!("0x{i:040x}")).collect() };

        assert!(super::validate_authorities(&a(1)).is_ok(), "one authority is legal");
        assert!(super::validate_authorities(&a(3)).is_ok(), "the current testnet set is legal");
        assert!(super::validate_authorities(&a(60)).is_ok(), "60 is the last legal size");

        // `60 / 0` — panics while constructing the chain.
        let empty = super::validate_authorities(&[]).expect_err("empty must be refused");
        assert!(empty.contains("empty"), "got: {empty}");

        // step_duration truncates to 0, then every slot calculation divides by zero AFTER boot.
        let too_many = super::validate_authorities(&a(61)).expect_err("61 must be refused");
        assert!(too_many.contains("60"), "the message must name the ceiling, got: {too_many}");
    }

    #[test]
    fn a_repeated_authority_is_refused() {
        let dup = vec![
            "0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            // Same authority, different case — it would still take two slots per round.
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        ];
        let err = super::validate_authorities(&dup).expect_err("a duplicate must be refused");
        assert!(err.contains("more than once"), "got: {err}");

        let blank = vec!["0xaaa".to_string(), "   ".to_string()];
        assert!(super::validate_authorities(&blank).is_err(), "a blank entry must be refused");
    }

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
    /// Write an account's nonce the way the chain stores it: big-endian u64 under
    /// `account_nonce_<canonical>` in the `state` column family. There is no setter on
    /// `AccountState` -- nonces only ever move by applying a transaction -- so the test writes the
    /// same bytes `increase_account_nonce_key` would have produced.
    fn seed_nonce(db: &Database, address: &str, nonce: u64) {
        use crate::node::transactions::address::canonical_account_address;
        let key = format!("account_nonce_{}", canonical_account_address(address)).into_bytes();
        let value = nonce.to_be_bytes().to_vec();
        db.write(vec![("state", key.as_slice(), Some(value.as_slice()))])
            .expect("seed nonce");
    }

    fn scratch_db(name: &str) -> Database {
        let _ = std::fs::remove_dir_all(format!("{}.db", name));
        Database::new_db(name)
    }

    fn drop_scratch(mut db: Database, name: &str) {
        db.close();
        db.delete_database(name).ok();
    }

    #[test]
    fn next_nonce_skips_what_is_already_queued() {
        // The outbox case that halted stage: two mints sent in one pass. Before this, both were
        // told nonce 1, both were signed with it, and the second could never become valid.
        let name = "clutch-node-test-nonce-queued";
        let db = scratch_db(name);
        seed_nonce(&db, "0xA", 0);

        let first = Blockchain::next_nonce_for(&db, &"0xA".to_string()).unwrap();
        TransactionPool::add_transaction(&db, &tf("0xA", first, "0xC")).unwrap();
        let second = Blockchain::next_nonce_for(&db, &"0xA".to_string()).unwrap();

        drop_scratch(db, name);
        assert_eq!(first, 1);
        assert_eq!(second, 2, "a queued nonce must not be handed out twice");
    }

    #[test]
    fn next_nonce_fills_a_gap_rather_than_stepping_over_it() {
        // Pool holds 1 and 3. The useful answer is 2 -- the transaction that lets both of the
        // others become valid -- not 4, which would leave 3 stranded for ever.
        let name = "clutch-node-test-nonce-gap";
        let db = scratch_db(name);
        seed_nonce(&db, "0xA", 0);
        TransactionPool::add_transaction(&db, &tf("0xA", 1, "0xC")).unwrap();
        TransactionPool::add_transaction(&db, &tf("0xA", 3, "0xD")).unwrap();

        let next = Blockchain::next_nonce_for(&db, &"0xA".to_string()).unwrap();
        drop_scratch(db, name);
        assert_eq!(next, 2);
    }

    #[test]
    fn next_nonce_ignores_other_senders() {
        let name = "clutch-node-test-nonce-other";
        let db = scratch_db(name);
        seed_nonce(&db, "0xA", 4);
        TransactionPool::add_transaction(&db, &tf("0xB", 5, "0xC")).unwrap();

        let next = Blockchain::next_nonce_for(&db, &"0xA".to_string()).unwrap();
        drop_scratch(db, name);
        assert_eq!(next, 5, "another account's queue says nothing about this one");
    }

    #[test]
    fn evicts_a_nonce_the_chain_has_already_consumed() {
        // The stage halt of 2026-09-14 in miniature: a transaction whose nonce the account has
        // already used. Kept in the pool it joins every candidate block, fails validation, and
        // takes the block with it -- so the chain stops and cannot restart, because the only thing
        // that removes a pooled transaction is the import of a block carrying it.
        let name = "clutch-node-test-evict-stale";
        let db = scratch_db(name);

        // Account nonce 5 on chain; a pooled transaction still carrying 3.
        seed_nonce(&db, "0xA", 5);
        let kept = Blockchain::evict_permanently_invalid(&db, vec![tf("0xA", 3, "0xC")]);
        drop_scratch(db, name);

        assert!(kept.is_empty(), "a consumed nonce can never be valid again");
    }

    #[test]
    fn keeps_a_nonce_gap_because_it_may_still_be_filled() {
        // Above the next expected nonce is NOT permanently invalid: the transaction that closes
        // the gap may yet arrive. Evicting these would drop good work on a busy pool.
        let name = "clutch-node-test-evict-gap";
        let db = scratch_db(name);

        seed_nonce(&db, "0xA", 5);
        let kept = Blockchain::evict_permanently_invalid(&db, vec![tf("0xA", 9, "0xC")]);
        drop_scratch(db, name);

        assert_eq!(kept.len(), 1, "a gap may close; only the past is permanent");
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

#[cfg(test)]
mod mint_authority_set_tests {
    use super::validate_mint_authority_set;
    use crate::node::transactions::chain_init::ChainInit;

    const A: &str = "0x00000000000000000000000000000000000000aa";
    const B: &str = "0x00000000000000000000000000000000000000bb";
    const C: &str = "0x00000000000000000000000000000000000000cc";

    fn params(cosigners: Vec<String>, threshold: u8) -> ChainInit {
        ChainInit {
            chain_id: 2077,
            is_testnet: true,
            tx_fee: 1000,
            ride_request_referrer_fee_bps: 200,
            ride_offer_referrer_fee_bps: 200,
            mint_authority: A.to_string(),
            faucet_address: "0x0000000000000000000000000000000000000000".to_string(),
            faucet_allocation: 0,
            mint_cosigners: cosigners,
            mint_threshold: threshold,
            ride_auto_release_secs: 0,
        }
    }

    #[test]
    fn a_single_signer_chain_is_valid() {
        assert!(validate_mint_authority_set(&params(vec![], 0)).is_ok());
        assert!(validate_mint_authority_set(&params(vec![], 1)).is_ok());
    }

    #[test]
    fn a_two_of_three_is_valid() {
        assert!(validate_mint_authority_set(&params(vec![B.to_string(), C.to_string()], 2)).is_ok());
        assert!(validate_mint_authority_set(&params(vec![B.to_string(), C.to_string()], 3)).is_ok());
    }

    /// The dangerous direction. mint_threshold is genesis-committed, so a threshold nothing can
    /// satisfy means minting is impossible for the life of the chain.
    #[test]
    fn a_threshold_above_the_set_size_is_refused() {
        let err = validate_mint_authority_set(&params(vec![B.to_string()], 3)).unwrap_err();
        assert!(err.contains("exceeds the 2 distinct authorities"), "{err}");
    }

    #[test]
    fn cosigners_that_would_never_be_consulted_are_refused() {
        let err = validate_mint_authority_set(&params(vec![B.to_string()], 1)).unwrap_err();
        assert!(err.contains("never be consulted"), "{err}");
    }

    /// No signature can ever recover to a malformed address, so it would silently never count
    /// toward the threshold — a 2-of-3 that is really a 2-of-2.
    #[test]
    fn a_malformed_cosigner_address_is_refused() {
        let err = validate_mint_authority_set(&params(vec!["not-an-address".to_string()], 2))
            .unwrap_err();
        assert!(err.contains("not a 20-byte hex address"), "{err}");
    }

    #[test]
    fn duplicate_cosigners_shrink_the_set_and_can_fail_the_threshold() {
        // A and B, with B listed twice: two distinct authorities, so a 3-of-N is impossible.
        let p = params(vec![B.to_string(), B.to_uppercase()], 3);
        assert!(validate_mint_authority_set(&p).is_err());
    }
}
