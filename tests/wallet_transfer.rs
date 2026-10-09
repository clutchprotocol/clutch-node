//! A transfer sent from MetaMask (or any Ethereum wallet) as a signed legacy Ethereum transaction
//! goes through the whole node path: decode, the pool, the authored block, and lookup by hash.
//!
//! The two raw transactions below were signed by ethers 6.13.4 (`Wallet.signTransaction`, type 0,
//! chain id 20771, gas price 48 gwei, gas limit 21000) with Hardhat's first well-known test key.
//! They pin the bytes a real wallet produces; the node is not asked what it expects.

use clutch_node::node::blockchain::Blockchain;
use clutch_node::node::transactions::chain_init::ChainInit;
use clutch_node::node::transactions::function_call::FunctionCall;
use clutch_node::node::transactions::transaction::Transaction;
use clutch_node::node::transactions::transfer::Transfer;
use clutch_node::node::transactions::wallet_transfer::WalletTransferRule;
use serial_test::serial;

const AUTHOR_PK: &str = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
const AUTHOR_SK: &str = "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";
const FAUCET_PK: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
const FAUCET_SK: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
const RECIPIENT: &str = "0x1111111111111111111111111111111111111111";
const CHAIN_ID: u64 = 2077;
const TX_FEE: u64 = 1000;

/// The wallet that signed the fixtures.
const WALLET: &str = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";
/// Nonce 0, 0.5 CLT.
const RAW_0: &str = "f86e80850b2d05e0008252089411111111111111111111111111111111111111118806f05b59d3b200008082a269a06fe94f2d86929531a23b7924bab337fe820ed571e05241b0f193a995741f9169a0302808e1a46ffccb4cff8d001bc24b14119936445feb9d8b8fb757231cc275f1";
const HASH_0: &str = "0x2d22fdda39f6c330d06e7520b51747b3a38b8ef2e07ea435ff36978dd25ec445";
/// Nonce 1, 0.25 CLT.
const RAW_1: &str = "f86e01850b2d05e0008252089411111111111111111111111111111111111111118803782dace9d900008082a269a0a1bee66642889bcf6446648cd51eec8c1b68ba3b9296653d4cd4a29db87ff9fca0114b404f5c70590089c75d7741ddd395b95de28d8c0311ac626cc37e6063aec5";
const HASH_1: &str = "0x6220ad88e727d1d868fdc0cca46c7109a5025bd0c6cccc454ef0a32c0e80e068";

const ON: Option<WalletTransferRule> = Some(WalletTransferRule {
    wallet_chain_id: 20771,
    from_block: 0,
});

fn chain(name: &str, rule: Option<WalletTransferRule>) -> Blockchain {
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
    .with_wallet_transfers(rule)
}

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

fn decode(chain: &Blockchain, raw: &str) -> Transaction {
    chain
        .decode_wallet_transaction(&hex::decode(raw).unwrap())
        .expect("decode")
}

