use clutch_node::node::blockchain::Blockchain;
use clutch_node::node::coordinate::Coordinates;
use clutch_node::node::signature_keys::SignatureKeys;
use clutch_node::node::transactions::chain_init::ChainInit;
use clutch_node::node::transactions::function_call::FunctionCall;
use clutch_node::node::transactions::ride_request::RideRequest;
use clutch_node::node::transactions::session_key::SessionKey;
use clutch_node::node::transactions::transaction::Transaction;
use clutch_node::node::transactions::transfer::Transfer;
use serial_test::serial;

const AUTHOR_PK: &str = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
const AUTHOR_SK: &str = "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";
const FAUCET_PK: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
const FAUCET_SK: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
const CHAIN_ID: u64 = 2077;
const USER: &str = "0x4444444444444444444444444444444444444444";

fn chain(name: &str) -> Blockchain {
    let _ = std::fs::remove_dir_all(format!("{}.db", name));
    Blockchain::new(
        name.to_string(),
        AUTHOR_PK.to_string(),
        AUTHOR_SK.to_string(),
        true,
        vec![AUTHOR_PK.to_string()],
        ChainInit {
            chain_id: CHAIN_ID,
            is_testnet: true,
            tx_fee: 1000,
            ride_request_referrer_fee_bps: 200,
            ride_offer_referrer_fee_bps: 200,
            mint_authority: AUTHOR_PK.to_string(),
            faucet_address: FAUCET_PK.to_string(),
            faucet_allocation: 1_000_000_000_000_000,
            mint_cosigners: Vec::new(),
            mint_threshold: 0,
            ride_auto_release_secs: 0,
        },
    )
}

fn tx(sk: &str, nonce: u64, call: FunctionCall) -> Transaction {
    let mut t = Transaction::new_transaction(FAUCET_PK.to_string(), nonce, CHAIN_ID, call);
    t.sign(sk);
    t
}

fn ride() -> FunctionCall {
    FunctionCall::RideRequest(RideRequest {
        pickup_location: Coordinates { latitude: 1.0, longitude: 1.0 },
        dropoff_location: Coordinates { latitude: 2.0, longitude: 2.0 },
        fare: 1000,
        referrer: None,
    })
}

#[test]
#[serial]
fn session_key_signs_rides_only_and_can_be_revoked() {
    let mut chain = chain("test-session-key");
    let session = SignatureKeys::generate_new_keypair();
    let stranger = SignatureKeys::generate_new_keypair();

    // Before any authorization the session key is nobody.
    assert!(chain.add_transaction_to_pool(&tx(&session.secret_key, 1, ride())).is_err());

    // The account authorizes it, signed by its own key.
    let auth = FunctionCall::SessionKey(SessionKey {
        session_key: session.address_key.clone(),
        expires_at_block: 1000,
    });
    chain.add_transaction_to_pool(&tx(FAUCET_SK, 1, auth)).unwrap();
    chain.author_new_block().unwrap();

    // A session key cannot authorize another key, move CLT, or be impersonated.
    let other = FunctionCall::SessionKey(SessionKey {
        session_key: stranger.address_key.clone(),
        expires_at_block: 1000,
    });
    assert!(chain.add_transaction_to_pool(&tx(&session.secret_key, 2, other)).is_err());
    let transfer = FunctionCall::Transfer(Transfer { to: USER.to_string(), value: 1 });
    assert!(chain.add_transaction_to_pool(&tx(&session.secret_key, 2, transfer)).is_err());
    assert!(chain.add_transaction_to_pool(&tx(&stranger.secret_key, 2, ride())).is_err());

    // A ride signed by the session key is accepted.
    chain.add_transaction_to_pool(&tx(&session.secret_key, 2, ride())).unwrap();
    chain.author_new_block().unwrap();

    // Revoked: the session key stops working.
    let revoke = FunctionCall::SessionKey(SessionKey {
        session_key: session.address_key.clone(),
        expires_at_block: 0,
    });
    chain.add_transaction_to_pool(&tx(FAUCET_SK, 3, revoke)).unwrap();
    chain.author_new_block().unwrap();
    let cancel_ride = tx(&session.secret_key, 4, ride());
    assert!(chain.add_transaction_to_pool(&cancel_ride).is_err());
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn session_key_expires_at_its_block() {
    let mut chain = chain("test-session-key-expiry");
    let session = SignatureKeys::generate_new_keypair();
    // Already past: the next block is far beyond height 1.
    let auth = FunctionCall::SessionKey(SessionKey {
        session_key: session.address_key.clone(),
        expires_at_block: 1,
    });
    chain.add_transaction_to_pool(&tx(FAUCET_SK, 1, auth)).unwrap();
    chain.author_new_block().unwrap();
    assert!(chain.add_transaction_to_pool(&tx(&session.secret_key, 2, ride())).is_err());
    chain.shutdown_blockchain();
}
