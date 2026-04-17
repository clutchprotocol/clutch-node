use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{error, info, warn};

use crate::node::database::Database;
use crate::node::time_utils::get_current_timespan;
use crate::node::account_state::AccountState;
use crate::node::transactions::transaction::Transaction;
use crate::node::transactions::transaction_pool::TransactionPool;
use crate::node::{metric, signature_keys};

use super::block_headers::BlockHeader;

#[derive(Debug, Serialize, Deserialize)]
pub struct Block {
    pub index: usize,
    pub timestamp: u64,
    pub previous_hash: String,
    pub author: String,
    pub signature_r: String,
    pub signature_s: String,
    pub signature_v: i32,
    pub hash: String,
    pub transactions: Vec<Transaction>,
}

impl Block {
    fn calculate_hash(&self) -> String {
        let mut hasher = Sha256::new();

        let transactions_hash_string = self
            .transactions
            .iter()
            .map(|tx| format!("{}", tx.hash))
            .collect::<Vec<String>>()
            .join("");

        hasher.update(format!(
            "{}{}{}",
            self.index, self.previous_hash, transactions_hash_string
        ));
        let result = hasher.finalize();
        format!("{:x}", result)
    }

    pub fn new_genesis_block() -> Block {
        let mut genesis_block = Block {
            author: String::new(),
            index: 0,
            timestamp: get_current_timespan(),
            previous_hash: "0".to_string(),
            signature_r: String::new(),
            signature_s: String::new(),
            signature_v: 0,
            hash: String::new(),
            transactions: vec![],
        };

        genesis_block.transactions = Transaction::new_genesis_transactions();
        genesis_block.hash = genesis_block.calculate_hash();
        genesis_block
    }

    pub fn new_block(index: usize, previous_hash: String, transactions: Vec<Transaction>) -> Block {
        let mut block = Block {
            index,
            timestamp: get_current_timespan(),
            previous_hash,
            author: String::new(),
            signature_r: String::new(),
            signature_s: String::new(),
            signature_v: 0,
            hash: String::new(),
            transactions,
        };

        block.hash = block.calculate_hash();
        block
    }

    pub fn sign(&mut self, author: &str, secret_key: &str) {
        let hash_bytes = self.hash.as_bytes();
        let (r, s, v) = signature_keys::SignatureKeys::sign(secret_key, hash_bytes);

        self.signature_r = r;
        self.signature_s = s;
        self.signature_v = v;
        self.author = author.to_string();
    }

    fn verify_signature(&self) -> Result<bool, String> {
        let author = &self.author;
        let data = self.hash.as_bytes();
        let r = &self.signature_r;
        let s = &self.signature_s;
        let v = self.signature_v;

        signature_keys::SignatureKeys::verify(author, data, r, s, v)
    }

    pub fn get_latest_block(db: &Database) -> Option<Block> {
        match db.get("blockchain", b"blockchain_latest_block") {
            Ok(Some(value)) => {
                let block_str = String::from_utf8(value).unwrap();
                let block: Block = serde_json::from_str(&block_str).unwrap();
                Some(block)
            }
            Ok(None) => None,
            Err(_) => panic!("Failed to retrieve the latest block index"),
        }
    }

    pub fn validate_block(&self, db: &Database) -> Result<bool, String> {
        match Block::get_latest_block(db) {
            Some(latest_block) => {
                match self.verify_signature() {
                    Ok(is_verified) => {
                        if !is_verified {
                            return Err(format!(
                                "Verification failed: Signature does not match for block from author: {}",
                                self.author
                            ));
                        }
                    }
                    Err(e) => return Err(format!("Signature verification error: {}", e)),
                }

                if self.index != latest_block.index + 1 {
                    return Err(format!(
                        "Invalid block: The block index should be {}, but it was {}.",
                        latest_block.index + 1,
                        self.index
                    ));
                }

                if self.previous_hash != latest_block.hash {
                    return Err(format!(
                        "Invalid block: The previous hash should be {}, but it was {}.",
                        latest_block.hash, self.previous_hash
                    ));
                }

                Ok(true)
            }
            None => Ok(true),
        }
    }

