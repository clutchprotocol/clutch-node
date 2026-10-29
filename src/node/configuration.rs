use config::{Config, ConfigError, Environment, File};
use dotenv::dotenv;
use serde::Deserialize;
use tracing::info;

use crate::node::aura::PastAuthoritySet;

/// 1,000 transactions per authored block.
///
/// A bound, not a tuned throughput figure. Nothing limited block size before this, so a pool that
/// filled faster than it drained produced one ever-larger block, and the flat `tx_fee` was the
/// only thing making that cost anything. Well above any volume this chain has seen, and low
/// enough that one block stays a sane size to serialise and gossip.
///
/// NOT a consensus value, so nodes may disagree on it without forking — see
/// `Blockchain::with_max_block_transactions`. It is absent from `ChainInit` on purpose.
fn default_max_block_transactions() -> usize {
    1_000
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub log_level: String,
    pub libp2p_topic_name: String,
    pub blockchain_name: String,
    pub author_public_key: String,
    pub author_secret_key: String,
    pub developer_mode: bool,
    pub websocket_addr: String,
    pub authorities: Vec<String>,
    /// The authority sets this chain had before `authorities`, oldest first, as `[[previous_authorities]]`
    /// tables with `until_block` (the last block that set authored) and `authorities`. Add one each
    /// time the set is replaced, with the set that is being retired: without it a node that is behind
    /// the rotation, or new, cannot sync past it. A TOML array of tables must come after every plain
    /// key, so put it at the end of the file. Empty is a chain whose set never changed.
    #[serde(default)]
    pub previous_authorities: Vec<PastAuthoritySet>,
    pub listen_addrs: Vec<String>,
    pub bootstrap_nodes: Vec<String>,
    pub block_authoring_enabled: bool,
    /// How many transactions this node puts in a block it authors. Local policy, not consensus:
    /// it does not belong in `ChainInit` and does not have to match across nodes. Defaulted so an
    /// existing deployment picks up the bound without a config edit.
    #[serde(default = "default_max_block_transactions")]
    pub max_block_transactions: usize,
    pub chain_id: u64,
    pub is_testnet: bool,
    pub tx_fee: u64,
    pub mint_authority: String,
    pub faucet_address: String,
    pub faucet_allocation: u64,
    /// Further addresses allowed to sign a Mint, beyond mint_authority. Genesis-committed, so it
    /// must be byte-identical on every node of a chain. Empty is single-signer.
    #[serde(default)]
    pub mint_cosigners: Vec<String>,
    /// Signatures a Mint requires. 0 and 1 both mean single-signer. Genesis-committed.
    #[serde(default)]
    pub mint_threshold: u8,
    /// Seconds after a RideAcceptance before the held fare stops being the rider to reclaim and a
    /// cancel pays the driver instead. 0 disables it. Genesis-committed, so every node of a chain
    /// must carry the same value.
    #[serde(default)]
    pub ride_auto_release_secs: u64,
    pub ride_request_referrer_fee_bps: u16,
    pub ride_offer_referrer_fee_bps: u16,
    /// The EIP-155 chain id wallets sign with (the Hub API's `wallet_chain_id`). With
    /// `wallet_transfers_from_block`, it turns on transfers signed by MetaMask and other Ethereum
    /// wallets. Both unset: refused. A consensus rule held in config, so every validator of a chain
    /// must carry the same two values; see `WalletTransferRule`.
    #[serde(default)]
    pub wallet_chain_id: Option<u64>,
    /// The first block height that may carry a wallet transfer. Set it ahead of the chain's
    /// height and roll it to every validator before that height is reached.
    #[serde(default)]
    pub wallet_transfers_from_block: Option<u64>,
    pub sync_enabled: bool,
    pub serve_metric_enabled: bool,
    pub serve_metric_addr: String,
    pub seq_url: String,
    pub seq_api_key: String,
}

impl AppConfig {
    /// The wallet transfer rule, when both settings are present. One without the other is a
    /// mistake, and is refused at start rather than half applied.
    pub fn wallet_transfer_rule(
        &self,
    ) -> Result<Option<crate::node::transactions::wallet_transfer::WalletTransferRule>, String> {
        use crate::node::transactions::wallet_transfer::WalletTransferRule;
        match (self.wallet_chain_id, self.wallet_transfers_from_block) {
            (Some(wallet_chain_id), Some(from_block)) => Ok(Some(WalletTransferRule {
                wallet_chain_id,
                from_block,
            })),
            (None, None) => Ok(None),
            _ => Err(
                "set both wallet_chain_id and wallet_transfers_from_block, or neither".to_string(),
            ),
        }
    }

    fn from_env(env: &str) -> Result<Self, ConfigError> {
        dotenv().ok();
        let file_path = format!("config/node/{}.toml", env);
        let builder = Config::builder()
            .add_source(File::with_name(&file_path)) 
            .add_source(Environment::with_prefix("APP"));

        builder.build()?.try_deserialize::<Self>()
    }

    pub fn load_configuration(env: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let config = AppConfig::from_env(env)?; 
        info!("Loaded configuration from env {:?}: {:?}", env, config);
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::FileFormat;

    /// The repository's own node config, with `extra` appended: a TOML array of tables has to come
    /// after every plain key.
    fn parse(extra: &str) -> AppConfig {
        let base = std::fs::read_to_string("config/node/node1.toml").expect("config/node/node1.toml");
        Config::builder()
            .add_source(File::from_str(&format!("{base}\n{extra}\n"), FileFormat::Toml))
            .build()
            .expect("the config builds")
            .try_deserialize::<AppConfig>()
            .expect("the config deserializes")
    }

    #[test]
    fn previous_authorities_default_to_none() {
        assert!(parse("").previous_authorities.is_empty());
    }

    #[test]
    fn previous_authorities_read_as_tables_oldest_first() {
        let cfg = parse(
            "[[previous_authorities]]\nuntil_block = 10\nauthorities = [\"0xaa\", \"0xbb\"]\n\n\
             [[previous_authorities]]\nuntil_block = 20\nauthorities = [\"0xcc\"]\n",
        );
        assert_eq!(cfg.previous_authorities.len(), 2);
        assert_eq!(cfg.previous_authorities[0].until_block, 10);
        assert_eq!(cfg.previous_authorities[0].authorities, vec!["0xaa", "0xbb"]);
        assert_eq!(cfg.previous_authorities[1].until_block, 20);
        assert_eq!(cfg.previous_authorities[1].authorities, vec!["0xcc"]);
    }
}
