use clutch_node::node::balance_effect::BalanceEffectKind;
use clutch_node::node::blockchain::Blockchain;
use clutch_node::node::coordinate::Coordinates;
use clutch_node::node::signature_keys::SignatureKeys;
use clutch_node::node::transactions::chain_init::ChainInit;
use clutch_node::node::transactions::function_call::FunctionCall;
use clutch_node::node::transactions::ride_acceptance::RideAcceptance;
use clutch_node::node::transactions::ride_cancel::RideCancel;
use clutch_node::node::transactions::ride_offer::RideOffer;
use clutch_node::node::transactions::ride_pay::RidePay;
use clutch_node::node::transactions::ride_request::RideRequest;
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

fn ci_with_faucet(faucet: &str) -> ChainInit {
    ChainInit {
        chain_id: CHAIN_ID,
        is_testnet: true,
        tx_fee: TX_FEE,
        ride_request_referrer_fee_bps: 200,
        ride_offer_referrer_fee_bps: 200,
        mint_authority: AUTHOR_PK.to_string(),
        faucet_address: faucet.to_string(),
        faucet_allocation: 1_000_000_000_000_000,
    }
}

fn ci() -> ChainInit {
    ci_with_faucet(FAUCET_PK)
}

fn chain_with(name: &str, params: ChainInit) -> Blockchain {
    // A panicking assertion never reaches shutdown_blockchain(), leaving the RocksDB dir
    // behind — the next run would then start with the previous run's nonces and fail for
    // the wrong reason. Start from a clean slate.
    let _ = std::fs::remove_dir_all(format!("{}.db", name));
    // Single authority: every Aura slot maps to AUTHOR_PK, so author_new_block always
    // succeeds and needs no slot polling.
    Blockchain::new(
        name.to_string(),
        AUTHOR_PK.to_string(),
        AUTHOR_SK.to_string(),
        true,
        vec![AUTHOR_PK.to_string()],
        params,
    )
}

fn chain(name: &str) -> Blockchain {
    chain_with(name, ci())
}

/// Pool `txs` and author exactly one block out of them, asserting the block really carried
/// them all — a silently-dropped tx would otherwise make the balance assertions meaningless.
fn mine(chain: &Blockchain, txs: &[Transaction]) {
    for tx in txs {
        chain
            .add_transaction_to_pool(tx)
            .unwrap_or_else(|e| panic!("pool rejected {}: {}", tx.hash, e));
    }
    let block = chain.author_new_block().expect("author_new_block");
    assert_eq!(
        block.transactions.len(),
        txs.len(),
        "authored block must carry every pooled transaction"
    );
}

fn balances(chain: &Blockchain, addrs: &[&str]) -> Vec<u64> {
    addrs
        .iter()
        .map(|a| chain.get_account_balance(&a.to_string()))
        .collect()
}

fn signed(from: &str, sk: &str, nonce: u64, call: FunctionCall) -> Transaction {
    let mut tx = Transaction::new_transaction(from.to_string(), nonce, CHAIN_ID, call);
    tx.sign(sk);
    tx
}

fn signed_transfer(from: &str, sk: &str, nonce: u64, to: &str, value: u64) -> Transaction {
    signed(
        from,
        sk,
        nonce,
        FunctionCall::Transfer(Transfer { to: to.to_string(), value }),
    )
}

fn signed_ride_request(from: &str, sk: &str, nonce: u64, fare: u64) -> Transaction {
    signed(
        from,
        sk,
        nonce,
        FunctionCall::RideRequest(RideRequest {
            fare,
            pickup_location: Coordinates { latitude: 35.55, longitude: 51.23 },
            dropoff_location: Coordinates { latitude: 26.64, longitude: 55.85 },
            referrer: None,
        }),
    )
}

fn signed_ride_offer(from: &str, sk: &str, nonce: u64, request_hash: &str, fare: u64) -> Transaction {
    signed(
        from,
        sk,
        nonce,
        FunctionCall::RideOffer(RideOffer {
            fare,
            ride_request_transaction_hash: request_hash.to_string(),
            referrer: None,
        }),
    )
}

