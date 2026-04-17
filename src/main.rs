/*
 * Copyright 2024 Mehran Mazhar and Clutch Protocol Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use clap::Parser;
mod node;
use node::blockchain::Blockchain;
use node::configuration::AppConfig;
use node::tracing::setup_tracing;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long, default_value = "default")]
    env: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env = &Args::parse().env;
    let config = AppConfig::load_configuration(env)?;
    setup_tracing(&config.log_level, &config.seq_url, &config.seq_api_key)?;

    let blockchain = initialize_blockchain(&config);
    blockchain.start_network_services(&config).await;
    Ok(())
}

fn initialize_blockchain(config: &AppConfig) -> Blockchain {
    Blockchain::new(
        config.blockchain_name.clone(),
        config.author_public_key.clone(),
        config.author_secret_key.clone(),
        config.developer_mode.clone(),
        config.authorities.clone(),
        config.block_reward_amount,
    )
}
