use clutch_node::node::database::Database;
use clutch_node::node::transactions::chain_init::ChainInit;
use serial_test::serial;

fn test_params() -> ChainInit {
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

#[test]
fn chain_init_rlp_roundtrip() {
    let ci = test_params();
    let encoded = clutch_node::node::rlp_encoding::encode(&ci);
    let decoded: ChainInit = clutch_node::node::rlp_encoding::decode(&encoded).unwrap();
    assert_eq!(ci, decoded);
}

#[test]
#[serial]
fn chain_init_rejected_outside_genesis() {
    let db = Database::new_db("test-chain-init-reject");
    let err = test_params()
        .verify_state(&"0xanyone".to_string(), &db)
        .unwrap_err();
    assert!(err.contains("genesis"), "got: {}", err);
    drop(db);
    let mut db = Database::new_db("test-chain-init-reject");
    db.close();
    db.delete_database("test-chain-init-reject").unwrap();
}

#[test]
#[serial]
fn chain_init_state_writes_params_supply_and_faucet() {
    let name = "test-chain-init-state";
    let db = Database::new_db(name);
    let ci = test_params();
    let updates = ci.state_transaction(&db);
    // Apply the storage updates the way add_block_to_chain would.
    let ops: Vec<(&str, &[u8], Option<&[u8]>)> = updates
        .iter()
        .filter_map(|u| u.storage.as_ref())
        .map(|(k, v)| ("state", k.as_slice(), Some(v.as_slice())))
        .collect();
    db.write(ops).unwrap();

    assert_eq!(ChainInit::get(&db).unwrap(), ci);
    assert_eq!(ChainInit::get_total_supply(&db).unwrap(), ci.faucet_allocation);
    let faucet = clutch_node::node::account_state::AccountState::get_current_state(
        &ci.faucet_address, &db,
    );
    assert_eq!(faucet.balance, ci.faucet_allocation);

    let mut db = db;
    db.close();
    db.delete_database(name).unwrap();
}

#[test]
#[serial]
fn chain_init_mainnet_flag_zeroes_supply() {
    let name = "test-chain-init-mainnet";
    let db = Database::new_db(name);
    let ci = ChainInit { is_testnet: false, faucet_allocation: 0, ..test_params() };
    let updates = ci.state_transaction(&db);
    let ops: Vec<(&str, &[u8], Option<&[u8]>)> = updates
        .iter()
        .filter_map(|u| u.storage.as_ref())
        .map(|(k, v)| ("state", k.as_slice(), Some(v.as_slice())))
        .collect();
    db.write(ops).unwrap();
    assert_eq!(ChainInit::get_total_supply(&db).unwrap(), 0);
    let mut db = db;
    db.close();
    db.delete_database(name).unwrap();
}
