use clutch_node::node::blockchain::Blockchain;
use clutch_node::node::transactions::burn::Burn;
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
    let err = chain.add_transaction_to_pool(&zero).unwrap_err();
    assert!(err.contains("amount must be positive"), "got: {}", err);
    let bad_ref = signed_mint(AUTHOR_SK, AUTHOR_PK, 1, USER, 100, "not-hex");
    let err = chain.add_transaction_to_pool(&bad_ref).unwrap_err();
    assert!(err.contains("credit_ref must be 64 lowercase hex chars"), "got: {}", err);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn mint_rejects_amount_over_i64_max() {
    let mut chain = chain("test-mint-overflow");
    let over = signed_mint(AUTHOR_SK, AUTHOR_PK, 1, USER, i64::MAX as u64 + 1, REF_A);
    let err = chain.add_transaction_to_pool(&over).unwrap_err();
    assert!(err.contains("amount exceeds i64::MAX"), "got: {}", err);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn mint_rejects_supply_out_of_range() {
    // Block-level `total_supply out of range` guard (block.rs) driven through a real
    // block, not asserted directly: genesis's testnet faucet_allocation (1e15, see `ci()`)
    // already makes total_supply > 0, so one Mint at the per-tx ceiling (`i64::MAX`, the
    // largest amount `Mint::verify_state` allows through) pushes
    // `supply0 + i64::MAX > i64::MAX` and trips the block-level guard on the same tx that
    // passed per-tx `verify_state` — the two checks are independent and both are needed.
    let mut chain = chain("test-mint-supply-range");
    let mint = signed_mint(AUTHOR_SK, AUTHOR_PK, 1, USER, i64::MAX as u64, REF_A);
    chain.add_transaction_to_pool(&mint).unwrap();
    let err = chain.author_new_block().unwrap_err();
    assert!(err.contains("total_supply out of range"), "got: {}", err);
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

const REF_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const TX_FEE: u64 = 1000;

fn signed_burn(sk: &str, from: &str, nonce: u64, amount: u64, redemption_ref: Option<&str>) -> Transaction {
    let mut tx = Transaction::new_transaction(
        from.to_string(),
        nonce,
        CHAIN_ID,
        FunctionCall::Burn(Burn {
            amount,
            redemption_ref: redemption_ref.map(|s| s.to_string()),
        }),
    );
    tx.sign(sk);
    tx
}

#[test]
#[serial]
fn burn_reduces_balance_and_supply() {
    let mut chain = chain("test-burn-ok");
    let (_, supply0) = chain.get_chain_info().unwrap();
    let faucet_before = chain.get_account_balance(&FAUCET_PK.to_string());
    let author_before = chain.get_account_balance(&AUTHOR_PK.to_string());

    let burn = signed_burn(FAUCET_SK, FAUCET_PK, 1, 2_000_000, Some(REF_B));
    chain.add_transaction_to_pool(&burn).unwrap();
    chain.author_new_block().unwrap();

    assert_eq!(
        chain.get_account_balance(&FAUCET_PK.to_string()),
        faucet_before - 2_000_000 - TX_FEE,
        "burner pays amount + fee"
    );
    let (_, supply1) = chain.get_chain_info().unwrap();
    assert_eq!(supply1, supply0 - 2_000_000, "supply shrinks by burn amount only (fee just moves)");
    // The fee moves to the block author rather than being destroyed alongside the burn —
    // this is what makes the supply delta above "amount only" instead of "amount + fee".
    assert_eq!(
        chain.get_account_balance(&AUTHOR_PK.to_string()),
        author_before + TX_FEE,
        "burn fee is credited to the block author, not destroyed"
    );
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn duplicate_redemption_ref_rejected() {
    let mut chain = chain("test-burn-dup");
    let b1 = signed_burn(FAUCET_SK, FAUCET_PK, 1, 100, Some(REF_B));
    chain.add_transaction_to_pool(&b1).unwrap();
    chain.author_new_block().unwrap();
    let b2 = signed_burn(FAUCET_SK, FAUCET_PK, 2, 100, Some(REF_B));
    assert!(chain.add_transaction_to_pool(&b2).is_err());
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn burn_more_than_balance_rejected() {
    let mut chain = chain("test-burn-overdraw");
    let balance = chain.get_account_balance(&FAUCET_PK.to_string());
    let burn = signed_burn(FAUCET_SK, FAUCET_PK, 1, balance, None); // no headroom for fee
    assert!(chain.add_transaction_to_pool(&burn).is_err());
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn plain_burn_without_ref_works() {
    let mut chain = chain("test-burn-plain");
    let burn = signed_burn(FAUCET_SK, FAUCET_PK, 1, 100, None);
    chain.add_transaction_to_pool(&burn).unwrap();
    chain.author_new_block().unwrap();
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn two_burns_from_different_senders_in_one_block_reduce_supply_by_sum() {
    // The block-level supply accumulation loop (block.rs) has never actually accumulated:
    // Mint is one-per-block (single authority, one tx per sender per block), but Burns come
    // from arbitrary users, so several can share a block. This is the first real exercise of
    // that loop with more than one entry — a wrong sign or a deferred-batch collision on
    // either burner's balance write would show up here and nowhere else.
    let mut chain = chain("test-burn-two-senders");
    let (_, supply0) = chain.get_chain_info().unwrap();

    // Second sender: DRIVER, a genuinely distinct matched keypair already used elsewhere
    // in this suite (tests/balance_effects.rs) — the address is derived from the secret
    // key via secp256k1, so it can't be picked independently of it.
    let second_user = "0x8f19077627cde4848b090c53c83b12956837d5e9";
    let second_sk = "e74e3f87268132c7b3ddb24600716fc362f4519bf9986a9436aa8a1be58c7150";

    // Fund the second sender via Mint (fee-exempt authority credit, single balance write).
    let fund = signed_mint(
        AUTHOR_SK, AUTHOR_PK, 1, second_user, 1_000_000,
        "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    );
    chain.add_transaction_to_pool(&fund).unwrap();
    chain.author_new_block().unwrap();

    let faucet_before = chain.get_account_balance(&FAUCET_PK.to_string());
    let second_before = chain.get_account_balance(&second_user.to_string());
    assert_eq!(second_before, 1_000_000);

    // Two Burns from two distinct senders, same block. Both are first-ever txs from their
    // sender in this fresh chain (the funding Mint above was sent by AUTHOR_PK, not either
    // burner), so both start at nonce 1.
    let burn_faucet = signed_burn(FAUCET_SK, FAUCET_PK, 1, 300_000, None);
    let burn_second = signed_burn(second_sk, second_user, 1, 400_000, None);
    chain.add_transaction_to_pool(&burn_faucet).unwrap();
    chain.add_transaction_to_pool(&burn_second).unwrap();
    chain.author_new_block().unwrap();

    assert_eq!(
        chain.get_account_balance(&FAUCET_PK.to_string()),
        faucet_before - 300_000 - TX_FEE,
        "faucet burner pays amount + fee"
    );
    assert_eq!(
        chain.get_account_balance(&second_user.to_string()),
        second_before - 400_000 - TX_FEE,
        "second burner pays amount + fee"
    );

    let (_, supply1) = chain.get_chain_info().unwrap();
    assert_eq!(
        supply1,
        supply0 + 1_000_000 - 300_000 - 400_000,
        "supply reflects the mint plus both burns summed, not last-write-wins"
    );
    chain.shutdown_blockchain();
}
