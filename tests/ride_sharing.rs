use clutch_node::node::{
    blockchain::Blockchain,
    blocks::block::Block,
    coordinate,
    transactions::{
        chain_init::ChainInit, function_call::FunctionCall, ride_acceptance::RideAcceptance,
        ride_cancel::RideCancel, ride_offer::RideOffer, ride_pay::RidePay,
        ride_request::RideRequest, transaction::Transaction, transfer::Transfer,
    },
};
use serial_test::serial;

use ::tracing::info;

const BLOCKCHAIN_NAME: &str = "clutch-node-test";

const PASSENGER_ADDRESS_KEY: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
const PASSENGER_SECRET_KEY: &str =
    "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";

const DRIVER_ADDRESS_KEY: &str = "0x8f19077627cde4848b090c53c83b12956837d5e9";
const DRIVER_SECRET_KEY: &str = "e74e3f87268132c7b3ddb24600716fc362f4519bf9986a9436aa8a1be58c7150";

const AUTHOR_1_PUBLIC_KEY: &str = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
const AUTHOR_1_SECRET_KEY: &str =
    "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";

const AUTHOR_2_PUBLIC_KEY: &str = "0x6fc11ba44483201f6e9c5eba6435805bb94ad080";
const AUTHOR_2_SECRET_KEY: &str =
    "9aba0d89bfa358d27cfc119657537b9c92c8e38a35d2333ddd5c62e6d1a9b15e";

const AUTHOR_3_PUBLIC_KEY: &str = "0xc4f3f661a43e099aedb8e396d9de1a831a1b4adc";
const AUTHOR_3_SECRET_KEY: &str =
    "2d75bdfabbbaa65d7a182968e579adf2566fbb6931411752dd834c56bbf092c9";

fn ci() -> ChainInit {
    ChainInit {
        chain_id: 2077,
        is_testnet: true,
        tx_fee: 1000,
        ride_request_referrer_fee_bps: 200,
        ride_offer_referrer_fee_bps: 200,
        mint_authority: AUTHOR_1_PUBLIC_KEY.to_string(),
        faucet_address: PASSENGER_ADDRESS_KEY.to_string(),
        faucet_allocation: 1_000_000_000_000_000,
    }
}

#[test]
#[serial]
fn test_ride_sharing_blockchain() {
    let mut blockchain = new_blockchain();

    import_blocks(&mut blockchain);
    author_blocks(&mut blockchain);

    blockchain.shutdown_blockchain();
}

fn import_blocks(blockchain: &mut Blockchain) {
    // Under the flat-fee rule a zero-balance sender fails validation, so the driver
    // (who never receives anything otherwise in this flow) must be funded before its
    // first RideOffer. This funding block shifts every later block's index +1 and the
    // passenger's nonce +1 (it now consumes passenger nonce 1). Each downstream tx
    // references its predecessor by that predecessor's REAL hash (captured off the
    // built Transaction) rather than a hardcoded literal, since the hash commits to
    // (from, nonce, chain_id, data) and nonces shifted.
    let mut funding_block = faucet_to_driver_block(1, 1, 100_000);
    import_block(blockchain, &mut funding_block).expect("block import failed: faucet->driver funding");

    let ride_request_tx = ride_request_transcation(20, 2);
    let ride_request_hash = ride_request_tx.hash.clone();
    let mut block = Block::new_block(2, String::new(), vec![ride_request_tx]);
    import_block(blockchain, &mut block).expect("block import failed: ride request");

    let ride_offer_tx = ride_offer_transaction(30, 1, &ride_request_hash);
    let ride_offer_hash = ride_offer_tx.hash.clone();
    let mut block = Block::new_block(3, String::new(), vec![ride_offer_tx]);
    // A swallowed import error here would let a dead flow (e.g. this RideOffer rejected
    // for insufficient driver balance) report a passing test having executed nothing
    // downstream. Fail hard instead.
    import_block(blockchain, &mut block).expect("block import failed: ride offer");

    let ride_acceptance_tx = ride_acceptance_transaction(3, &ride_offer_hash);
    let ride_acceptance_hash = ride_acceptance_tx.hash.clone();
    let mut block = Block::new_block(4, String::new(), vec![ride_acceptance_tx]);
    import_block(blockchain, &mut block).expect("block import failed: ride acceptance");

    let mut block = Block::new_block(
        5,
        String::new(),
        vec![ride_pay_transaction(5, 4, &ride_acceptance_hash)], //5
    );
    import_block(blockchain, &mut block).expect("block import failed: ride pay 1");

    let mut block = Block::new_block(
        6,
        String::new(),
        vec![ride_pay_transaction(10, 5, &ride_acceptance_hash)], // 5 + 10 = 15
    );
    import_block(blockchain, &mut block).expect("block import failed: ride pay 2");

    let mut block = Block::new_block(
        7,
        String::new(),
        vec![ride_pay_transaction(10, 6, &ride_acceptance_hash)], // 15 + 10 = 25
    );
    import_block(blockchain, &mut block).expect("block import failed: ride pay 3");

    let mut block = Block::new_block(
        8,
        String::new(),
        vec![ride_cancel_transaction(7, &ride_acceptance_hash)],
    );
    import_block(blockchain, &mut block).expect("block import failed: ride cancel");
}

