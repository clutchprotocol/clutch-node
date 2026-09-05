use config::{Config, ConfigError, Environment, File};
use dotenv::dotenv;
use serde::Deserialize;
use tracing::info;

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
    pub ride_request_referrer_fee_bps: u16,
    pub ride_offer_referrer_fee_bps: u16,
    pub sync_enabled: bool,
    pub serve_metric_enabled: bool,
    pub serve_metric_addr: String,
    pub seq_url: String,
    pub seq_api_key: String,
}

impl AppConfig {
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
