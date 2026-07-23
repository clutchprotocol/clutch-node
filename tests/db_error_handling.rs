// Guards Fix #1: DB read paths on the hot loop must return Err instead of panicking.
// If someone reverts the map_err back to .unwrap(), this test panics instead of failing.
use clutch_node::node::{blocks::block::Block, database::Database};

const DB_NAME: &str = "clutch-node-test-db-error";

#[test]
fn get_latest_block_returns_err_on_corrupt_value() {
    let mut db = Database::new_db(DB_NAME);
    // Corrupt bytes under the latest-block key: not valid JSON.
    db.put("blockchain", b"blockchain_latest_block", b"not-a-valid-block")
        .expect("put should succeed");

    let result = Block::get_latest_block(&db);

    db.close();
    db.delete_database(DB_NAME).ok();

    assert!(
        result.is_err(),
        "corrupt latest block must return Err, got {:?}",
        result
    );
}
