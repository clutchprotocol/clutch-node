use clutch_node::node::blockchain::Blockchain;
use clutch_node::node::transactions::chain_init::ChainInit;
use clutch_node::node::transactions::function_call::FunctionCall;
use clutch_node::node::transactions::transaction::Transaction;
use clutch_node::node::transactions::transfer::Transfer;
use serial_test::serial;

const AUTHOR_PK: &str = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
const AUTHOR_SK: &str = "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";
const FAUCET_PK: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
// Faucet secret is the committed dev key from clutch-hub-api config/default.toml:18.
const FAUCET_SK: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
const CHAIN_ID: u64 = 2077;
const TX_FEE: u64 = 1000;

fn ci() -> ChainInit {
    ChainInit {
        chain_id: CHAIN_ID,
        is_testnet: true,
        tx_fee: TX_FEE,
        ride_request_referrer_fee_bps: 200,
        ride_offer_referrer_fee_bps: 200,
        mint_authority: AUTHOR_PK.to_string(),
        faucet_address: FAUCET_PK.to_string(),
        faucet_allocation: 1_000_000_000_000_000,
    }
}

fn chain(name: &str) -> Blockchain {
    Blockchain::new(
        name.to_string(),
        AUTHOR_PK.to_string(),
        AUTHOR_SK.to_string(),
        true,
        vec![AUTHOR_PK.to_string()],
        ci(),
    )
}

fn signed_transfer(from: &str, sk: &str, nonce: u64, to: &str, value: u64) -> Transaction {
    let mut tx = Transaction::new_transaction(
        from.to_string(),
        nonce,
        CHAIN_ID,
        FunctionCall::Transfer(Transfer { to: to.to_string(), value }),
    );
    tx.sign(sk);
    tx
}

#[test]
#[serial]
fn transfer_charges_fee_and_credits_author() {
    let mut chain = chain("test-fee-basic");
    let faucet_before = chain.get_account_balance(&FAUCET_PK.to_string());

    let tx = signed_transfer(FAUCET_PK, FAUCET_SK, 1, "0x1111111111111111111111111111111111111111", 500);
    chain.add_transaction_to_pool(&tx).unwrap();
    chain.author_new_block().unwrap();

    assert_eq!(
        chain.get_account_balance(&FAUCET_PK.to_string()),
        faucet_before - 500 - TX_FEE,
        "sender pays value + fee"
    );
    assert_eq!(
        chain.get_account_balance(&"0x1111111111111111111111111111111111111111".to_string()),
        500
    );
    assert_eq!(
        chain.get_account_balance(&AUTHOR_PK.to_string()),
        TX_FEE,
        "author earns the fee (no block reward anymore)"
    );
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn exact_balance_without_fee_is_rejected() {
    use clutch_node::node::signature_keys::SignatureKeys;
    let mut chain = chain("test-fee-insufficient");
    // Fund a fresh account with exactly `value` (no headroom for the fee).
    let poor = SignatureKeys::generate_new_keypair();
    let fund = signed_transfer(FAUCET_PK, FAUCET_SK, 1, &poor.address_key, 500);
    chain.add_transaction_to_pool(&fund).unwrap();
    chain.author_new_block().unwrap();

    let overspend = signed_transfer(&poor.address_key, &poor.secret_key, 1, FAUCET_PK, 500);
    let err = chain.add_transaction_to_pool(&overspend).unwrap_err();
    assert!(err.to_lowercase().contains("fee") || err.to_lowercase().contains("insufficient"), "got: {}", err);
    chain.shutdown_blockchain();
}