    pub fn get_blocks(db: &Database) -> Result<Vec<Block>, String> {
        match db.get_keys_values_by_cf_name("block") {
            Ok(entries) => {
                let mut blocks = Vec::new();

                for (_key, value) in entries {
                    match serde_json::from_slice::<Block>(&value) {
                        Ok(block) => {
                            blocks.push(block);
                        }
                        Err(e) => {
                            return Err(format!("Failed to deserialize block: {}", e));
                        }
                    }
                }

                Ok(blocks)
            }
            Err(e) => Err(format!("Failed to retrieve blocks: {}", e)),
        }
    }

    pub fn get_blocks_with_limit_and_skip(
        db: &Database,
        start_index: usize,
        skip: usize,
        limit: usize,
    ) -> Result<Vec<Block>, String> {
        let mut blocks = Vec::new();
        let end_index = start_index + skip + limit;

        for index in start_index + skip..end_index {
            let key = format!("block_{}", index);
            match db.get("block", key.as_bytes()) {
                Ok(Some(value)) => match serde_json::from_slice::<Block>(&value) {
                    Ok(block) => {
                        blocks.push(block);
                    }
                    Err(e) => {
                        return Err(format!("Failed to deserialize block {}: {}", key, e));
                    }
                },
                Ok(None) => {
                    // Stop if we reach a point where no more blocks are found
                    break;
                }
                Err(e) => {
                    return Err(format!("Failed to retrieve block {}: {}", key, e));
                }
            }
        }

        Ok(blocks)
    }

    pub fn get_blocks_by_indexes(db: &Database, indexes: Vec<usize>) -> Result<Vec<Block>, String> {
        let mut blocks = Vec::new();

        for index in indexes {
            let key = format!("block_{}", index);
            match db.get("block", key.as_bytes()) {
                Ok(Some(value)) => match serde_json::from_slice::<Block>(&value) {
                    Ok(block) => {
                        blocks.push(block);
                    }
                    Err(e) => {
                        return Err(format!("Failed to deserialize block {}: {}", key, e));
                    }
                },
                Ok(None) => {
                    return Err(format!("Block {} not found in database", index));
                }
                Err(e) => {
                    return Err(format!("Failed to retrieve block {}: {}", key, e));
                }
            }
        }

        Ok(blocks)
    }

    pub fn state_block(&self) -> Option<(Vec<Vec<u8>>, Vec<Vec<u8>>)> {
        let mut keys: Vec<Vec<u8>> = Vec::new();
        let mut values: Vec<Vec<u8>> = Vec::new();

        //Add block
        let block_key = format!("block_{}", self.index).into_bytes();
        let block_value = serde_json::to_string(self).unwrap().into_bytes();
        keys.push(block_key);
        values.push(block_value);

        Some((keys, values))
    }

    pub fn state_blockchain(&self) -> Option<(Vec<Vec<u8>>, Vec<Vec<u8>>)> {
        let mut keys: Vec<Vec<u8>> = Vec::new();
        let mut values: Vec<Vec<u8>> = Vec::new();

        // Save the latest block index to the blockchain
        let blockchain_latest_block_key = b"blockchain_latest_block";
        let blockchain_latest_block_value = serde_json::to_string(self).unwrap().into_bytes();

        keys.push(blockchain_latest_block_key.to_vec());
        values.push(blockchain_latest_block_value);

        Some((keys, values))
    }

    pub fn genesis_import_block(db: &Database) {
        match Self::get_genesis_block(db) {
            Some(_) => {
                warn!("Genesis block already exists.");
            }
            None => {
                info!("Genesis block does not exist, creating new one...");
                let genesis_block = Self::new_genesis_block();
                Self::add_block_to_chain(db, &genesis_block, 0);
            }
        }
    }

