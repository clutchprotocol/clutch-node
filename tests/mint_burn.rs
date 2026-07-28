use clutch_node::node::blockchain::Blockchain;
use clutch_node::node::transactions::chain_init::ChainInit;
use clutch_node::node::transactions::function_call::FunctionCall;
use clutch_node::node::transactions::mint::Mint;
use clutch_node::node::transactions::transaction::Transaction;
use serial_test::serial;

const AUTHOR_PK: &str = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
const AUTHOR_SK: &str = "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";
const FAUCET_PK: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
// Same committed dev key as tests/tx_fee.rs (clutch-hub-api config/default.toml:18).
const FAUCET_SK: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
const CHAIN_ID: u64 = 2077;
const USER: &str = "0x4444444444444444444444444444444444444444";
const REF_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn ci() -> ChainInit {
    ChainInit {
        chain_id: CHAIN_ID,
        is_testnet: true,
        tx_fee: 1000,
        ride_request_referrer_fee_bps: 200,
        ride_offer_referrer_fee_bps: 200,
        // Mint authority = node1 dev key so tests can sign mints. Prod: dedicated key.
        mint_authority: AUTHOR_PK.to_string(),
        faucet_address: FAUCET_PK.to_string(),
        faucet_allocation: 1_000_000_000_000_000,
    }
}

fn chain(name: &str) -> Blockchain {
    // A panicking assertion never reaches shutdown_blockchain(), leaving the RocksDB dir
    // behind — the next run would then start with the previous run's nonces and fail for
    // the wrong reason. Start from a clean slate (same defensive pattern as tests/tx_fee.rs).
    let _ = std::fs::remove_dir_all(format!("{}.db", name));
    Blockchain::new(
        name.to_string(),
        AUTHOR_PK.to_string(),
        AUTHOR_SK.to_string(),
        true,
        vec![AUTHOR_PK.to_string()],
        ci(),
    )
}

fn signed_mint(sk: &str, from: &str, nonce: u64, to: &str, amount: u64, credit_ref: &str) -> Transaction {
    let mut tx = Transaction::new_transaction(
        from.to_string(),
        nonce,
        CHAIN_ID,
        FunctionCall::Mint(Mint {
            to: to.to_string(),
            amount,
            credit_ref: credit_ref.to_string(),
        }),
    );
    tx.sign(sk);
    tx
}

#[test]
#[serial]
fn authorized_mint_credits_and_grows_supply() {
    let mut chain = chain("test-mint-ok");
    let (_, supply0) = chain.get_chain_info().unwrap();

    let mint = signed_mint(AUTHOR_SK, AUTHOR_PK, 1, USER, 5_000_000, REF_A);
    chain.add_transaction_to_pool(&mint).unwrap();
    chain.author_new_block().unwrap();

    assert_eq!(chain.get_account_balance(&USER.to_string()), 5_000_000);
    let (_, supply1) = chain.get_chain_info().unwrap();
    assert_eq!(supply1, supply0 + 5_000_000, "total_supply tracks the mint");
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn unauthorized_mint_rejected() {
    let mut chain = chain("test-mint-unauth");
    // Faucet key is NOT the mint authority.
    let mint = signed_mint(FAUCET_SK, FAUCET_PK, 1, USER, 100, REF_A);
    let err = chain.add_transaction_to_pool(&mint).unwrap_err();
    assert!(err.contains("authority"), "got: {}", err);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn duplicate_credit_ref_rejected() {
    let mut chain = chain("test-mint-dup");
    let m1 = signed_mint(AUTHOR_SK, AUTHOR_PK, 1, USER, 100, REF_A);
    chain.add_transaction_to_pool(&m1).unwrap();
    chain.author_new_block().unwrap();

    let m2 = signed_mint(AUTHOR_SK, AUTHOR_PK, 2, USER, 100, REF_A);
    let err = chain.add_transaction_to_pool(&m2).unwrap_err();
    assert!(err.contains("credit_ref"), "exactly-once minting: {}", err);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn mint_rejects_zero_and_bad_ref() {
    let mut chain = chain("test-mint-bad");
    let zero = signed_mint(AUTHOR_SK, AUTHOR_PK, 1, USER, 0, REF_A);
    assert!(chain.add_transaction_to_pool(&zero).is_err());
    let bad_ref = signed_mint(AUTHOR_SK, AUTHOR_PK, 1, USER, 100, "not-hex");
    assert!(chain.add_transaction_to_pool(&bad_ref).is_err());
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn mint_works_with_zero_treasury_balance() {
    // Mint is fee-exempt: the authority holds no CLT at genesis and must still mint.
    let mut chain = chain("test-mint-feeless");
    assert_eq!(chain.get_account_balance(&AUTHOR_PK.to_string()), 0);
    let mint = signed_mint(AUTHOR_SK, AUTHOR_PK, 1, USER, 100, REF_A);
    chain.add_transaction_to_pool(&mint).unwrap();
    chain.author_new_block().unwrap();
    assert_eq!(chain.get_account_balance(&USER.to_string()), 100);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn author_own_tx_pays_no_fee() {
    use clutch_node::node::transactions::transfer::Transfer;
    // Fund the author via Mint (fee-exempt, single balance write — no deferred-batch
    // collision), then the author sends a transfer in a block it authors itself.
    let mut chain = chain("test-fee-author");
    let mint = signed_mint(
        AUTHOR_SK, AUTHOR_PK, 1, AUTHOR_PK, 10_000,
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    );
    chain.add_transaction_to_pool(&mint).unwrap();
    chain.author_new_block().unwrap();
    assert_eq!(chain.get_account_balance(&AUTHOR_PK.to_string()), 10_000);

    let mut own = Transaction::new_transaction(
        AUTHOR_PK.to_string(),
        2,
        CHAIN_ID,
        FunctionCall::Transfer(Transfer {
            to: "0x3333333333333333333333333333333333333333".to_string(),
            value: 100,
        }),
    );
    own.sign(AUTHOR_SK);
    chain.add_transaction_to_pool(&own).unwrap();
    chain.author_new_block().unwrap();

    assert_eq!(
        chain.get_account_balance(&AUTHOR_PK.to_string()),
        10_000 - 100,
        "author's own tx nets zero fee"
    );
    chain.shutdown_blockchain();
}