#[test]
#[serial]
fn a_metamask_transfer_is_accepted_applied_and_found_by_its_hash() {
    let mut chain = chain("test-wallet-transfer-ok", ON);
    fund(&chain, WALLET, 2_000_000);

    let tx = decode(&chain, RAW_0);
    assert_eq!(tx.from, WALLET);
    assert_eq!(tx.hash, HASH_0, "the node keeps the hash the wallet shows");
    assert_eq!(tx.nonce, 1);

    chain.add_transaction_to_pool(&tx).expect("pool");
    let pending = chain.get_transaction_by_hash(HASH_0).unwrap().expect("in the pool");
    assert!(pending.1.is_none(), "not in a block yet");

    let block = chain.author_new_block().expect("block");
    assert_eq!(block.transactions.len(), 1);
    assert_eq!(chain.get_account_balance(&WALLET.to_string()), 2_000_000 - 500_000 - TX_FEE);
    assert_eq!(chain.get_account_balance(&RECIPIENT.to_string()), 500_000);

    let (found, place) = chain
        .get_transaction_by_hash(&HASH_0.to_uppercase().replace("0X", "0x"))
        .unwrap()
        .expect("in a block");
    assert_eq!(found.hash, HASH_0);
    assert_eq!(place.unwrap().0, block.index as u64);

    // The second one carries the next wallet nonce.
    let tx1 = decode(&chain, RAW_1);
    assert_eq!(tx1.hash, HASH_1);
    chain.add_transaction_to_pool(&tx1).expect("second");
    chain.author_new_block().expect("block 2");
    assert_eq!(chain.get_account_balance(&RECIPIENT.to_string()), 750_000);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn without_a_rule_wallet_transfers_are_refused() {
    let mut chain = chain("test-wallet-transfer-off", None);
    fund(&chain, WALLET, 2_000_000);
    let err = chain.add_transaction_to_pool(&decode(&chain, RAW_0)).unwrap_err();
    assert!(err.contains("not enabled"), "{err}");
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn before_the_activation_block_they_are_refused() {
    let rule = Some(WalletTransferRule { wallet_chain_id: 20771, from_block: 50 });
    let mut chain = chain("test-wallet-transfer-early", rule);
    fund(&chain, WALLET, 2_000_000);
    let err = chain.add_transaction_to_pool(&decode(&chain, RAW_0)).unwrap_err();
    assert!(err.contains("starts at block 50"), "{err}");
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn a_signature_for_another_wallet_network_is_refused() {
    // Mainnet's wallet id: a stage signature must not move mainnet CLT.
    let rule = Some(WalletTransferRule { wallet_chain_id: 20770, from_block: 0 });
    let mut chain = chain("test-wallet-transfer-other-net", rule);
    fund(&chain, WALLET, 2_000_000);
    let err = chain.add_transaction_to_pool(&decode(&chain, RAW_0)).unwrap_err();
    assert!(err.contains("signed for chain id 20771"), "{err}");
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn a_tampered_transaction_is_refused() {
    let mut chain = chain("test-wallet-transfer-tamper", ON);
    fund(&chain, WALLET, 2_000_000);
    let mut tx = decode(&chain, RAW_0);
    if let FunctionCall::WalletTransfer(wt) = &mut tx.data {
        wt.to = "0x2222222222222222222222222222222222222222".to_string();
    }
    assert!(chain.add_transaction_to_pool(&tx).is_err(), "a changed recipient must not verify");
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn the_wallet_must_hold_value_plus_fee() {
    let mut chain = chain("test-wallet-transfer-poor", ON);
    fund(&chain, WALLET, 500_000);
    let err = chain.add_transaction_to_pool(&decode(&chain, RAW_0)).unwrap_err();
    assert!(err.contains("insufficient balance"), "{err}");
    chain.shutdown_blockchain();
}

/// A peer imports the authored blocks after an RLP round trip (the gossip and sync encoding).
/// A peer without the rule refuses the block that carries the wallet transfer: this is why every
/// validator must carry the same rule before its height arrives.
#[test]
#[serial]
fn peers_import_the_block_only_with_the_same_rule() {
    use clutch_node::node::blocks::block::Block;
    use clutch_node::node::rlp_encoding::{decode as rlp_decode, encode};

    let mut author = chain("test-wallet-transfer-author", ON);
    let mut peer = chain("test-wallet-transfer-peer", ON);
    let mut old_peer = chain("test-wallet-transfer-old-peer", None);

    fund(&author, WALLET, 2_000_000);
    author.add_transaction_to_pool(&decode(&author, RAW_0)).unwrap();
    author.author_new_block().unwrap();

    for index in [1usize, 2] {
        let block = author.get_blocks_by_indexes(vec![index]).unwrap().remove(0);
        let wire: Block = rlp_decode(&encode(&block)).expect("block RLP round trip");
        peer.import_block(&wire)
            .unwrap_or_else(|e| panic!("peer refused block {index}: {e}"));
        let old = old_peer.import_block(&wire);
        if index == 1 {
            old.expect("the funding block has no wallet transfer");
        } else {
            let err = old.unwrap_err();
            assert!(err.contains("not enabled"), "{err}");
        }
    }
    assert_eq!(peer.get_account_balance(&RECIPIENT.to_string()), 500_000);

    author.shutdown_blockchain();
    peer.shutdown_blockchain();
    old_peer.shutdown_blockchain();
}