fn signed_ride_acceptance(from: &str, sk: &str, nonce: u64, offer_hash: &str) -> Transaction {
    signed(
        from,
        sk,
        nonce,
        FunctionCall::RideAcceptance(RideAcceptance {
            ride_offer_transaction_hash: offer_hash.to_string(),
        }),
    )
}

fn signed_ride_pay(from: &str, sk: &str, nonce: u64, acceptance_hash: &str, fare: u64) -> Transaction {
    signed(
        from,
        sk,
        nonce,
        FunctionCall::RidePay(RidePay {
            fare,
            ride_acceptance_transaction_hash: acceptance_hash.to_string(),
        }),
    )
}

fn signed_ride_cancel(from: &str, sk: &str, nonce: u64, acceptance_hash: &str) -> Transaction {
    signed(
        from,
        sk,
        nonce,
        FunctionCall::RideCancel(RideCancel {
            ride_acceptance_transaction_hash: acceptance_hash.to_string(),
        }),
    )
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
    let mut chain = chain("test-fee-insufficient");
    // Fund a fresh account with exactly `value` (no headroom for the fee).
    let poor = SignatureKeys::generate_new_keypair();
    let fund = signed_transfer(FAUCET_PK, FAUCET_SK, 1, &poor.address_key, 500);
    mine(&chain, &[fund]);

    let overspend = signed_transfer(&poor.address_key, &poor.secret_key, 1, FAUCET_PK, 500);
    let err = chain.add_transaction_to_pool(&overspend).unwrap_err();
    // Pin the amount+fee check specifically: a bare "insufficient balance" substring would
    // also match the unrelated value-only check this rule replaced.
    assert!(
        err.contains("insufficient balance for amount + fee")
            && err.contains(&format!("Required: {}", 500 + TX_FEE))
            && err.contains("available: 500"),
        "got: {}",
        err
    );
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn author_own_transaction_survives_the_block_fee_credit() {
    // Regression (money): the block-level author fee credit is computed from pre-block
    // state. Appending it after the per-tx loop made it the LAST write to the author's
    // balance key in the deferred batch, so when the author also sent a transaction in
    // that block its own debit was overwritten — the recipient kept the value and the
    // author was made whole. Net new unbacked CLT.
    const SEED: u64 = 1_000_000;
    const V: u64 = 5_000; // author -> dave
    const W: u64 = 700; // bob -> erin
    const DAVE: &str = "0x2222222222222222222222222222222222222222";
    const ERIN: &str = "0x3333333333333333333333333333333333333333";

    // Author holds the genesis allocation so it can spend without first being credited.
    let mut chain = chain_with("test-fee-author-selfsend", ci_with_faucet(AUTHOR_PK));
    let bob = SignatureKeys::generate_new_keypair();

    // Block 1: only the author sends, so effective_fee is 0 and no block credit is emitted.
    mine(&chain, &[signed_transfer(AUTHOR_PK, AUTHOR_SK, 1, &bob.address_key, SEED)]);

    let accounts = [AUTHOR_PK, bob.address_key.as_str(), DAVE, ERIN];
    let before = balances(&chain, &accounts);

    // Block 2: the author's own (fee-exempt) tx alongside a fee-paying tx from bob.
    mine(
        &chain,
        &[
            signed_transfer(AUTHOR_PK, AUTHOR_SK, 2, DAVE, V),
            signed_transfer(&bob.address_key, &bob.secret_key, 1, ERIN, W),
        ],
    );
    let after = balances(&chain, &accounts);

    assert_eq!(
        after[0],
        before[0] - V + TX_FEE,
        "author must keep its own debit AND earn bob's fee"
    );
    assert_eq!(after[1], before[1] - W - TX_FEE, "bob pays value + fee");
    assert_eq!(after[2], before[2] + V, "dave receives the author's transfer");
    assert_eq!(after[3], before[3] + W, "erin receives bob's transfer");
    assert_eq!(
        after.iter().sum::<u64>(),
        before.iter().sum::<u64>(),
        "supply across the involved accounts must be conserved"
    );
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn ride_pay_when_payer_is_also_the_driver_keeps_the_credit() {
    // Regression (money): RidePay took the central standalone fee debit on `from` while
    // also crediting the driver. Nothing stops a passenger from offering on their own
    // request, so both writes hit one balance key and the deferred batch kept only the
    // fee debit — the fare, already escrowed at RideAcceptance, was credited to nobody.
    const FARE: u64 = 40_000;
    let mut chain = chain("test-fee-ridepay-self-driver");

    let req = signed_ride_request(FAUCET_PK, FAUCET_SK, 1, FARE);
    mine(&chain, &[req.clone()]);
    // Passenger offers on their own request: driver == passenger == payer.
    let offer = signed_ride_offer(FAUCET_PK, FAUCET_SK, 2, &req.hash, FARE);
    mine(&chain, &[offer.clone()]);
    let acceptance = signed_ride_acceptance(FAUCET_PK, FAUCET_SK, 3, &offer.hash);
    mine(&chain, &[acceptance.clone()]);

    let before = balances(&chain, &[FAUCET_PK, AUTHOR_PK]);

    let pay = signed_ride_pay(FAUCET_PK, FAUCET_SK, 4, &acceptance.hash, FARE);
    mine(&chain, &[pay.clone()]);
    let after = balances(&chain, &[FAUCET_PK, AUTHOR_PK]);

    assert_eq!(
        after[0],
        before[0] + FARE - TX_FEE,
        "payer is also the driver: the fare credit must survive the fee debit"
    );
    assert_eq!(after[1], before[1] + TX_FEE, "author earns the fee");
    assert_eq!(
        after.iter().sum::<u64>(),
        before.iter().sum::<u64>() + FARE,
        "exactly the escrowed fare re-enters circulation, nothing more or less"
    );

    // One storage write, but the audit trail still records each reason separately.
    let kinds: Vec<BalanceEffectKind> = chain
        .get_tx_balance_effects(&pay.hash)
        .into_iter()
        .map(|e| e.effect.kind)
        .collect();
    assert!(kinds.contains(&BalanceEffectKind::RidePayDriverCredit), "{:?}", kinds);
    assert!(kinds.contains(&BalanceEffectKind::TxFeePaid), "{:?}", kinds);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn ride_cancel_by_driver_refunds_passenger_and_debits_driver() {
    // The driver branch of RideCancel takes a standalone fee debit while the passenger
    // gets the refund; mis-branching here is a money bug, so pin both legs.
    const FARE: u64 = 20_000;
    let mut chain = chain("test-fee-cancel-driver");
    let driver = SignatureKeys::generate_new_keypair();

    mine(&chain, &[signed_transfer(FAUCET_PK, FAUCET_SK, 1, &driver.address_key, 50_000)]);
    let req = signed_ride_request(FAUCET_PK, FAUCET_SK, 2, FARE);
    mine(&chain, &[req.clone()]);
    let offer = signed_ride_offer(&driver.address_key, &driver.secret_key, 1, &req.hash, FARE);
    mine(&chain, &[offer.clone()]);
    let acceptance = signed_ride_acceptance(FAUCET_PK, FAUCET_SK, 3, &offer.hash);
    mine(&chain, &[acceptance.clone()]);

    let accounts = [FAUCET_PK, driver.address_key.as_str(), AUTHOR_PK];
    let before = balances(&chain, &accounts);

    mine(
        &chain,
        &[signed_ride_cancel(&driver.address_key, &driver.secret_key, 2, &acceptance.hash)],
    );
    let after = balances(&chain, &accounts);

    assert_eq!(
        after[0],
        before[0] + FARE,
        "driver-cancel refunds the whole unpaid escrow to the passenger"
    );
    assert_eq!(after[1], before[1] - TX_FEE, "the cancelling driver pays the fee");
    assert_eq!(after[2], before[2] + TX_FEE, "author earns the fee");
    chain.shutdown_blockchain();
}