fn author_blocks(blockchain: &mut Blockchain) {
    let ride_request_tx = ride_request_transcation(1, 8);
    let pooled_hash = ride_request_tx.hash.clone();
    add_transaction_to_pool(&blockchain, ride_request_tx);

    // author_new_block already imports the block internally (Blockchain::author_new_block
    // calls self.import_block before returning) — importing it again here re-signs an
    // already-committed block against whatever slot the wall clock has since rotated to,
    // which always fails. The original swallowed-error pattern hid this dead second import
    // AND hid a pre-existing, unrelated timing gap this task doesn't own: this fixture's
    // node is fixed to AUTHOR_1's identity, but Aura picks the author by wall-clock slot
    // across all 3 authorities (step_duration = 20s), so author_new_block only succeeds
    // when AUTHOR_1's slot happens to be current. Poll for it instead of asserting on the
    // first tick — bounded to just over one full rotation so it can't hang.
    // ponytail: real-clock poll, not a proper test clock. Fine for an integration test;
    // revisit if Aura ever grows an injectable clock.
    let mut last_err = String::new();
    let mut authored = None;
    for _ in 0..300 {
        match blockchain.author_new_block() {
            Ok(block) => {
                authored = Some(block);
                break;
            }
            Err(e) => {
                last_err = e;
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }
    // Keep the block: discarding it let the loop pass while authoring an empty block, so a
    // pool regression that never drained the pending tx would still report green.
    let block = authored.unwrap_or_else(|| {
        panic!("failed to author new block after polling for a full Aura rotation: {}", last_err)
    });
    assert_eq!(block.transactions.len(), 1, "authored block must carry the pooled transaction");
    assert_eq!(block.transactions[0].hash, pooled_hash);
    info!("Successfully imported the new block.");
}

fn add_transaction_to_pool(blockchain: &Blockchain, ride_request_transcation: Transaction) {
    // Same hard-failure principle as import_block: a swallowed error here would let
    // author_new_block silently draft an empty block and the test would still pass
    // having authored nothing.
    blockchain
        .add_transaction_to_pool(&ride_request_transcation)
        .expect("failed to add transaction to transaction_pool");
    info!("Successfully added transaction to transaction_pool");
}

fn new_blockchain() -> Blockchain {
    let authorities = vec![
        AUTHOR_1_PUBLIC_KEY.to_string(),
        AUTHOR_2_PUBLIC_KEY.to_string(),
        AUTHOR_3_PUBLIC_KEY.to_string(),
    ];
    let blockchain = Blockchain::new(
        BLOCKCHAIN_NAME.to_string(),
        AUTHOR_1_PUBLIC_KEY.to_string(),
        AUTHOR_1_SECRET_KEY.to_string(),
        true,
        authorities,
        ci(),
    );
    blockchain
}

fn import_block(blockchain: &mut Blockchain, block: &mut Block) -> Result<(), String> {
    block.previous_hash = get_previous_hash(blockchain);

    if let Some((public_key, secret_key)) = current_author_keys(blockchain) {
        block.sign(public_key, secret_key);
    } else {
        return Err("Current author not found".to_string());
    }

    blockchain.import_block(block)
}

fn get_previous_hash(blockchain: &Blockchain) -> String {
    blockchain
        .get_latest_block()
        .expect("db read failed")
        .expect("Failed to get the latest block")
        .hash
}

fn current_author_keys(blockchain: &Blockchain) -> Option<(&str, &str)> {
    let author_keys = [
        (AUTHOR_1_PUBLIC_KEY, AUTHOR_1_SECRET_KEY),
        (AUTHOR_2_PUBLIC_KEY, AUTHOR_2_SECRET_KEY),
        (AUTHOR_3_PUBLIC_KEY, AUTHOR_3_SECRET_KEY),
    ];

    let current_author = blockchain.current_author();

    for &(public_key, secret_key) in &author_keys {
        if current_author == public_key {
            return Some((public_key, secret_key));
        }
    }
    None
}

fn faucet_to_driver_block(index: usize, nonce: u64, value: u64) -> Block {
    let mut transfer_transaction = Transaction::new_transaction(
        PASSENGER_ADDRESS_KEY.to_string(),
        nonce,
        2077,
        FunctionCall::Transfer(Transfer {
            to: DRIVER_ADDRESS_KEY.to_string(),
            value,
        }),
    );
    transfer_transaction.sign(PASSENGER_SECRET_KEY);
    Block::new_block(index, String::new(), vec![transfer_transaction])
}

fn ride_request_transcation(fare: u64, nonce: u64) -> Transaction {
    let ride_request = RideRequest {
        fare: fare,
        pickup_location: coordinate::Coordinates {
            latitude: 35.55841414973938,
            longitude: 51.23861773552397,
        }, //Tehran,Iran
        dropoff_location: coordinate::Coordinates {
            latitude: 26.649646426996483,
            longitude: 55.857706441083984,
        }, //Ghil,Hengam iceland,Iran
        referrer: None,
    };

    let mut ride_request_transcation = Transaction::new_transaction(
        PASSENGER_ADDRESS_KEY.to_string(),
        nonce,
        2077,
        FunctionCall::RideRequest(ride_request),
    );

    ride_request_transcation.sign(PASSENGER_SECRET_KEY);
    ride_request_transcation
}

fn ride_offer_transaction(fare: u64, nonce: u64, ride_request_tx_hash: &str) -> Transaction {
    let ride_offer = RideOffer {
        fare: fare,
        ride_request_transaction_hash: ride_request_tx_hash.to_string(),
        referrer: None,
    };

    let mut ride_offer_transaction = Transaction::new_transaction(
        DRIVER_ADDRESS_KEY.to_string(),
        nonce,
        2077,
        FunctionCall::RideOffer(ride_offer),
    );
    ride_offer_transaction.sign(DRIVER_SECRET_KEY);
    ride_offer_transaction
}

fn ride_acceptance_transaction(nonce: u64, ride_offer_tx_hash: &str) -> Transaction {
    let ride_acceptance = RideAcceptance {
        ride_offer_transaction_hash: ride_offer_tx_hash.to_string(),
    };

    let mut ride_acceptance_transaction = Transaction::new_transaction(
        PASSENGER_ADDRESS_KEY.to_string(),
        nonce,
        2077,
        FunctionCall::RideAcceptance(ride_acceptance),
    );
    ride_acceptance_transaction.sign(PASSENGER_SECRET_KEY);
    ride_acceptance_transaction
}

fn ride_pay_transaction(fare: u64, nonce: u64, ride_acceptance_tx_hash: &str) -> Transaction {
    let ride_pay = RidePay {
        fare: fare,
        ride_acceptance_transaction_hash: ride_acceptance_tx_hash.to_string(),
    };

    let mut ride_pay_transaction = Transaction::new_transaction(
        PASSENGER_ADDRESS_KEY.to_string(),
        nonce,
        2077,
        FunctionCall::RidePay(ride_pay),
    );
    ride_pay_transaction.sign(PASSENGER_SECRET_KEY);
    ride_pay_transaction
}

fn ride_cancel_transaction(nonce: u64, ride_acceptance_tx_hash: &str) -> Transaction {
    let ride_cancel = RideCancel {
        ride_acceptance_transaction_hash: ride_acceptance_tx_hash.to_string(),
    };

    let mut ride_cancel_transaction = Transaction::new_transaction(
        PASSENGER_ADDRESS_KEY.to_string(),
        nonce,
        2077,
        FunctionCall::RideCancel(ride_cancel),
    );

    ride_cancel_transaction.sign(PASSENGER_SECRET_KEY);
    ride_cancel_transaction
}

