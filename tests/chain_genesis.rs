use clutch_node::node::blockchain::Blockchain;
use clutch_node::node::transactions::chain_init::ChainInit;
use serial_test::serial;

fn test_chain_init() -> ChainInit {
    ChainInit {
        chain_id: 2077,
        is_testnet: true,
        tx_fee: 1000,
        ride_request_referrer_fee_bps: 200,
        ride_offer_referrer_fee_bps: 200,
        mint_authority: "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20".to_string(),
        faucet_address: "0xdeb4cfb63db134698e1879ea24904df074726cc0".to_string(),
        faucet_allocation: 1_000_000_000_000_000,
    }
}

fn new_test_chain(name: &str, ci: ChainInit) -> Blockchain {
    Blockchain::new(
        name.to_string(),
        "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20".to_string(),
        "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509".to_string(),
        true, // developer_mode: DB wiped on shutdown_blockchain
        vec!["0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20".to_string()],
        ci,
    )
}

#[test]
#[serial]
fn genesis_funds_faucet_and_stores_params() {
    let ci = test_chain_init();
    let mut chain = new_test_chain("test-genesis-testnet", ci.clone());
    assert_eq!(chain.get_account_balance(&ci.faucet_address), ci.faucet_allocation);
    let (params, supply) = chain.get_chain_info().unwrap();
    assert_eq!(params, ci);
    assert_eq!(supply, ci.faucet_allocation);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn genesis_hash_commits_to_chain_params() {
    let mut a = new_test_chain("test-genesis-a", test_chain_init());
    let hash_a = a.get_genesis_block().unwrap().unwrap().hash;
    a.shutdown_blockchain();

    let mut b = new_test_chain(
        "test-genesis-b",
        ChainInit { chain_id: 1, ..test_chain_init() },
    );
    let hash_b = b.get_genesis_block().unwrap().unwrap().hash;
    b.shutdown_blockchain();

    assert_ne!(hash_a, hash_b, "different chain params must yield different genesis hashes");
}

#[test]
#[serial]
fn mainnet_genesis_has_zero_supply() {
    let ci = ChainInit { is_testnet: false, faucet_allocation: 0, ..test_chain_init() };
    let mut chain = new_test_chain("test-genesis-mainnet", ci.clone());
    assert_eq!(chain.get_account_balance(&ci.faucet_address), 0);
    let (_, supply) = chain.get_chain_info().unwrap();
    assert_eq!(supply, 0);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
#[should_panic(expected = "non-testnet chain must have zero faucet_allocation")]
fn mainnet_with_faucet_allocation_fails_loudly() {
    let ci = ChainInit { is_testnet: false, faucet_allocation: 1, ..test_chain_init() };
    let _ = new_test_chain("test-genesis-loud", ci);
}

#[test]
#[should_panic(expected = "referrer fee bps sum exceeds 100%")]
fn referrer_bps_over_100_percent_fails_loudly() {
    let ci = ChainInit {
        ride_request_referrer_fee_bps: 6000,
        ride_offer_referrer_fee_bps: 6000,
        ..test_chain_init()
    };
    let _ = new_test_chain("test-genesis-bps-overflow", ci);
}

#[test]
#[should_panic(expected = "faucet_allocation exceeds i64::MAX")]
fn faucet_allocation_over_i64_max_fails_loudly() {
    let ci = ChainInit { faucet_allocation: u64::MAX, ..test_chain_init() };
    let _ = new_test_chain("test-genesis-faucet-overflow", ci);
}
