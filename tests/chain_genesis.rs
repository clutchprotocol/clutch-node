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

#[test]
#[serial]
fn wrong_chain_id_rejected_at_pool() {
    use clutch_node::node::transactions::function_call::FunctionCall;
    use clutch_node::node::transactions::transaction::Transaction;
    use clutch_node::node::transactions::transfer::Transfer;

    let ci = test_chain_init(); // chain_id 2077
    let mut chain = new_test_chain("test-wrong-chain", ci.clone());

    let mut tx = Transaction::new_transaction(
        ci.faucet_address.clone(),
        1,
        1, // wrong chain
        FunctionCall::Transfer(Transfer {
            to: "0x1111111111111111111111111111111111111111".to_string(),
            value: 1,
        }),
    );
    tx.sign("d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f");

    let err = chain.add_transaction_to_pool(&tx).unwrap_err();
    assert!(err.contains("does not match chain"), "got: {}", err);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn chain_info_supply_tracks_mint_and_burn() {
    use clutch_node::node::transactions::burn::Burn;
    use clutch_node::node::transactions::function_call::FunctionCall;
    use clutch_node::node::transactions::mint::Mint;
    use clutch_node::node::transactions::transaction::Transaction;

    const AUTHOR_SK: &str = "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";
    const FAUCET_SK: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
    const USER: &str = "0x4444444444444444444444444444444444444444";

    let ci = test_chain_init(); // chain_id 2077, mint_authority = author, testnet
    let mut chain = new_test_chain("test-supply-e2e", ci.clone());

    let (_, supply_genesis) = chain.get_chain_info().unwrap();
    assert_eq!(supply_genesis, ci.faucet_allocation);

    // Mint block: +5_000_000 to USER.
    let mut mint = Transaction::new_transaction(
        ci.mint_authority.clone(),
        1,
        ci.chain_id,
        FunctionCall::Mint(Mint {
            to: USER.to_string(),
            amount: 5_000_000,
            credit_ref: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                .to_string(),
        }),
    );
    mint.sign(AUTHOR_SK);
    chain.add_transaction_to_pool(&mint).unwrap();
    chain.author_new_block().unwrap();

    let (_, supply_after_mint) = chain.get_chain_info().unwrap();
    assert_eq!(supply_after_mint, supply_genesis + 5_000_000);

    // Burn block: faucet burns 2_000_000 (fee moves CLT, supply drops by burn only).
    let mut burn = Transaction::new_transaction(
        ci.faucet_address.clone(),
        1,
        ci.chain_id,
        FunctionCall::Burn(Burn {
            amount: 2_000_000,
            redemption_ref: Some(
                "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string(),
            ),
        }),
    );
    burn.sign(FAUCET_SK);
    chain.add_transaction_to_pool(&burn).unwrap();
    chain.author_new_block().unwrap();

    let (_, supply_after_burn) = chain.get_chain_info().unwrap();
    assert_eq!(supply_after_burn, supply_after_mint - 2_000_000);

    chain.shutdown_blockchain();
}