    pub fn get_genesis_block(db: &Database) -> Option<Block> {
        match db.get("block", b"block_0") {
            Ok(Some(value)) => {
                let block_str = String::from_utf8(value).unwrap();
                let block: Block = serde_json::from_str(&block_str).unwrap();
                Some(block)
            }
            Ok(None) => None,
            Err(_) => panic!("Failed to retrieve the genesis block"),
        }
    }

    pub fn add_block_to_chain(db: &Database, block: &Block, block_reward_amount: u64) {
        // Storage for keys and values
        let mut cf_storage: Vec<String> = Vec::new();
        let mut keys_storage: Vec<Vec<u8>> = Vec::new();
        let mut values_storage: Vec<Vec<u8>> = Vec::new();

        let mut operations: Vec<(&str, &[u8], Option<&[u8]>)> = Vec::new();
        let mut tx_keys_to_delete: Vec<Vec<u8>> = Vec::new(); // Store tx keys to delete

        // Handle block state
        if let Some((block_keys, block_values)) = block.state_block() {
            for (key, value) in block_keys.into_iter().zip(block_values.into_iter()) {
                cf_storage.push("block".to_string());
                keys_storage.push(key);
                values_storage.push(value);
            }
        } else {
            error!("Failed to serialize block for storage.");
            return;
        }

        // Handle Blockchain State
        if let Some((block_keys, block_values)) = block.state_blockchain() {
            for (key, value) in block_keys.into_iter().zip(block_values.into_iter()) {
                cf_storage.push("blockchain".to_string());
                keys_storage.push(key);
                values_storage.push(value);
            }
        } else {
            error!("Failed to serialize block for storage.");
            return;
        }

        // Handle transactions State
        for tx in block.transactions.iter() {
            let updates = tx.state_transaction(&db);

            for update in updates {
                if let Some((key, value)) = update {
                    cf_storage.push("state".to_string());
                    keys_storage.push(key);
                    values_storage.push(value);
                }
            }

            // Prepare keys for deletion from tx_pool
            let tx_key = TransactionPool::construct_tx_pool_key(&tx.hash);
            tx_keys_to_delete.push(tx_key);
        }

        // Mint reward for non-genesis block author.
        if block.index > 0 && block_reward_amount > 0 {
            let (author_reward_key, author_reward_value) = AccountState::update_account_state_key(
                &block.author,
                block_reward_amount as i64,
                db,
            );
            cf_storage.push("state".to_string());
            keys_storage.push(author_reward_key);
            values_storage.push(author_reward_value);
        }

        // Prepare operations for database write
        for ((key, value), cf_name) in keys_storage
            .iter()
            .zip(values_storage.iter())
            .zip(cf_storage.iter())
        {
            operations.push((cf_name, key.as_slice(), Some(value.as_slice())));
        }

        // Delete transactions from tx_pool
        for tx_key in tx_keys_to_delete.iter() {
            operations.push(("tx_pool", tx_key.as_slice(), None));
        }

        // Update the database
        match &db.write(operations) {
            Ok(_) => {
                info!(
                    "add_block_to_chain successfully. block hash: {}. block index: {}.",
                    block.hash, block.index
                );

                metric::LATEST_BLOCK_INDEX.set(block.index as i64);

                metric::LATEST_BLOCK.clear();
                metric::LATEST_BLOCK
                    .get_or_create(&metric::BlockLabels {
                        block_hash: block.hash.to_string(),
                    })
                    .set(block.index as i64);
            }
            Err(e) => panic!("Failed add_block_to_chain: {}", e),
        }
    }

    pub fn to_block_header(&self) -> BlockHeader {
        BlockHeader {
            index: self.index,
            previous_hash: self.previous_hash.clone(),
            author: self.author.clone(),
            signature_r: self.signature_r.clone(),
            signature_s: self.signature_s.clone(),
            signature_v: self.signature_v,
            hash: self.hash.clone(),
        }
    }
}
