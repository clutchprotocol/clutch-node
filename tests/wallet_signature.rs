//! A transaction signed by a wallet (`personal_sign`, EIP-191) goes through the whole node path:
//! the pool, the authored block and its re-validation. Wallets such as MetaMask and Trust Wallet
//! will not sign a bare hash, so this is the way their users reach the chain.
//!
//! The signing text is written out in full in these tests on purpose. They pin the format that the
//! SDK and the wallets have to produce, and do not ask the node what it expects.

use clutch_node::node::blockchain::Blockchain;
use clutch_node::node::signature_keys::SignatureKeys;
use clutch_node::node::transactions::chain_init::ChainInit;
use clutch_node::node::transactions::function_call::FunctionCall;
use clutch_node::node::transactions::transaction::Transaction;
use clutch_node::node::transactions::transfer::Transfer;
use serial_test::serial;

const AUTHOR_PK: &str = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
const AUTHOR_SK: &str = "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";
const FAUCET_PK: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
const FAUCET_SK: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
const RECIPIENT: &str = "0x1111111111111111111111111111111111111111";
const CHAIN_ID: u64 = 2077;
const TX_FEE: u64 = 1000;

fn chain(name: &str) -> Blockchain {
    // A failed assertion never reaches shutdown_blockchain(), so start from a clean slate.
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
            tx_fee: TX_FEE,
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

fn transfer(from: &str, nonce: u64, value: u64) -> Transaction {
    Transaction::new_transaction(
        from.to_string(),
        nonce,
        CHAIN_ID,
        FunctionCall::Transfer(Transfer {
            to: RECIPIENT.to_string(),
            value,
        }),
    )
}

/// Sign `tx` as a wallet would, over `text`.
fn wallet_sign(tx: &mut Transaction, secret: &str, text: &str) {
    let bytes = SignatureKeys::personal_sign_bytes(text.as_bytes());
    let (r, s, v) = SignatureKeys::sign(secret, &bytes);
    tx.signature_r = r;
    tx.signature_s = s;
    tx.signature_v = v;
}

/// Fund `to` from the faucet with an ordinary signed transfer, and author the block.
fn fund(chain: &Blockchain, to: &str, value: u64) {
    let mut tx = Transaction::new_transaction(
        FAUCET_PK.to_string(),
        1,
        CHAIN_ID,
        FunctionCall::Transfer(Transfer {
            to: to.to_string(),
            value,
        }),
    );
    tx.sign(FAUCET_SK);
    chain.add_transaction_to_pool(&tx).expect("funding transfer");
    chain.author_new_block().expect("funding block");
}

#[test]
#[serial]
fn a_wallet_signed_transfer_is_accepted_and_applied() {
    let mut chain = chain("test-wallet-sig-transfer");
    let wallet = SignatureKeys::generate_new_keypair();
    fund(&chain, &wallet.address_key, 10_000);
    assert_eq!(chain.get_account_balance(&wallet.address_key), 10_000);

    let mut tx = transfer(&wallet.address_key, 1, 500);
    let text = format!("clutch-tx:{}:{}", CHAIN_ID, tx.hash.trim_start_matches("0x"));
    wallet_sign(&mut tx, &wallet.secret_key, &text);

    chain
        .add_transaction_to_pool(&tx)
        .unwrap_or_else(|e| panic!("pool refused a wallet-signed transfer: {}", e));
    let block = chain.author_new_block().expect("author_new_block");
    assert_eq!(block.transactions.len(), 1, "the block must carry the wallet's transfer");

    assert_eq!(
        chain.get_account_balance(&wallet.address_key),
        10_000 - 500 - TX_FEE,
        "the wallet pays value + fee"
    );
    assert_eq!(chain.get_account_balance(&RECIPIENT.to_string()), 500);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn a_wallet_signature_made_for_another_chain_is_refused() {
    let mut chain = chain("test-wallet-sig-other-chain");
    let wallet = SignatureKeys::generate_new_keypair();
    fund(&chain, &wallet.address_key, 10_000);

    let mut tx = transfer(&wallet.address_key, 1, 500);
    // The wallet was shown chain 1; this node runs chain 2077.
    let text = format!("clutch-tx:1:{}", tx.hash.trim_start_matches("0x"));
    wallet_sign(&mut tx, &wallet.secret_key, &text);

    let err = chain.add_transaction_to_pool(&tx).unwrap_err();
    assert!(err.contains("does not match the from address"), "got: {}", err);
    assert_eq!(chain.get_account_balance(&RECIPIENT.to_string()), 0);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn a_wallet_signature_from_another_key_is_refused() {
    let mut chain = chain("test-wallet-sig-other-key");
    let wallet = SignatureKeys::generate_new_keypair();
    let intruder = SignatureKeys::generate_new_keypair();
    fund(&chain, &wallet.address_key, 10_000);

    let mut tx = transfer(&wallet.address_key, 1, 500);
    let text = format!("clutch-tx:{}:{}", CHAIN_ID, tx.hash.trim_start_matches("0x"));
    wallet_sign(&mut tx, &intruder.secret_key, &text);

    let err = chain.add_transaction_to_pool(&tx).unwrap_err();
    assert!(err.contains("does not match the from address"), "got: {}", err);
    chain.shutdown_blockchain();
}
