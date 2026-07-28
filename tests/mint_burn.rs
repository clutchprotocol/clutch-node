use clutch_node::node::blockchain::Blockchain;
use clutch_node::node::blocks::block::Block;
use clutch_node::node::database::Database;
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

// Second sender for multi-sender block tests: DRIVER, a genuinely distinct matched keypair
// already used elsewhere in this suite (tests/balance_effects.rs) — the address is derived
// from the secret key via secp256k1, so it can't be picked independently of it.
const SECOND_PK: &str = "0x8f19077627cde4848b090c53c83b12956837d5e9";
const SECOND_SK: &str = "e74e3f87268132c7b3ddb24600716fc362f4519bf9986a9436aa8a1be58c7150";

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
        faucet_before - 2_000_000 - ci().tx_fee,
        "burner pays amount + fee"
    );
    let (_, supply1) = chain.get_chain_info().unwrap();
    assert_eq!(supply1, supply0 - 2_000_000, "supply shrinks by burn amount only (fee just moves)");
    // The fee moves to the block author rather than being destroyed alongside the burn —
    // this is what makes the supply delta above "amount only" instead of "amount + fee".
    assert_eq!(
        chain.get_account_balance(&AUTHOR_PK.to_string()),
        author_before + ci().tx_fee,
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
    let err = chain.add_transaction_to_pool(&b2).unwrap_err();
    assert!(
        err.contains("redemption_ref") && err.contains("already processed"),
        "exactly-once burn: {}",
        err
    );
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn burn_more_than_balance_rejected() {
    let mut chain = chain("test-burn-overdraw");
    let balance = chain.get_account_balance(&FAUCET_PK.to_string());
    let burn = signed_burn(FAUCET_SK, FAUCET_PK, 1, balance, None); // no headroom for fee
    let err = chain.add_transaction_to_pool(&burn).unwrap_err();
    assert!(
        err.contains("insufficient balance for amount + fee"),
        "got: {}",
        err
    );
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn burn_rejects_zero_and_bad_ref() {
    let mut chain = chain("test-burn-bad");
    let zero = signed_burn(FAUCET_SK, FAUCET_PK, 1, 0, Some(REF_B));
    let err = chain.add_transaction_to_pool(&zero).unwrap_err();
    assert!(err.contains("amount must be positive"), "got: {}", err);
    let bad_ref = signed_burn(FAUCET_SK, FAUCET_PK, 1, 100, Some("not-hex"));
    let err = chain.add_transaction_to_pool(&bad_ref).unwrap_err();
    assert!(err.contains("redemption_ref must be 64 lowercase hex chars"), "got: {}", err);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn burn_rejects_amount_over_i64_max() {
    // Unlike Mint (fee-exempt, so `verify_state` is the first thing to see the amount), an
    // over-max Burn can never reach this guard through the pool: `validate_transaction`
    // checks `amount + fee` against the balance first, and no balance can exceed the
    // i64::MAX supply cap, so it is always rejected for insufficient balance. The guard
    // still has to exist — `state_transaction` casts `-(amount as i64)`, which for
    // amount > i64::MAX wraps to a *credit* — so pin it by calling `verify_state` directly
    // (same direct-DB pattern as tests/chain_init.rs).
    let name = "test-burn-overflow";
    let db = Database::new_db(name);
    let err = Burn { amount: i64::MAX as u64 + 1, redemption_ref: None }
        .verify_state(&FAUCET_PK.to_string(), &db)
        .unwrap_err();
    assert!(err.contains("amount exceeds i64::MAX"), "got: {}", err);
    let mut db = db;
    db.close();
    db.delete_database(name).unwrap();
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

    // Fund the second sender via Mint (fee-exempt authority credit, single balance write).
    let fund = signed_mint(
        AUTHOR_SK, AUTHOR_PK, 1, SECOND_PK, 1_000_000,
        "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    );
    chain.add_transaction_to_pool(&fund).unwrap();
    chain.author_new_block().unwrap();

    let faucet_before = chain.get_account_balance(&FAUCET_PK.to_string());
    let second_before = chain.get_account_balance(&SECOND_PK.to_string());
    let author_before = chain.get_account_balance(&AUTHOR_PK.to_string());
    assert_eq!(second_before, 1_000_000);

    // Two Burns from two distinct senders, same block. Both are first-ever txs from their
    // sender in this fresh chain (the funding Mint above was sent by AUTHOR_PK, not either
    // burner), so both start at nonce 1. Neither carries a redemption_ref: `None` is the
    // absence of a ref, so the block-level ref-uniqueness check must not treat two of them
    // as a collision — this is also the plain-burn path's guard against that regression.
    let burn_faucet = signed_burn(FAUCET_SK, FAUCET_PK, 1, 300_000, None);
    let burn_second = signed_burn(SECOND_SK, SECOND_PK, 1, 400_000, None);
    chain.add_transaction_to_pool(&burn_faucet).unwrap();
    chain.add_transaction_to_pool(&burn_second).unwrap();
    let block = chain.author_new_block().unwrap();

    // Without this the test would still pass in shapes where only one burn landed and the
    // supply arithmetic happened to agree — the accumulation loop is only exercised at 2.
    assert_eq!(
        block.transactions.len(),
        2,
        "both ref-less burns must share the block for the supply sum to mean anything"
    );

    assert_eq!(
        chain.get_account_balance(&FAUCET_PK.to_string()),
        faucet_before - 300_000 - ci().tx_fee,
        "faucet burner pays amount + fee"
    );
    assert_eq!(
        chain.get_account_balance(&SECOND_PK.to_string()),
        second_before - 400_000 - ci().tx_fee,
        "second burner pays amount + fee"
    );
    // Two fees aggregate into the author's single folded balance write (block.rs) — this is
    // the only test with more than one fee-paying tx in a block, so nothing else can catch
    // a collapse to one fee.
    assert_eq!(
        chain.get_account_balance(&AUTHOR_PK.to_string()),
        author_before + 2 * ci().tx_fee,
        "author earns both fees, not just the last one folded in"
    );

    let (_, supply1) = chain.get_chain_info().unwrap();
    assert_eq!(
        supply1,
        supply0 + 1_000_000 - 300_000 - 400_000,
        "supply reflects the mint plus both burns summed, not last-write-wins"
    );
    chain.shutdown_blockchain();
}

/// Signs a block the way `author_new_block` does, so `import_block` sees a well-formed block
/// whose only defect is its transaction set — the point being that block *validation*, not
/// the authoring filter, is what has to reject it (a hostile author skips the filter).
fn forged_block(chain: &Blockchain, transactions: Vec<Transaction>) -> Block {
    let latest = chain.get_latest_block().unwrap().unwrap();
    let mut block = Block::new_block(latest.index + 1, latest.hash, transactions);
    block.sign(AUTHOR_PK, AUTHOR_SK);
    block
}

#[test]
#[serial]
fn same_redemption_ref_from_two_senders_cannot_share_a_block() {
    // Mint's same-block exactly-once property rode on the duplicate-sender guard: every
    // Mint is signed by the one authority, so two Mints in a block are the same sender and
    // get rejected there. Burn is permissionless, so two *different* senders can each carry
    // the same redemption_ref: both validate against pre-block state (the marker only lands
    // when the block commits) and their two identical `processed_ref_{ref}` writes collapse
    // to one in the deferred batch. Pool txs are re-gossiped, so a pending ref is public —
    // an attacker burns dust carrying Alice's ref and the treasury watcher sees two
    // confirmed burns claiming one redemption intent, for the price of the dust plus a fee.
    let mut chain = chain("test-burn-same-ref");
    let fund = signed_mint(AUTHOR_SK, AUTHOR_PK, 1, SECOND_PK, 1_000_000, REF_A);
    chain.add_transaction_to_pool(&fund).unwrap();
    chain.author_new_block().unwrap();

    let faucet_before = chain.get_account_balance(&FAUCET_PK.to_string());
    let (_, supply0) = chain.get_chain_info().unwrap();

    // Alice's genuine redemption, and the attacker's 1-micro-dollar burn claiming her ref.
    // Both are individually valid against committed state, so the pool accepts both.
    let alice = signed_burn(FAUCET_SK, FAUCET_PK, 1, 300_000, Some(REF_B));
    let attacker = signed_burn(SECOND_SK, SECOND_PK, 1, 1, Some(REF_B));
    chain.add_transaction_to_pool(&alice).unwrap();
    chain.add_transaction_to_pool(&attacker).unwrap();

    // Consensus layer: a block carrying both is invalid however it was assembled.
    let forged = forged_block(&chain, vec![alice.clone(), attacker.clone()]);
    let err = chain.import_block(&forged).unwrap_err();
    assert!(
        err.contains(REF_B),
        "block with two burns claiming one ref must be rejected, naming the ref: {}",
        err
    );

    // Authoring layer: the loser is filtered out so the node still makes progress, and the
    // winner is picked by nonce-then-hash order — identically on every node.
    let block = chain.author_new_block().unwrap();
    assert_eq!(block.transactions.len(), 1, "only one claim on the ref may land");
    let winner = if alice.hash <= attacker.hash { &alice } else { &attacker };
    assert_eq!(
        block.transactions[0].hash, winner.hash,
        "deterministic winner: lowest nonce, tie-broken by hash"
    );
    assert_eq!(
        chain.get_transactions_from_pool().unwrap().len(),
        1,
        "the loser stays pooled, it did not land"
    );

    // Exactly one of the two burns actually moved money.
    let (_, supply1) = chain.get_chain_info().unwrap();
    let landed = match &winner.data {
        FunctionCall::Burn(b) => b.amount,
        _ => unreachable!(),
    };
    assert_eq!(supply1, supply0 - landed, "one burn's worth of supply destroyed, not two");
    if winner.hash == attacker.hash {
        assert_eq!(
            chain.get_account_balance(&FAUCET_PK.to_string()),
            faucet_before,
            "Alice's burn did not land, so her balance is untouched"
        );
    }
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn mint_and_burn_sharing_a_ref_cannot_share_a_block() {
    // `processed_ref_{ref}` is one namespace across both types by design, so a Mint and a
    // Burn claiming the same ref collide exactly like two Burns — and being different
    // senders (authority vs. user), the duplicate-sender guard never sees them.
    let mut chain = chain("test-mint-burn-same-ref");
    let mint = signed_mint(AUTHOR_SK, AUTHOR_PK, 1, USER, 100, REF_A);
    let burn = signed_burn(FAUCET_SK, FAUCET_PK, 1, 100, Some(REF_A));
    chain.add_transaction_to_pool(&mint).unwrap();
    chain.add_transaction_to_pool(&burn).unwrap();

    let forged = forged_block(&chain, vec![mint.clone(), burn.clone()]);
    let err = chain.import_block(&forged).unwrap_err();
    assert!(err.contains(REF_A), "cross-type ref collision must be rejected: {}", err);

    let block = chain.author_new_block().unwrap();
    assert_eq!(block.transactions.len(), 1, "authoring keeps one claim on the ref");
    chain.shutdown_blockchain();
}
