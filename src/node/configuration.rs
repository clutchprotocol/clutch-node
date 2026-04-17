use config::{Config, ConfigError, Environment, File};
use dotenv::dotenv;
use serde::Deserialize;
use tracing::info;

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
    pub block_reward_amount: u64,
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
