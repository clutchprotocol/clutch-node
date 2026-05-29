use crate::node::database::Database;
use crate::node::transactions::address::canonical_account_address;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BalanceEffectKind {
    TransferOut,
    TransferIn,
    RideAcceptanceDebit,
    RidePayDriverCredit,
    ReferrerRequestFee,
    ReferrerOfferFee,
    RideCancelRefund,
    BlockReward,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BalanceEffect {
    pub address: String,
    pub delta: i64,
    pub kind: BalanceEffectKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterparty: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredBalanceEffect {
    #[serde(flatten)]
    pub effect: BalanceEffect,
    pub block_height: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call_type: Option<String>,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Default)]
pub struct StateUpdate {
    pub storage: Option<(Vec<u8>, Vec<u8>)>,
    pub effect: Option<BalanceEffect>,
}

impl StateUpdate {
    pub fn storage_only(key: Vec<u8>, value: Vec<u8>) -> Self {
        Self {
            storage: Some((key, value)),
            effect: None,
        }
    }

    pub fn from_legacy(item: Option<(Vec<u8>, Vec<u8>)>) -> Self {
        match item {
            Some((k, v)) => Self::storage_only(k, v),
            None => Self::default(),
        }
    }

    pub fn from_legacy_vec(items: Vec<Option<(Vec<u8>, Vec<u8>)>>) -> Vec<Self> {
        items.into_iter().map(Self::from_legacy).collect()
    }
}

pub fn tx_effects_key(tx_hash: &str) -> Vec<u8> {
    format!("tx_effects_{}", tx_hash).into_bytes()
}

pub fn block_effects_key(block_height: u64) -> Vec<u8> {
    format!("block_effects_{}", block_height).into_bytes()
}

fn account_effect_index_key(
    address: &str,
    block_height: u64,
    tx_index: Option<u32>,
    seq: u8,
) -> Vec<u8> {
    let canonical = canonical_account_address(address);
    let sort_block = u64::MAX.saturating_sub(block_height);
    match tx_index {
        Some(idx) => format!(
            "account_effect_{}_{:020}_{:04}_{:02}",
            canonical, sort_block, idx, seq
        ),
        None => format!(
            "account_effect_{}_{:020}_block_{:02}",
            canonical, sort_block, seq
        ),
    }
    .into_bytes()
}

pub fn persist_tx_effects(
    tx_hash: &str,
    block_height: u64,
    tx_index: u32,
    timestamp: u64,
    function_call_type: &str,
    effects: &[BalanceEffect],
) -> Vec<(Vec<u8>, Vec<u8>)> {
    if effects.is_empty() {
        return Vec::new();
    }

    let stored: Vec<StoredBalanceEffect> = effects
        .iter()
        .enumerate()
        .map(|(seq, effect)| StoredBalanceEffect {
            effect: effect.clone(),
            block_height,
            tx_hash: Some(tx_hash.to_string()),
            tx_index: Some(tx_index),
            function_call_type: Some(function_call_type.to_string()),
            timestamp,
        })
        .collect();

    let mut writes = Vec::new();
    let tx_key = tx_effects_key(tx_hash);
    let tx_value = serde_json::to_string(&stored).unwrap().into_bytes();
    writes.push((tx_key, tx_value));

    for (seq, effect) in effects.iter().enumerate() {
        let index_key = account_effect_index_key(
            &effect.address,
            block_height,
            Some(tx_index),
            seq as u8,
        );
        let index_value = serde_json::to_string(&StoredBalanceEffect {
            effect: effect.clone(),
            block_height,
            tx_hash: Some(tx_hash.to_string()),
            tx_index: Some(tx_index),
            function_call_type: Some(function_call_type.to_string()),
            timestamp,
        })
        .unwrap()
        .into_bytes();
        writes.push((index_key, index_value));
    }

    writes
}

pub fn persist_block_effects(
    block_height: u64,
    timestamp: u64,
    effects: &[BalanceEffect],
) -> Vec<(Vec<u8>, Vec<u8>)> {
    if effects.is_empty() {
        return Vec::new();
    }

    let stored: Vec<StoredBalanceEffect> = effects
        .iter()
        .enumerate()
        .map(|(_seq, effect)| StoredBalanceEffect {
            effect: effect.clone(),
            block_height,
            tx_hash: None,
            tx_index: None,
            function_call_type: None,
            timestamp,
        })
        .collect();

    let mut writes = Vec::new();
    let block_key = block_effects_key(block_height);
    let block_value = serde_json::to_string(&stored).unwrap().into_bytes();
    writes.push((block_key, block_value));

    for (seq, effect) in effects.iter().enumerate() {
        let index_key =
            account_effect_index_key(&effect.address, block_height, None, seq as u8);
        let index_value = serde_json::to_string(&stored[seq]).unwrap().into_bytes();
        writes.push((index_key, index_value));
    }

    writes
}

pub fn load_tx_effects(db: &Database, tx_hash: &str) -> Vec<StoredBalanceEffect> {
    let key = tx_effects_key(tx_hash);
    match db.get("state", &key) {
        Ok(Some(value)) => serde_json::from_slice(&value).unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn load_block_effects(db: &Database, block_height: u64) -> Vec<StoredBalanceEffect> {
    let key = block_effects_key(block_height);
    match db.get("state", &key) {
        Ok(Some(value)) => serde_json::from_slice(&value).unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn get_account_balance_effects(
    db: &Database,
    address: &str,
    limit: usize,
    offset: usize,
) -> Vec<StoredBalanceEffect> {
    let canonical = canonical_account_address(address);
    let prefix = format!("account_effect_{}_", canonical);
    let entries = db
        .prefix_scan("state", prefix.as_bytes())
        .unwrap_or_default();

    entries
        .into_iter()
        .filter_map(|(_, value)| serde_json::from_slice::<StoredBalanceEffect>(&value).ok())
        .skip(offset)
        .take(limit)
        .collect()
}
