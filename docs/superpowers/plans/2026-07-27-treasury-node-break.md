# Treasury Node Breaking Release — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One breaking testnet release of clutch-node that adds Mint/Burn transaction types with exactly-once refs, a chain ID in the signed payload, consensus parameters committed by the genesis hash, a flat transaction fee replacing block rewards, basis-point floor-rounded referrer fees, and a total-supply counter + `get_chain_info` RPC — the on-chain foundation for the Treasury Service.

**Architecture:** A new genesis-only `ChainInit` transaction carries all consensus parameters (chain_id, is_testnet, tx_fee, referrer bps, mint_authority, faucet allocation). Because the tx hash feeds the genesis block hash, which peers compare at p2p handshake, mismatched params can never peer — this fixes the existing class of bug where `block_reward_amount` was per-node config. Runtime reads params from state (`chain_params` key), never from config. Supply changes and author fee credits are computed **once per block** (single read-modify-write), never per-tx, because the deferred RocksDB batch applies all tx updates against pre-block state (last-write-wins — see `ponytail:` comments in `transaction.rs`/`blockchain.rs`).

**Tech Stack:** Rust (existing repo), rlp 0.5.2, RocksDB, secp256k1, serde_json; proptest (new dev-dep) for fee-invariant property tests.

## Deviations from spec (explicit, per spec §11)

- **Spec §4.1/§4a (locked: i64→i128, 6 decimals) is superseded**, approved by the user 2026-07-27 with the peg decision: 1 USD = 1,000,000 CLT makes CLT itself the micro-USD base unit, so 6 decimals + i128 add scope with no benefit — full rationale in `D:\source\clutch\treasury-analysis-2026-07-27.md` §2. §4a's "i128 boundary" property tests are correspondingly reinterpreted as u64 boundary tests with wide-integer intermediates (Task 1 provides them).

## Global Constraints

- **Peg (decided):** 1 USD = 1,000,000 CLT. CLT is the base unit (micro-USD). **Zero decimals. Keep `u64` balances / `i64` deltas.** No i128/u128 in stored state or RLP; wide-integer *intermediates* for overflow-safe arithmetic are fine and used deliberately (Task 1's u128 fee math, Task 6's i128 supply delta). Never floats.
- **Never run host `cargo build`/`cargo test`** (user convention). All test runs use the Docker image built in Task 1:
  `docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test`
- All commands run from repo root `D:\source\clutch\clutch-node` on branch `treasury-break`. **Never push to `main`** — user reviews first (spec §11).
- Breaking changes are expected (alpha/testnet, DBs get wiped) — no backward-compat shims for state or RLP formats.
- RLP tags: Transfer=0, RideRequest=1, RideOffer=2, RideAcceptance=3, RidePay=4, RideCancel=5, **Mint=6, Burn=7**, RideRequestCancel=8, **ChainInit=9**. These must byte-match the JS SDK encoder in the follow-up SDK release.
- New tests that open a database must carry `#[serial]` (serial_test crate) and clean up via `blockchain.shutdown_blockchain()` (developer_mode) or `db.delete_database(name)` — repo convention.
- Chain params for this testnet: `chain_id = 2077`, `is_testnet = true`, `tx_fee = 1000` (= $0.001), `ride_request_referrer_fee_bps = 200`, `ride_offer_referrer_fee_bps = 200`, `faucet_allocation = 1_000_000_000_000_000` (= $1B, JS-safe below 2^53).
- The three node TOMLs must carry **identical** chain-param values or the nodes cannot peer (genesis hash mismatch by design).
- Known deferred ceiling (do NOT fix here): intra-block state uses one deferred batch with fresh-DB reads, so two writes to the same account key in one block collide (pre-existing; documented in CLAUDE.md). This plan avoids adding new collisions by (a) one-tx-per-sender already enforced, (b) block-level single-write for author fees and total_supply, (c) merging each sender's fee into its single balance write. Mark any spot relying on this with a `ponytail:` comment.

## File Structure

| File | Change |
|---|---|
| `Dockerfile.test` | new — test-runner image (rust + clang for rocksdb) |
| `Cargo.toml` | + `proptest` dev-dep |
| `src/node/transactions/chain_init.rs` | new — ChainInit params tx + state keys + getters |
| `src/node/transactions/mint.rs` | new — Mint tx |
| `src/node/transactions/burn.rs` | new — Burn tx |
| `src/node/transactions/function_call.rs` | + 3 enum variants |
| `src/node/rlp_encoding.rs` | + tags 6/7/9; Transaction 7→8 items (chain_id) |
| `src/node/transactions/transaction.rs` | chain_id field + hash preimage; genesis via ChainInit; fee helpers; params-based dispatch |
| `src/node/transactions/transfer.rs` | fee merged into sender debit |
| `src/node/transactions/ride_acceptance.rs` | fee merged into escrow debit; verify fare+fee |
| `src/node/transactions/ride_cancel.rs` | fee merged into refund (passenger) / standalone (driver) |
| `src/node/transactions/ride_pay.rs` | percent→bps, ceiling→floor |
| `src/node/account_state.rs` | + `apply_balance_change_with_fee` |
| `src/node/balance_effect.rs` | + 4 `BalanceEffectKind` variants |
| `src/node/blocks/block.rs` | genesis(params); add_block_to_chain: params from state, reward code → fee credit + supply delta |
| `src/node/blockchain.rs` | new constructor signature; boot validation; drop reward/percent fields |
| `src/node/configuration.rs` | + 6 fields, − block_reward_amount, percent→bps |
| `src/node/wss/websocket.rs` | + `get_chain_info`; block_reward literal 0 |
| `src/main.rs` | build ChainInit from config |
| `config/node/*.toml` (6 files) | new/renamed keys |
| `tests/` | new: `chain_genesis.rs`, `mint_burn.rs`, `tx_fee.rs`; updated: all existing |
| `docs/state_keys.csv`, `CLAUDE.md` | document new keys/types/RPC |

---

### Task 1: Basis-point referrer fees with floor rounding

**Files:**
- Create: `Dockerfile.test`
- Modify: `Cargo.toml`, `src/node/transactions/ride_pay.rs`, `src/node/configuration.rs`, `src/node/transactions/transaction.rs:233-238`, `src/node/blockchain.rs:26-28,38-40,51-53,119-125,151-157`, `src/node/blocks/block.rs:296-302,336-341`, `config/node/{default,node1,node2,node3,node2-docker,node3-docker}.toml`
- Test: `src/node/transactions/ride_pay.rs` (unit + proptest in `#[cfg(test)]`)

**Interfaces:**
- Consumes: existing `split_fare(fare, request_fee, offer_fee) -> (u64, u64, u64)` (unchanged).
- Produces: `fn referrer_fee_floor(bps: u16, fare: u64) -> u64`; `RidePay::state_transaction(&self, tx_hash: &String, db: &Database, request_fee_bps: u16, offer_fee_bps: u16, passenger: &String)`; `AppConfig.ride_request_referrer_fee_bps: u16` / `ride_offer_referrer_fee_bps: u16`; `Transaction::state_transaction(&self, db, request_fee_bps: u16, offer_fee_bps: u16)`; same u16 plumbing through `Blockchain` and `Block::add_block_to_chain`. (Task 3 replaces this plumbing with a params struct — keep it mechanical.)

- [ ] **Step 1: Branch + test image**

```bash
git checkout -b treasury-break
```

Create `Dockerfile.test`:

```dockerfile
# Test-runner image: rocksdb (librocksdb-sys) needs clang/libclang for bindgen.
FROM rust:1.86-bookworm
RUN apt-get update && apt-get install -y clang libclang-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
```

```bash
docker build -f Dockerfile.test -t clutch-node-test .
```

- [ ] **Step 2: Add proptest dev-dependency**

In `Cargo.toml` `[dev-dependencies]`:

```toml
[dev-dependencies]
serial_test = "3.1.1"
proptest = "1.5"
```

- [ ] **Step 3: Write the failing tests** — in `ride_pay.rs` replace the `#[cfg(test)]` module's fee tests:

```rust
#[cfg(test)]
mod tests {
    use super::{referrer_fee_floor, split_fare};
    use proptest::prelude::*;

    #[test]
    fn referrer_fee_floor_bps() {
        assert_eq!(referrer_fee_floor(0, 100), 0);
        assert_eq!(referrer_fee_floor(200, 0), 0);
        assert_eq!(referrer_fee_floor(200, 100), 2); // 2% of 100
        // Floor kills the old ceiling distortion (2% of 3 ceiling-rounded to 33%).
        assert_eq!(referrer_fee_floor(200, 3), 0);
        assert_eq!(referrer_fee_floor(200, 49), 0);
        assert_eq!(referrer_fee_floor(200, 50), 1);
        assert_eq!(referrer_fee_floor(10_000, u64::MAX), u64::MAX); // 100%, no overflow
        assert_eq!(referrer_fee_floor(1, 10_000), 1); // 1 bp granularity
    }

    #[test]
    fn split_fare_never_exceeds_fare() {
        assert_eq!(split_fare(100, 2, 2), (2, 2, 96));
        assert_eq!(split_fare(1, 1, 1), (1, 0, 0));
        assert_eq!(split_fare(10, 8, 8), (8, 2, 0));
        assert_eq!(split_fare(50, 0, 0), (0, 0, 50));
    }

    proptest! {
        // Spec §4a: request + offer + driver == fare, exactly, for every input.
        #[test]
        fn fee_split_sums_exactly(fare in any::<u64>(), rbps in 0u16..=10_000, obps in 0u16..=10_000) {
            let (r, o, d) = split_fare(
                fare,
                referrer_fee_floor(rbps, fare),
                referrer_fee_floor(obps, fare),
            );
            prop_assert!(r <= fare && o <= fare - r);
            prop_assert_eq!(r + o + d, fare);
        }

        #[test]
        fn floor_fee_bounded_by_fare(fare in any::<u64>(), bps in 0u16..=10_000) {
            prop_assert!(referrer_fee_floor(bps, fare) <= fare);
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test --lib ride_pay
```

Expected: FAIL — `referrer_fee_floor` not found.

- [ ] **Step 5: Implement** — in `ride_pay.rs` replace `referrer_fee_ceiling` (lines 16-22):

```rust
/// Referrer fee in base units: floor(fare * bps / 10_000). Stored as basis points so
/// fractional percentages need no config migration (spec §4a). u128 intermediate —
/// the product can exceed u64 but the result never does (result <= fare).
fn referrer_fee_floor(bps: u16, fare: u64) -> u64 {
    ((fare as u128 * bps as u128) / 10_000) as u64
}
```

Update the two call sites in `RidePay::state_transaction` (lines 170-177) to `referrer_fee_floor(request_fee_bps, self.fare)` / `referrer_fee_floor(offer_fee_bps, self.fare)`, and its signature to `request_fee_bps: u16, offer_fee_bps: u16`.

Also add the spec-mandated runtime assert inside `split_fare` (§4a "Assert it"), just before the return:

```rust
    debug_assert_eq!(request + offer + driver, fare, "fee split must sum exactly");
```

- [ ] **Step 6: Mechanical rename through the plumbing** — `percent`→`bps`, `u8`→`u16`, everywhere:

- `configuration.rs:20-21`: `pub ride_request_referrer_fee_bps: u16, pub ride_offer_referrer_fee_bps: u16`
- `blockchain.rs`: fields (27-28), constructor params (39-40), initializers (52-53), `import_block` args (123-124), getters (151-157) — rename to `_bps`, type `u16`
- `block.rs:296-302`: `add_block_to_chain(db, block, block_reward_amount: u64, ride_request_referrer_fee_bps: u16, ride_offer_referrer_fee_bps: u16)`; pass-through at 336-341
- `transaction.rs:233-238`: `state_transaction(&self, db, ride_request_referrer_fee_bps: u16, ride_offer_referrer_fee_bps: u16)`; RidePay arm (250-256) passes bps
- All 6 TOMLs: replace `ride_request_referrer_fee_percent = 2` → `ride_request_referrer_fee_bps = 200`, same for offer side
- Find stragglers (incl. `src/main.rs` constructing `Blockchain::new`, and `tests/`) — case-insensitive, because `tests/balance_effects.rs:17` has an uppercase `const REFERRER_FEE_PERCENT`:

```bash
grep -rni "fee_percent" src/ tests/ config/ && echo FOUND-FIX-THESE || echo CLEAN
```

Also fix the positional `2, 2` percent literals passed to `Blockchain::new` in `tests/ride_sharing.rs` and `tests/block_reward.rs` (grep won't catch bare numbers) — they become `200, 200`.

- [ ] **Step 7: Run full test suite**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test
```

Expected: PASS after these test updates:
- `tests/balance_effects.rs`: `const REFERRER_FEE_PERCENT: u8 = 2` → `const REFERRER_FEE_BPS: u16 = 200`. Its RidePay test uses fare 10, and floor(10 × 200 / 10_000) = **0** — no referrer effect is emitted at all (`request_fee > 0` gate). Raise the fare in that test to 100 so the expected referrer delta is 2, and update the driver-remainder assertion to 98 accordingly.
- `tests/referrer_account.rs` carries its own local `referrer_fee_ceiling` copy and does not touch node fee code — it keeps passing unchanged; leave it (its floor migration is cosmetic).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat!: referrer fees in basis points with floor rounding

Replaces ceiling percent (2% of 3 rounded to 33%) with bps floor per
treasury spec 4a. Driver share stays remainder-based; property test
pins request+offer+driver == fare for all inputs.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: ChainInit transaction type (+ new BalanceEffectKind variants)

**Files:**
- Create: `src/node/transactions/chain_init.rs`
- Modify: `src/node/transactions/function_call.rs`, `src/node/transactions/mod.rs`, `src/node/rlp_encoding.rs`, `src/node/transactions/transaction.rs` (match arms only), `src/node/balance_effect.rs:7-16`
- Test: `tests/chain_init.rs`

**Interfaces:**
- Produces (used by Tasks 3-8):
  - `pub struct ChainInit { pub chain_id: u64, pub is_testnet: bool, pub tx_fee: u64, pub ride_request_referrer_fee_bps: u16, pub ride_offer_referrer_fee_bps: u16, pub mint_authority: String, pub faucet_address: String, pub faucet_allocation: u64 }`
  - `ChainInit::get(db: &Database) -> Result<ChainInit, String>` — reads state key `chain_params`
  - `ChainInit::get_total_supply(db: &Database) -> Result<u64, String>` — reads state key `total_supply`, `Ok(0)` if absent
  - `pub const CHAIN_PARAMS_KEY: &[u8]`, `pub const TOTAL_SUPPLY_KEY: &[u8]`
  - `FunctionCall::ChainInit(ChainInit)` — RLP tag 9
  - `BalanceEffectKind::{Mint, Burn, TxFeePaid, TxFeeEarned}`

- [ ] **Step 1: Write the failing test** — `tests/chain_init.rs`:

```rust
use clutch_node::node::database::Database;
use clutch_node::node::transactions::chain_init::ChainInit;
use serial_test::serial;

fn test_params() -> ChainInit {
    ChainInit {
        chain_id: 2077,
        is_testnet: true,
        tx_fee: 1000,
        ride_request_referrer_fee_bps: 200,
        ride_offer_referrer_fee_bps: 200,
        mint_authority: "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20".to_string(),
        faucet_address: "0xdeb4cfb63db134698e1879ea24904df074726cc0".to_string(),
        faucet_allocation: 1_000_000_000_000_000,
    }
}

#[test]
fn chain_init_rlp_roundtrip() {
    let ci = test_params();
    let encoded = clutch_node::node::rlp_encoding::encode(&ci);
    let decoded: ChainInit = clutch_node::node::rlp_encoding::decode(&encoded).unwrap();
    assert_eq!(ci, decoded);
}

#[test]
#[serial]
fn chain_init_rejected_outside_genesis() {
    let db = Database::new_db("test-chain-init-reject");
    let err = test_params()
        .verify_state(&"0xanyone".to_string(), &db)
        .unwrap_err();
    assert!(err.contains("genesis"), "got: {}", err);
    drop(db);
    let mut db = Database::new_db("test-chain-init-reject");
    db.close();
    db.delete_database("test-chain-init-reject").unwrap();
}

#[test]
#[serial]
fn chain_init_state_writes_params_supply_and_faucet() {
    let name = "test-chain-init-state";
    let db = Database::new_db(name);
    let ci = test_params();
    let updates = ci.state_transaction(&db);
    // Apply the storage updates the way add_block_to_chain would.
    let ops: Vec<(&str, &[u8], Option<&[u8]>)> = updates
        .iter()
        .filter_map(|u| u.storage.as_ref())
        .map(|(k, v)| ("state", k.as_slice(), Some(v.as_slice())))
        .collect();
    db.write(ops).unwrap();

    assert_eq!(ChainInit::get(&db).unwrap(), ci);
    assert_eq!(ChainInit::get_total_supply(&db).unwrap(), ci.faucet_allocation);
    let faucet = clutch_node::node::account_state::AccountState::get_current_state(
        &ci.faucet_address, &db,
    );
    assert_eq!(faucet.balance, ci.faucet_allocation);

    let mut db = db;
    db.close();
    db.delete_database(name).unwrap();
}

#[test]
#[serial]
fn chain_init_mainnet_flag_zeroes_supply() {
    let name = "test-chain-init-mainnet";
    let db = Database::new_db(name);
    let ci = ChainInit { is_testnet: false, faucet_allocation: 0, ..test_params() };
    let updates = ci.state_transaction(&db);
    let ops: Vec<(&str, &[u8], Option<&[u8]>)> = updates
        .iter()
        .filter_map(|u| u.storage.as_ref())
        .map(|(k, v)| ("state", k.as_slice(), Some(v.as_slice())))
        .collect();
    db.write(ops).unwrap();
    assert_eq!(ChainInit::get_total_supply(&db).unwrap(), 0);
    let mut db = db;
    db.close();
    db.delete_database(name).unwrap();
}
```

> Database API verified against `src/node/database.rs`: `new_db(&str) -> Database`, `get(&self, cf, key) -> Result<Option<Vec<u8>>, String>`, `write(&self, Vec<(&str, &[u8], Option<&[u8]>)>)`, `close(&mut self)`, `delete_database(&self, name)`. The test code above matches these signatures as written.

- [ ] **Step 2: Run to verify failure**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test --test chain_init
```

Expected: FAIL — module `chain_init` not found.

- [ ] **Step 3: Create `src/node/transactions/chain_init.rs`**

```rust
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::node::account_state::AccountState;
use crate::node::balance_effect::{BalanceEffectKind, StateUpdate};
use crate::node::database::Database;

pub const CHAIN_PARAMS_KEY: &[u8] = b"chain_params";
pub const TOTAL_SUPPLY_KEY: &[u8] = b"total_supply";

/// Consensus parameters, committed to by the genesis hash: this struct rides in the
/// genesis block's single ChainInit transaction, whose hash feeds the block hash that
/// peers compare at p2p handshake. Runtime reads them from state via `get`, never from
/// per-node config — a node with different values gets a different genesis and cannot
/// peer. This closes the block_reward-style consensus-divergence bug class.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChainInit {
    pub chain_id: u64,
    pub is_testnet: bool,
    pub tx_fee: u64,
    pub ride_request_referrer_fee_bps: u16,
    pub ride_offer_referrer_fee_bps: u16,
    pub mint_authority: String,
    pub faucet_address: String,
    pub faucet_allocation: u64,
}

impl ChainInit {
    pub fn get(db: &Database) -> Result<ChainInit, String> {
        match db.get("state", CHAIN_PARAMS_KEY) {
            Ok(Some(v)) => serde_json::from_slice(&v)
                .map_err(|e| format!("corrupt chain_params in state: {}", e)),
            Ok(None) => Err("chain_params missing from state (genesis not imported?)".to_string()),
            Err(e) => Err(format!("failed to read chain_params: {}", e)),
        }
    }

    pub fn get_total_supply(db: &Database) -> Result<u64, String> {
        match db.get("state", TOTAL_SUPPLY_KEY) {
            Ok(Some(v)) => serde_json::from_slice(&v)
                .map_err(|e| format!("corrupt total_supply in state: {}", e)),
            Ok(None) => Ok(0),
            Err(e) => Err(format!("failed to read total_supply: {}", e)),
        }
    }

    pub fn verify_state(&self, _from: &String, _db: &Database) -> Result<(), String> {
        // Genesis import bypasses validate_transaction entirely, so reaching this check
        // means the tx arrived via the pool or a non-genesis block — always reject.
        Err("ChainInit is only valid in the genesis block".to_string())
    }

    pub fn state_transaction(&self, db: &Database) -> Vec<StateUpdate> {
        let initial_supply = if self.is_testnet { self.faucet_allocation } else { 0 };
        let mut updates = vec![
            StateUpdate::storage_only(
                CHAIN_PARAMS_KEY.to_vec(),
                serde_json::to_vec(self).expect("serialize chain params"),
            ),
            StateUpdate::storage_only(
                TOTAL_SUPPLY_KEY.to_vec(),
                serde_json::to_vec(&initial_supply).expect("serialize supply"),
            ),
        ];
        if initial_supply > 0 {
            // faucet_allocation is validated <= i64::MAX at boot (Blockchain::new).
            updates.push(AccountState::apply_balance_change(
                &self.faucet_address,
                initial_supply as i64,
                BalanceEffectKind::Mint,
                None,
                db,
            ));
        }
        updates
    }
}

impl Encodable for ChainInit {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(8);
        stream.append(&self.chain_id);
        stream.append(&(self.is_testnet as u8));
        stream.append(&self.tx_fee);
        stream.append(&self.ride_request_referrer_fee_bps);
        stream.append(&self.ride_offer_referrer_fee_bps);
        stream.append(&self.mint_authority);
        stream.append(&self.faucet_address);
        stream.append(&self.faucet_allocation);
    }
}

impl Decodable for ChainInit {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 8 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        Ok(ChainInit {
            chain_id: rlp.val_at(0)?,
            is_testnet: rlp.val_at::<u8>(1)? != 0,
            tx_fee: rlp.val_at(2)?,
            ride_request_referrer_fee_bps: rlp.val_at(3)?,
            ride_offer_referrer_fee_bps: rlp.val_at(4)?,
            mint_authority: rlp.val_at(5)?,
            faucet_address: rlp.val_at(6)?,
            faucet_allocation: rlp.val_at(7)?,
        })
    }
}
```

- [ ] **Step 4: Wire the enum and effects**

`function_call.rs` — add import, variant, Display arm:

```rust
use super::chain_init::ChainInit;
// in enum FunctionCall:
    ChainInit(ChainInit),
// in Display:
    FunctionCall::ChainInit(args) => write!(f, "ChainInit: {:?}", args),
```

`src/node/transactions/mod.rs` — add `pub mod chain_init;` (mirror how `transfer` is declared).

`rlp_encoding.rs` — encode arm (tag 9, after RideRequestCancel) and decode arm:

```rust
// encode, inside the match:
FunctionCall::ChainInit(args) => {
    stream.begin_list(2);
    stream.append(&9u8); // Tag for ChainInit (genesis-only)
    stream.append(args);
}
// decode, inside the match:
9 => {
    let args: ChainInit = rlp.val_at(1)?;
    Ok(FunctionCall::ChainInit(args))
}
```

(and `use super::transactions::chain_init::ChainInit;` at the top.)

`transaction.rs` — three match arms:

```rust
// verify_state:
FunctionCall::ChainInit(chain_init) => chain_init.verify_state(&self.from, db),
// function_call_type:
FunctionCall::ChainInit(_) => "ChainInit",
// state_transaction:
FunctionCall::ChainInit(chain_init) => chain_init.state_transaction(db),
```

`balance_effect.rs:7-16` — extend the kind enum:

```rust
pub enum BalanceEffectKind {
    TransferOut,
    TransferIn,
    RideAcceptanceDebit,
    RidePayDriverCredit,
    ReferrerRequestFee,
    ReferrerOfferFee,
    RideCancelRefund,
    BlockReward,
    Mint,
    Burn,
    TxFeePaid,
    TxFeeEarned,
}
```

> Follow-up flagged for later plans: clutch-explorer deserializes these kinds — confirm it tolerates unknown variants before deploying a node emitting them.

- [ ] **Step 5: Run tests to verify pass**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test --test chain_init
```

Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat!: ChainInit genesis transaction carrying consensus parameters

New RLP tag 9, genesis-only (verify_state always rejects). Writes
chain_params + total_supply state keys and the testnet faucet credit.
Adds Mint/Burn/TxFeePaid/TxFeeEarned balance-effect kinds.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Genesis rework — params from state, block reward removed, testnet-gated faucet

**Files:**
- Modify: `src/node/configuration.rs`, `src/node/transactions/transaction.rs:43-58,233-261`, `src/node/blocks/block.rs:49-65,264-280,296-400`, `src/node/blockchain.rs`, `src/main.rs`, `src/node/wss/websocket.rs:355-370`, `config/node/*.toml` (6 files)
- Test: `tests/chain_genesis.rs`; update `Blockchain::new` call sites in `tests/*.rs` and `src/main.rs`

**Interfaces:**
- Consumes: `ChainInit` (Task 2), bps fee fns (Task 1).
- Produces (relied on by Tasks 4-8):
  - `Blockchain::new(name: String, author_public_key: String, author_secret_key: String, developer_mode: bool, authorities: Vec<String>, chain_init: ChainInit) -> Blockchain` — panics at boot if `request_bps + offer_bps > 10_000`, if `faucet_allocation > i64::MAX as u64`, or if `!is_testnet && faucet_allocation > 0` (spec §4.5 "fails loudly").
  - `Transaction::new_genesis_transactions(params: &ChainInit) -> Vec<Transaction>` — single ChainInit tx from `0xGENESIS`.
  - `Block::new_genesis_block(params: &ChainInit) -> Block`; `Block::genesis_import_block(db: &Database, params: &ChainInit)`.
  - `Block::add_block_to_chain(db: &Database, block: &Block) -> Result<(), String>` — resolves params internally via `params_for_block`.
  - `Transaction::state_transaction(&self, db: &Database, params: &ChainInit) -> Vec<StateUpdate>`.
  - `AppConfig` gains `chain_id: u64, is_testnet: bool, tx_fee: u64, mint_authority: String, faucet_address: String, faucet_allocation: u64`; **loses** `block_reward_amount`.
  - Block rewards are gone: no `BlockReward` effects are emitted anywhere.

- [ ] **Step 1: Write the failing test** — `tests/chain_genesis.rs`:

```rust
use clutch_node::node::blockchain::Blockchain;
use clutch_node::node::transactions::chain_init::ChainInit;
use serial_test::serial;

fn test_chain_init() -> ChainInit {
    ChainInit {
        chain_id: 2077,
        is_testnet: true,
        tx_fee: 1000,
        ride_request_referrer_fee_bps: 200,
        ride_offer_referrer_fee_bps: 200,
        mint_authority: "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20".to_string(),
        faucet_address: "0xdeb4cfb63db134698e1879ea24904df074726cc0".to_string(),
        faucet_allocation: 1_000_000_000_000_000,
    }
}

fn new_test_chain(name: &str, ci: ChainInit) -> Blockchain {
    Blockchain::new(
        name.to_string(),
        "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20".to_string(),
        "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509".to_string(),
        true, // developer_mode: DB wiped on shutdown_blockchain
        vec!["0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20".to_string()],
        ci,
    )
}

#[test]
#[serial]
fn genesis_funds_faucet_and_stores_params() {
    let ci = test_chain_init();
    let mut chain = new_test_chain("test-genesis-testnet", ci.clone());
    assert_eq!(chain.get_account_balance(&ci.faucet_address), ci.faucet_allocation);
    let (params, supply) = chain.get_chain_info().unwrap();
    assert_eq!(params, ci);
    assert_eq!(supply, ci.faucet_allocation);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
fn genesis_hash_commits_to_chain_params() {
    let mut a = new_test_chain("test-genesis-a", test_chain_init());
    let hash_a = a.get_genesis_block().unwrap().unwrap().hash;
    a.shutdown_blockchain();

    let mut b = new_test_chain(
        "test-genesis-b",
        ChainInit { chain_id: 1, ..test_chain_init() },
    );
    let hash_b = b.get_genesis_block().unwrap().unwrap().hash;
    b.shutdown_blockchain();

    assert_ne!(hash_a, hash_b, "different chain params must yield different genesis hashes");
}

#[test]
#[serial]
fn mainnet_genesis_has_zero_supply() {
    let ci = ChainInit { is_testnet: false, faucet_allocation: 0, ..test_chain_init() };
    let mut chain = new_test_chain("test-genesis-mainnet", ci.clone());
    assert_eq!(chain.get_account_balance(&ci.faucet_address), 0);
    let (_, supply) = chain.get_chain_info().unwrap();
    assert_eq!(supply, 0);
    chain.shutdown_blockchain();
}

#[test]
#[serial]
#[should_panic(expected = "faucet")]
fn mainnet_with_faucet_allocation_fails_loudly() {
    let ci = ChainInit { is_testnet: false, faucet_allocation: 1, ..test_chain_init() };
    let _ = new_test_chain("test-genesis-loud", ci);
}
```

- [ ] **Step 2: Run to verify failure**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test --test chain_genesis
```

Expected: FAIL — `Blockchain::new` arity / `get_chain_info` missing.

- [ ] **Step 3: Config fields** — `configuration.rs` `AppConfig`: remove `block_reward_amount`, add:

```rust
    pub chain_id: u64,
    pub is_testnet: bool,
    pub tx_fee: u64,
    pub mint_authority: String,
    pub faucet_address: String,
    pub faucet_allocation: u64,
```

All 6 TOMLs — remove `block_reward_amount = 50`, add (identical values in every file; without identical values the nodes can't peer):

```toml
chain_id = 2077
is_testnet = true
tx_fee = 1000
mint_authority = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20"
faucet_address = "0xdeb4cfb63db134698e1879ea24904df074726cc0"
faucet_allocation = 1000000000000000
```

> `mint_authority` here is node1's dev authority key — testnet convenience only. Production genesis uses a dedicated treasury key from the key ceremony (never a validator key).

- [ ] **Step 4: Genesis transactions** — `transaction.rs:43-58` replace `new_genesis_transactions`:

```rust
    pub fn new_genesis_transactions(params: &ChainInit) -> Vec<Transaction> {
        vec![Self::new_transaction(
            FROM_GENESIS.to_string(),
            0,
            FunctionCall::ChainInit(params.clone()),
        )]
    }
```

(add `use super::chain_init::ChainInit;` — the old faucet Transfer and its comment are deleted; the `0xGENESIS`-debit clamp path in `account_state.rs` is no longer exercised because ChainInit only credits.)

- [ ] **Step 5: Params-based state dispatch** — `transaction.rs:233-261` change signature and RidePay arm:

```rust
    pub fn state_transaction(&self, db: &Database, params: &ChainInit) -> Vec<StateUpdate> {
        let mut states = match &self.data {
            // ... all arms unchanged except RidePay:
            FunctionCall::RidePay(ride_pay) => ride_pay.state_transaction(
                &self.hash,
                db,
                params.ride_request_referrer_fee_bps,
                params.ride_offer_referrer_fee_bps,
                &self.from,
            ),
            FunctionCall::ChainInit(chain_init) => chain_init.state_transaction(db),
            // ...
        };
        // nonce push unchanged
        states
    }
```

- [ ] **Step 6: Block-side wiring** — `block.rs`:

`new_genesis_block` (49-65) and `genesis_import_block` (264-280) take `params: &ChainInit` and pass through (`Transaction::new_genesis_transactions(params)`; `Self::add_block_to_chain(db, &genesis_block)`).

Replace `add_block_to_chain` signature (296-302) and delete the block-reward section (375-400):

```rust
    /// Resolve consensus params: from state for normal blocks; from the block's own
    /// ChainInit for the genesis import (its params aren't in state yet).
    fn params_for_block(db: &Database, block: &Block) -> Result<ChainInit, String> {
        if block.index == 0 {
            block
                .transactions
                .iter()
                .find_map(|tx| match &tx.data {
                    FunctionCall::ChainInit(ci) => Some(ci.clone()),
                    _ => None,
                })
                .ok_or_else(|| "genesis block missing ChainInit transaction".to_string())
        } else {
            ChainInit::get(db)
        }
    }

    pub fn add_block_to_chain(db: &Database, block: &Block) -> Result<(), String> {
        let params = Self::params_for_block(db, block)?;
```

The rest of the body is the existing code with two edits. The per-tx loop head (old lines 336-341) becomes:

```rust
        for (tx_index, tx) in block.transactions.iter().enumerate() {
            let updates = tx.state_transaction(&db, &params);
```

and the entire `if block.index > 0 && block_reward_amount > 0 { ... }` block (old lines 375-400) is **deleted** — block rewards no longer exist (spec §4.2). Task 5 puts the fee credit in that slot.

(imports: `use crate::node::transactions::chain_init::ChainInit;` and `use crate::node::transactions::function_call::FunctionCall;`; `persist_block_effects` stays imported — Task 5 uses it for the fee credit.)

- [ ] **Step 7: Blockchain constructor + boot validation** — `blockchain.rs`: remove fields `block_reward_amount`, `ride_request_referrer_fee_bps`, `ride_offer_referrer_fee_bps` and their getters; add field `chain_init: ChainInit`:

```rust
    pub fn new(
        name: String,
        author_public_key: String,
        author_secret_key: String,
        developer_mode: bool,
        authorities: Vec<String>,
        chain_init: ChainInit,
    ) -> Blockchain {
        // Fail loudly at boot on inconsistent economics — spec §4.5. Genesis must never
        // be importable with a mainnet flag and a faucet pre-mint.
        assert!(
            chain_init.ride_request_referrer_fee_bps as u32
                + chain_init.ride_offer_referrer_fee_bps as u32
                <= 10_000,
            "referrer fee bps sum exceeds 100%"
        );
        assert!(
            chain_init.faucet_allocation <= i64::MAX as u64,
            "faucet_allocation exceeds i64::MAX (balance deltas are i64)"
        );
        assert!(
            chain_init.is_testnet || chain_init.faucet_allocation == 0,
            "non-testnet chain must have zero faucet_allocation (a surviving faucet pre-mint destroys the peg)"
        );

        let db = Database::new_db(&name);
        let step_duration = 60 / authorities.len() as u64;
        let blockchain = Blockchain {
            name,
            db,
            developer_mode,
            consensus: Aura::new(authorities, step_duration),
            author_public_key,
            author_secret_key,
            chain_init,
        };

        Block::genesis_import_block(&blockchain.db, &blockchain.chain_init);
        blockchain
    }

    /// Consensus params + total supply, read from state (post-genesis truth).
    pub fn get_chain_info(&self) -> Result<(ChainInit, u64), String> {
        let params = ChainInit::get(&self.db)?;
        let supply = ChainInit::get_total_supply(&self.db)?;
        Ok((params, supply))
    }
```

`import_block` (115-128): `Block::add_block_to_chain(&self.db, block)?;` — no param plumbing.

- [ ] **Step 8: main.rs + websocket compat** — `src/main.rs` declares `mod node;` and imports via `use node::...` (NOT the `clutch_node` lib path — that resolves to a *different* copy of the types and E0308s against the bin's `Blockchain::new`). Add alongside the existing `use node::blockchain::Blockchain;`:

```rust
use node::transactions::chain_init::ChainInit;
```

then build the struct from config and pass it:

```rust
    let chain_init = ChainInit {
        chain_id: config.chain_id,
        is_testnet: config.is_testnet,
        tx_fee: config.tx_fee,
        ride_request_referrer_fee_bps: config.ride_request_referrer_fee_bps,
        ride_offer_referrer_fee_bps: config.ride_offer_referrer_fee_bps,
        mint_authority: config.mint_authority.clone(),
        faucet_address: config.faucet_address.clone(),
        faucet_allocation: config.faucet_allocation,
    };

    let blockchain = Blockchain::new(
        config.blockchain_name.clone(),
        config.author_public_key.clone(),
        config.author_secret_key.clone(),
        config.developer_mode,
        config.authorities.clone(),
        chain_init,
    );
```

(match the existing call's argument sources — the current call passes the same config fields plus the three now-removed reward/percent values.)

`websocket.rs` `handle_get_block_by_index` (~355-370): the getter `blockchain.block_reward_amount()` is gone — keep the JSON field for explorer compatibility with a literal:

```rust
    // ponytail: block rewards removed; field kept as 0 until clutch-explorer drops it.
    let block_reward: u64 = 0;
```

- [ ] **Step 9: Fix remaining call sites**

```bash
grep -rn "block_reward\|Blockchain::new\|new_genesis\|genesis_import_block\|add_block_to_chain\|state_transaction(" src/ tests/ --include="*.rs" | grep -v "docs/"
```

(`genesis_import_block` matters: `tests/balance_effects.rs:49` calls `Block::genesis_import_block(&db)` directly for its DB setup — it now needs a `&ChainInit` argument; use the test-fixture `ci()` pattern.)

Update every caller to the new signatures (tests use the `new_test_chain` helper pattern from Step 1). `tests/block_reward.rs`: **delete the file** — rewards no longer exist; Task 5 adds `tests/tx_fee.rs` as its successor.

- [ ] **Step 10: Run full suite**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test
```

Expected: PASS.

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "feat!: genesis carries ChainInit; params read from state; block rewards removed

Genesis hash now commits to chain_id/fees/mint authority, so mismatched
nodes cannot peer (fixes the config-divergence class block_reward had).
Faucet allocation is testnet-gated and fails loudly on mainnet flags.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: chain_id in the signed transaction payload

**Files:**
- Modify: `src/node/transactions/transaction.rs`, `src/node/rlp_encoding.rs:107-159`, plus every `Transaction::new_transaction` / raw-RLP fixture call site (`tests/rlp_decode_test.rs`, in-file tests)
- Test: `transaction.rs` `#[cfg(test)]` + `tests/` updates

**Interfaces:**
- Consumes: `ChainInit::get` (Task 2).
- Produces:
  - `Transaction.chain_id: u64` (serde + RLP).
  - Wire RLP: **8-item list** `[from, nonce, chain_id, signature_r, signature_s, signature_v, hash, data]`.
  - Hash preimage: **4-item list** `[from (no 0x), nonce, chain_id, data]` (Keccak-256).
  - `Transaction::new_transaction(from: String, nonce: u64, chain_id: u64, function_call: FunctionCall) -> Transaction`.
  - `validate_transaction` rejects `tx.chain_id != chain_params.chain_id`.
- **Cross-repo contract (SDK/hub follow-up plans):** the JS SDK and hub faucet must adopt the same 8-item wire format and 4-item preimage, same field order, or every tx is rejected.

- [ ] **Step 1: Write the failing test** — add to `transaction.rs` tests (the `sdk_style_tx` helper gets a `chain_id` param in Step 3; write the new expectations first):

```rust
    #[test]
    fn hash_commits_to_chain_id() {
        let a = Transaction::new_transaction(
            "0xdeb4cfb63db134698e1879ea24904df074726cc0".to_string(),
            1,
            2077,
            FunctionCall::Transfer(Transfer { to: "0xA".to_string(), value: 10 }),
        );
        let b = Transaction::new_transaction(
            "0xdeb4cfb63db134698e1879ea24904df074726cc0".to_string(),
            1,
            1,
            FunctionCall::Transfer(Transfer { to: "0xA".to_string(), value: 10 }),
        );
        assert_ne!(a.hash, b.hash, "same tx on a different chain must hash differently");
    }
```

- [ ] **Step 2: Run to verify failure** — `new_transaction` arity error:

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test --lib transactions::transaction
```

- [ ] **Step 3: Implement**

`transaction.rs`:
- struct: add `pub chain_id: u64,` after `nonce`.
- `new_transaction(from, nonce, chain_id, function_call)` — set the field.
- `new_genesis_transactions`: `Self::new_transaction(FROM_GENESIS.to_string(), 0, params.chain_id, FunctionCall::ChainInit(params.clone()))`.
- `calculate_hash` (65-77): 4-item preimage —

```rust
        let mut stream = RlpStream::new();
        stream.begin_list(4);
        stream.append(&from_no_prefix.to_string());
        stream.append(&self.nonce);
        stream.append(&self.chain_id);
        stream.append(&self.data);
```

(update the doc comment: preimage is now `[from (no 0x), nonce, chain_id, data]`; SDK/faucet must match.)
- `validate_transaction` (168-175): load params once and check chain:

```rust
    pub fn validate_transaction(&self, db: &Database) -> Result<(), String> {
        self.verify_hash()?;
        self.verify_signature()?;
        let params = ChainInit::get(db)?;
        if self.chain_id != params.chain_id {
            return Err(format!(
                "Verification failed: transaction chain_id {} does not match chain {}",
                self.chain_id, params.chain_id
            ));
        }
        self.verify_nonce(db)?;
        self.verify_state(db)?;
        Ok(())
    }
```

`rlp_encoding.rs:107-159` — Transaction 8 items:

```rust
impl Encodable for Transaction {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(8);
        stream.append(&self.from);
        stream.append(&self.nonce);
        stream.append(&self.chain_id);
        stream.append(&self.signature_r);
        stream.append(&self.signature_s);
        let signature_v_as_u64 = self.signature_v as u64;
        stream.append(&signature_v_as_u64);
        stream.append(&self.hash);
        stream.append(&self.data);
    }
}
```

Decode — full replacement (the `from` string/bytes dual decoding stays byte-identical):

```rust
impl Decodable for Transaction {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 8 {
            return Err(DecoderError::RlpIncorrectListLen);
        }

        // Handle 'from' field which may be encoded as binary data by JavaScript RLP library
        let from = {
            let from_item = rlp.at(0)?;
            let from_value = if let Ok(string_val) = from_item.as_val::<String>() {
                string_val
            } else if let Ok(bytes_val) = from_item.as_val::<Vec<u8>>() {
                hex::encode(&bytes_val)
            } else {
                return Err(DecoderError::Custom("Unable to decode 'from' field as string or bytes"));
            };
            if from_value.starts_with("0x") {
                from_value
            } else {
                format!("0x{}", from_value)
            }
        };

        Ok(Transaction {
            from,
            nonce: rlp.val_at(1)?,
            chain_id: rlp.val_at(2)?,
            signature_r: rlp.val_at(3)?,
            signature_s: rlp.val_at(4)?,
            signature_v: rlp.val_at::<u64>(5)? as i32,
            hash: rlp.val_at(6)?,
            data: rlp.val_at(7)?,
        })
    }
}
```

- [ ] **Step 4: Update fixtures** — in `transaction.rs` tests, the shared builder becomes (full replacement):

```rust
    /// Builds a full signed tx for a `data` payload the way the SDK will in v3: the hash is
    /// Keccak-256 over the unsigned `[from (no 0x), nonce, chain_id, data]` preimage, so these
    /// bytes are self-consistent by construction.
    fn sdk_style_tx(from_clean: &str, nonce: u64, chain_id: u64, data_rlp: &[u8]) -> Transaction {
        let mut unsigned = RlpStream::new_list(4);
        unsigned.append(&from_clean.to_string());
        unsigned.append(&nonce);
        unsigned.append(&chain_id);
        unsigned.append_raw(data_rlp, 1);
        let mut hasher = Keccak256::new();
        hasher.update(unsigned.out().as_ref());
        let hash_hex = hex::encode(hasher.finalize());

        let dummy = "cd".repeat(32);
        let mut full = RlpStream::new_list(8);
        full.append(&from_clean.to_string());
        full.append(&nonce);
        full.append(&chain_id);
        full.append(&dummy);
        full.append(&dummy);
        full.append(&28u64);
        full.append(&hash_hex);
        full.append_raw(data_rlp, 1);
        crate::node::rlp_encoding::decode(full.out().as_ref()).expect("decode sdk-style tx")
    }
```

- Update its two callers (`WIRE_*` tests) to pass `2077`.
- `accepts_sdk_generated_ride_acceptance_hash`: the pinned raw hex predates chain_id and cannot be regenerated without the SDK — **replace** with a `sdk_style_tx`-built RideAcceptance equivalent and this comment: `// TODO(sdk-v3): re-pin with real clutch-hub-sdk-js output once the SDK adds chain_id.`
- `accepts_faucet_style_transfer_hash` — full replacement of the builder section (mirrors the future faucet format):

```rust
        let mut unsigned = RlpStream::new_list(4);
        unsigned.append(&from_clean.to_string());
        unsigned.append(&nonce);
        unsigned.append(&2077u64);
        unsigned.append_raw(data_rlp.as_ref(), 1);
        let mut hasher = Keccak256::new();
        hasher.update(unsigned.out().as_ref());
        let hash_hex = hex::encode(hasher.finalize());

        let dummy = "cd".repeat(32);
        let mut full = RlpStream::new_list(8);
        full.append(&from_clean.to_string());
        full.append(&nonce);
        full.append(&2077u64);
        full.append(&dummy);
        full.append(&dummy);
        full.append(&28u64);
        full.append(&hash_hex);
        full.append_raw(data_rlp.as_ref(), 1);
        let raw = full.out();
```

- `tf` helpers in `transaction.rs`/`blockchain.rs` tests: add chain_id `2077` (any constant — these never hit chain validation).
- `src/node/rlp_encoding.rs` in-file tests: add `chain_id: 2077,` to the `Transaction { ... }` struct literals (~lines 360-412).
- `tests/rlp_decode_test.rs`: only the `new_transaction` arity fix is required (its raw 7-item hex fixture feeds a decode test that never asserts — println-only; optionally rebuild it as an 8-item fixture with the builder above).

```bash
grep -rn "new_transaction(\|new_list(7)\|new_list(3)" src/ tests/ --include="*.rs"
```

- [ ] **Step 5: Wrong-chain rejection test** — append to `tests/chain_genesis.rs`:

```rust
#[test]
#[serial]
fn wrong_chain_id_rejected_at_pool() {
    use clutch_node::node::transactions::function_call::FunctionCall;
    use clutch_node::node::transactions::transaction::Transaction;
    use clutch_node::node::transactions::transfer::Transfer;

    let ci = test_chain_init(); // chain_id 2077
    let mut chain = new_test_chain("test-wrong-chain", ci.clone());

    let mut tx = Transaction::new_transaction(
        ci.faucet_address.clone(),
        1,
        1, // wrong chain
        FunctionCall::Transfer(Transfer {
            to: "0x1111111111111111111111111111111111111111".to_string(),
            value: 1,
        }),
    );
    tx.sign("d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f");

    let err = chain.add_transaction_to_pool(&tx).unwrap_err();
    assert!(err.contains("chain_id"), "got: {}", err);
    chain.shutdown_blockchain();
}
```

- [ ] **Step 6: Full suite**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat!: chain_id in transaction hash preimage and wire format

Signatures now commit to the chain — a testnet Mint can never replay on
mainnet. Wire RLP is 8 items; preimage is [from, nonce, chain_id, data].
SDK and hub faucet must adopt the same format (coordinated release).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Flat transaction fee paid to the block author

**Files:**
- Modify: `src/node/transactions/transaction.rs`, `src/node/transactions/transfer.rs`, `src/node/transactions/ride_acceptance.rs`, `src/node/transactions/ride_cancel.rs`, `src/node/account_state.rs`, `src/node/blocks/block.rs` (fee credit where reward code was)
- Test: `tests/tx_fee.rs` (successor to deleted `tests/block_reward.rs`)

**Fee routing table — the load-bearing design.** A sender's balance key must be written **at most once per tx** (deferred-batch last-write-wins). Which types touch the sender's balance in `state_transaction` (verified against source):

| Type | Sender balance touched in-type? | Fee handling |
|---|---|---|
| Transfer | yes — debits `value` | merged in-type via `apply_balance_change_with_fee` |
| Burn (Task 7) | yes — debits `amount` | merged in-type |
| RideAcceptance | **yes — debits full fare escrow** (`ride_acceptance.rs:159`, sender == passenger enforced) | merged in-type |
| RideCancel | **when sender == passenger** — refund credit hits sender (`ride_cancel.rs:138`) | handled fully in-type: merge into refund when sender==passenger, standalone debit when sender==driver |
| RideRequest, RideOffer, RidePay, RideRequestCancel | no balance writes | central standalone fee debit in `Transaction::state_transaction` |
| Mint, ChainInit | — | fee-exempt |

**Interfaces:**
- Consumes: `ChainInit.tx_fee`, `BalanceEffectKind::{TxFeePaid, TxFeeEarned}`.
- Produces:
  - `Transaction::fee_exempt(&self) -> bool` — true for `Mint` (Task 6; write the match arm with `ChainInit` only for now) and `ChainInit`.
  - `Transaction::sender_direct_debit(&self) -> u64` — **statically-known** sender debits only: `Transfer.value` now, `Burn.amount` in Task 7, else 0. RideAcceptance's fare debit is db-dependent, so its fare+fee sufficiency is enforced inside `RideAcceptance::verify_state` instead (below).
  - `Transaction::effective_fee(&self, block_author: &str, params: &ChainInit) -> u64` — 0 if exempt or sender == author (canonical compare), else `params.tx_fee`.
  - `Transaction::state_transaction(&self, db, params: &ChainInit, block_author: &str)`.
  - `Transfer::state_transaction(&self, from, db, fee: u64)`; `RideAcceptance::state_transaction(&self, from, tx_hash, db, fee: u64)`; `RideCancel::state_transaction(&self, from, tx_hash, db, fee: u64)` (gains `from`).
  - `AccountState::apply_balance_change_with_fee(public_key, main_delta: i64, fee: u64, kind, counterparty, db) -> Vec<StateUpdate>`.
  - `RideAcceptance::verify_state` additionally requires `balance >= fare + tx_fee` (reads `ChainInit::get(db)`).
  - `add_block_to_chain`: single block-level author credit `TxFeeEarned` = Σ effective fees.
- Validation rule: non-exempt tx requires `balance >= sender_direct_debit() + tx_fee` (checked add) in `validate_transaction`. Pool-side check has no author context, so it conservatively requires the fee even for the author's own tx. Known conservative edge, accepted + documented: a passenger whose entire balance is escrowed cannot RideCancel until they hold `tx_fee` loose CLT (the incoming refund doesn't count at validation time).

- [ ] **Step 1: Write the failing tests** — `tests/tx_fee.rs`:

```rust
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
```

> `signed_transfer` takes `&str` — adjust the helper's params or call with `poor.address_key.as_str()`.

> `author_new_block` requires this node to be the current Aura slot author; with a single authority (as here) every slot is ours, so it always succeeds.

> The author-pays-no-fee case is tested in Task 6's `author_own_tx_pays_no_fee` — it needs Mint to fund the author collision-free (funding the author by Transfer in a self-authored block would hit the known deferred-batch same-account collision: TransferIn write + fee-credit write, last one wins).

- [ ] **Step 2: Run to verify failure**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test --test tx_fee
```

Expected: FAIL — balances off by fee amounts / helpers missing.

- [ ] **Step 3: AccountState helper** — `account_state.rs` after `apply_balance_change`:

```rust
    /// Sender leg with fee merged into ONE balance write and two audit effects.
    /// Two separate apply_balance_change calls on the same account within a tx would
    /// each read pre-block state and the deferred batch keeps only the last write —
    /// silently dropping one debit. One write, split effects, no collision.
    pub fn apply_balance_change_with_fee(
        public_key: &String,
        main_delta: i64,
        fee: u64,
        kind: BalanceEffectKind,
        counterparty: Option<String>,
        db: &Database,
    ) -> Vec<StateUpdate> {
        if fee == 0 {
            return vec![Self::apply_balance_change(public_key, main_delta, kind, counterparty, db)];
        }
        let canonical = canonical_account_address(public_key);
        let combined = main_delta - fee as i64;
        let (key, value) = Self::update_account_state_key(public_key, combined, db);
        vec![
            StateUpdate {
                storage: Some((key, value)),
                effect: Some(BalanceEffect {
                    address: canonical.clone(),
                    delta: main_delta,
                    kind,
                    counterparty,
                }),
            },
            StateUpdate {
                storage: None, // effect-only: storage already carries the combined write
                effect: Some(BalanceEffect {
                    address: canonical,
                    delta: -(fee as i64),
                    kind: BalanceEffectKind::TxFeePaid,
                    counterparty: None,
                }),
            },
        ]
    }
```

- [ ] **Step 4: Transfer merges its fee** — `transfer.rs`:

```rust
    pub fn state_transaction(&self, from: &String, db: &Database, fee: u64) -> Vec<StateUpdate> {
        let transfer_value: i64 = self.value as i64;
        let to = self.to.clone();

        let mut updates = AccountState::apply_balance_change_with_fee(
            from,
            -transfer_value,
            fee,
            BalanceEffectKind::TransferOut,
            Some(to.clone()),
            db,
        );
        updates.push(AccountState::apply_balance_change(
            &to,
            transfer_value,
            BalanceEffectKind::TransferIn,
            Some(from.clone()),
            db,
        ));
        updates
    }
```

- [ ] **Step 5: Transaction fee helpers + dispatch** — `transaction.rs`:

```rust
    /// Mint is exempt: the treasury authority mints TO users and may itself hold zero
    /// balance. ChainInit is genesis-only. Everything else pays the flat fee.
    fn fee_exempt(&self) -> bool {
        matches!(&self.data, FunctionCall::ChainInit(_))
        // Task 6 extends this to: FunctionCall::Mint(_) | FunctionCall::ChainInit(_)
    }

    /// CLT the sender's balance is directly debited by this tx (excluding the fee).
    fn sender_direct_debit(&self) -> u64 {
        match &self.data {
            FunctionCall::Transfer(t) => t.value,
            // Task 7 adds: FunctionCall::Burn(b) => b.amount,
            _ => 0,
        }
    }

    /// ponytail: author's own tx nets zero fee — a debit and an aggregate credit on the
    /// same account in one block collide in the deferred batch (last write wins), so we
    /// skip both sides instead. Lift with incremental intra-block state.
    pub fn effective_fee(&self, block_author: &str, params: &ChainInit) -> u64 {
        use crate::node::transactions::address::canonical_account_address;
        if self.fee_exempt()
            || canonical_account_address(&self.from) == canonical_account_address(block_author)
        {
            0
        } else {
            params.tx_fee
        }
    }
```

In `validate_transaction`, after the chain_id check:

```rust
        if !self.fee_exempt() {
            let required = self
                .sender_direct_debit()
                .checked_add(params.tx_fee)
                .ok_or("Verification failed: amount + fee overflows u64")?;
            let balance = AccountState::get_current_state(&self.from, db).balance;
            if balance < required {
                return Err(format!(
                    "Verification failed: insufficient balance for amount + fee. Required: {}, available: {}",
                    required, balance
                ));
            }
        }
```

`state_transaction` gains the author and routes fees per the routing table:

```rust
    pub fn state_transaction(
        &self,
        db: &Database,
        params: &ChainInit,
        block_author: &str,
    ) -> Vec<StateUpdate> {
        let fee = self.effective_fee(block_author, params);
        let mut states = match &self.data {
            FunctionCall::Transfer(transfer) => transfer.state_transaction(&self.from, db, fee),
            FunctionCall::RideAcceptance(ride_acceptance) => {
                ride_acceptance.state_transaction(&self.from, &self.hash, db, fee)
            }
            FunctionCall::RideCancel(ride_cancel) => {
                ride_cancel.state_transaction(&self.from, &self.hash, db, fee)
            }
            // Task 7 adds: FunctionCall::Burn(burn) => burn.state_transaction(&self.from, &self.hash, db, fee),
            // ... remaining arms unchanged ...
        };

        // Standalone fee debit ONLY for types that never write the sender's balance
        // in-type (see routing table). Types that do (Transfer, Burn, RideAcceptance,
        // RideCancel) merge the fee themselves — two writes to one account key in a tx
        // collide in the deferred batch (last write wins).
        let fee_handled_in_type = matches!(
            &self.data,
            FunctionCall::Transfer(_)
                | FunctionCall::RideAcceptance(_)
                | FunctionCall::RideCancel(_)
            // Task 7 adds: | FunctionCall::Burn(_)
        );
        if fee > 0 && !fee_handled_in_type {
            states.push(AccountState::apply_balance_change(
                &self.from,
                -(fee as i64),
                BalanceEffectKind::TxFeePaid,
                None,
                db,
            ));
        }

        // nonce push unchanged
        ...
        states
    }
```

`ride_acceptance.rs` — escrow debit merges the fee. Signature `state_transaction(&self, from: &String, tx_hash: &String, db: &Database, fee: u64)` (unchanged params otherwise); replace the `passenger_update` block (lines 158-165) and splice the resulting Vec:

```rust
        let transfer_value: i64 = ride_offer.fare as i64;
        // ponytail: fee merged into the single escrow write — a separate TxFeePaid write
        // on the same account would collide in the deferred batch. Lift with
        // incremental intra-block state.
        let passenger_updates = AccountState::apply_balance_change_with_fee(
            from,
            -transfer_value,
            fee,
            BalanceEffectKind::RideAcceptanceDebit,
            None,
            db,
        );

        let mut updates = vec![
            StateUpdate::storage_only(ride_acceptance_key, ride_acceptance_value),
            StateUpdate::storage_only(ride_request_acceptance_key, ride_request_acceptance_value),
            StateUpdate::storage_only(ride_offer_acceptance_key, ride_offer_acceptance_value),
        ];
        updates.extend(passenger_updates);
        updates
```

`ride_acceptance.rs::verify_state` — the balance check (lines 80-87) must cover fare + fee (the fare is db-dependent, so the central `sender_direct_debit` check can't see it):

```rust
            let tx_fee = crate::node::transactions::chain_init::ChainInit::get(db)?.tx_fee;
            let required = ride_offer
                .fare
                .checked_add(tx_fee)
                .ok_or("fare + fee overflows u64")?;
            let passenger_account_state = AccountState::get_current_state(from, db);
            if passenger_account_state.balance < required {
                return Err(format!(
                    "The account balance is insufficient to cover the fare plus the transaction fee. \
                     Account balance is: {}, fare: {}, fee: {}",
                    passenger_account_state.balance, ride_offer.fare, tx_fee
                ));
            }
```

`ride_cancel.rs` — signature `state_transaction(&self, from: &String, tx_hash: &String, db: &Database, fee: u64)`; replace the `passenger_update` block (lines 136-150):

```rust
        let remaining_amount = (ride_offer.fare as i64) - (fare_paid as i64);

        use crate::node::transactions::address::canonical_account_address;
        let sender_is_passenger =
            canonical_account_address(from) == canonical_account_address(&passenger);

        // ponytail: when the passenger cancels, refund credit and fee debit hit the SAME
        // account — merge into one write. Driver-cancel: driver's key is otherwise
        // untouched, standalone fee debit is safe.
        let mut updates = vec![
            StateUpdate::storage_only(ride_cancel_key, ride_cancel_value),
            StateUpdate::storage_only(ride_acceptance_cancel_key, ride_acceptance_cancel_value),
        ];
        if sender_is_passenger {
            updates.extend(AccountState::apply_balance_change_with_fee(
                &passenger,
                remaining_amount,
                fee,
                BalanceEffectKind::RideCancelRefund,
                None,
                db,
            ));
        } else {
            updates.push(AccountState::apply_balance_change(
                &passenger,
                remaining_amount,
                BalanceEffectKind::RideCancelRefund,
                None,
                db,
            ));
            if fee > 0 {
                updates.push(AccountState::apply_balance_change(
                    from,
                    -(fee as i64),
                    BalanceEffectKind::TxFeePaid,
                    None,
                    db,
                ));
            }
        }
        updates
```

- [ ] **Step 6: Author credit in add_block_to_chain** — `block.rs`, in the slot where the reward code was (after the tx loop, before the write):

```rust
        // Fees replace block rewards: one aggregate author credit per block (single
        // write — per-tx credits would collide in the deferred batch). Fee revenue is
        // backed CLT changing hands, so the reserve invariant is untouched.
        // ponytail: residual ceiling (pre-existing class, same as the old block reward):
        // if any tx in a fee-paying block ALSO credits the author's balance (Transfer to
        // author, Mint to author, author-as-driver RidePay), that credit collides with
        // this write and is lost. Operational rule: validator accounts are not app
        // accounts. Lift with incremental intra-block state.
        let total_fees: u64 = block
            .transactions
            .iter()
            .map(|tx| tx.effective_fee(&block.author, &params))
            .sum();
        if block.index > 0 && total_fees > 0 {
            let fee_update = AccountState::apply_balance_change(
                &block.author,
                total_fees as i64,
                BalanceEffectKind::TxFeeEarned,
                None,
                &db,
            );
            if let Some((key, value)) = fee_update.storage {
                cf_storage.push("state".to_string());
                keys_storage.push(key);
                values_storage.push(value);
            }
            if let Some(effect) = fee_update.effect {
                for (key, value) in persist_block_effects(
                    block.index as u64,
                    block.timestamp,
                    std::slice::from_ref(&effect),
                ) {
                    cf_storage.push("state".to_string());
                    keys_storage.push(key);
                    values_storage.push(value);
                }
            }
        }
```

And the tx loop passes the author: `tx.state_transaction(&db, &params, &block.author)`.

- [ ] **Step 7: Run full suite**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test
```

Expected: PASS after substantial test surgery — the fee rule breaks existing ride-flow tests structurally, not just numerically:

- **`tests/ride_sharing.rs`**: the driver (`0x8f19...d5e9`) holds **zero balance** and sends RideOffer txs — under the fee rule a zero-balance sender fails validation, block 2 fails import, and the whole downstream flow silently never runs (the test swallows import errors with `error!()` + `break` around lines 66-71). Two mandatory fixes: (1) add a faucet→driver funding Transfer block at the start of the flow (renumber nonces/block indexes for everything after), (2) **make import failures hard failures** — replace the `error!` + `break` with `panic!`/`.expect()` so a dead flow can never pass green.
- **`tests/balance_effects.rs`**: same funding requirement for any zero-balance sender, and every expected balance shifts by `TX_FEE` per non-exempt tx sent.
- Adjust expected balances everywhere by the fee; don't weaken assertions.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat!: flat tx fee to block author replaces block reward

Every non-exempt tx pays chain_params.tx_fee; validation requires
balance >= direct debit + fee. Sender fee merges into one balance write;
author credited once per block. Spam now has a price.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Mint transaction — treasury-authorized, exactly-once credit_ref

**Files:**
- Create: `src/node/transactions/mint.rs`
- Modify: `function_call.rs`, `mod.rs`, `rlp_encoding.rs` (tag 6), `transaction.rs` (arms + `fee_exempt` + Mint dispatch passes tx_hash), `block.rs` (supply delta)
- Test: `tests/mint_burn.rs`

**Interfaces:**
- Consumes: `ChainInit::{get, get_total_supply}`, `TOTAL_SUPPLY_KEY`, `BalanceEffectKind::Mint`.
- Produces:
  - `pub struct Mint { pub to: String, pub amount: u64, pub credit_ref: String }` — RLP `[to, amount, credit_ref]`, tag 6. `credit_ref` = 64 lowercase hex chars (hash of the treasury intent id), no `0x`.
  - `Mint::verify_state(&self, from: &String, db: &Database) -> Result<(), String>` — authority, amount bounds, ref format, ref-unseen.
  - `Mint::state_transaction(&self, tx_hash: &String, db: &Database) -> Vec<StateUpdate>` — credit + `processed_ref_{ref}` marker (value = tx hash).
  - `pub fn processed_ref_key(reference: &str) -> Vec<u8>` — `format!("processed_ref_{}", reference)`.
  - `add_block_to_chain` computes `supply_delta` per block and single-writes `total_supply`.
  - `Transaction::fee_exempt` now includes `Mint`.
- **Cross-repo contract:** the Treasury Service builds Mint txs with `credit_ref = hex(keccak256(intent_id))`, signs with the mint-authority key, same RLP as here.

- [ ] **Step 1: Write the failing tests** — `tests/mint_burn.rs` (Mint half; Burn tests arrive in Task 7):

```rust
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
```

> Note: the mint credit_ref here (`cc…cc`) must differ from REF_A/REF_B — refs are globally exactly-once.

- [ ] **Step 2: Run to verify failure** — module `mint` not found:

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test --test mint_burn
```

- [ ] **Step 3: Create `src/node/transactions/mint.rs`**

```rust
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::node::account_state::AccountState;
use crate::node::balance_effect::{BalanceEffectKind, StateUpdate};
use crate::node::database::Database;

use super::address::canonical_account_address;
use super::chain_init::ChainInit;

/// Exactly-once ref marker: `processed_ref_{64-hex}` in the state CF, value = tx hash.
/// Shared by Mint (credit_ref) and Burn (redemption_ref) — refs are keccak256 hashes of
/// treasury intent ids, so one namespace cannot collide across the two uses.
pub fn processed_ref_key(reference: &str) -> Vec<u8> {
    format!("processed_ref_{}", reference).into_bytes()
}

pub fn ref_is_valid(reference: &str) -> bool {
    reference.len() == 64 && reference.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

pub fn ref_already_processed(db: &Database, reference: &str) -> Result<bool, String> {
    match db.get("state", &processed_ref_key(reference)) {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(false),
        Err(e) => Err(format!("failed to read processed ref: {}", e)),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mint {
    pub to: String,
    pub amount: u64,
    pub credit_ref: String,
}

impl Mint {
    pub fn verify_state(&self, from: &String, db: &Database) -> Result<(), String> {
        let params = ChainInit::get(db)?;
        if canonical_account_address(from) != canonical_account_address(&params.mint_authority) {
            return Err(format!(
                "Mint rejected: '{}' is not the mint authority",
                from
            ));
        }
        if self.amount == 0 {
            return Err("Mint rejected: amount must be positive".to_string());
        }
        if self.amount > i64::MAX as u64 {
            return Err("Mint rejected: amount exceeds i64::MAX (balance deltas are i64)".to_string());
        }
        if !ref_is_valid(&self.credit_ref) {
            return Err("Mint rejected: credit_ref must be 64 lowercase hex chars".to_string());
        }
        if ref_already_processed(db, &self.credit_ref)? {
            return Err(format!(
                "Mint rejected: credit_ref '{}' already processed (exactly-once)",
                self.credit_ref
            ));
        }
        Ok(())
    }

    pub fn state_transaction(&self, tx_hash: &String, db: &Database) -> Vec<StateUpdate> {
        vec![
            AccountState::apply_balance_change(
                &self.to,
                self.amount as i64,
                BalanceEffectKind::Mint,
                None,
                db,
            ),
            StateUpdate::storage_only(
                processed_ref_key(&self.credit_ref),
                tx_hash.clone().into_bytes(),
            ),
        ]
    }
}

impl Encodable for Mint {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(3);
        stream.append(&self.to);
        stream.append(&self.amount);
        stream.append(&self.credit_ref);
    }
}

impl Decodable for Mint {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 3 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        Ok(Mint {
            to: rlp.val_at(0)?,
            amount: rlp.val_at(1)?,
            credit_ref: rlp.val_at(2)?,
        })
    }
}
```

- [ ] **Step 4: Wire it** — mirror Task 2's wiring:
- `mod.rs`: `pub mod mint;`
- `function_call.rs`: `Mint(Mint)` variant + Display arm
- `rlp_encoding.rs`: encode/decode arms, tag `6u8`
- `transaction.rs`: `verify_state` arm → `mint.verify_state(&self.from, db)`; `function_call_type` → `"Mint"`; `state_transaction` arm → `mint.state_transaction(&self.hash, db)`; `fee_exempt` → `matches!(&self.data, FunctionCall::Mint(_) | FunctionCall::ChainInit(_))`

- [ ] **Step 5: Supply delta in add_block_to_chain** — `block.rs`, next to the fee-credit block:

```rust
        // Supply changes once per block: per-tx read-modify-writes of the single
        // total_supply key would collide in the deferred batch (last write wins,
        // e.g. two Burns in one block). Sum first, then one read + one write.
        let mut supply_delta: i128 = 0;
        for tx in &block.transactions {
            match &tx.data {
                FunctionCall::Mint(m) => supply_delta += m.amount as i128,
                _ => {}
            }
        }
        if block.index > 0 && supply_delta != 0 {
            let current = ChainInit::get_total_supply(db)? as i128;
            let next = current + supply_delta;
            // Cap at i64::MAX, not u64::MAX: supply <= i64::MAX implies every balance
            // <= i64::MAX, keeping all deltas representable in i64 (Transfer casts
            // `value as i64` — a balance above i64::MAX would wrap negative).
            if next < 0 || next > i64::MAX as i128 {
                return Err(format!(
                    "total_supply out of range: {} + {} = {}",
                    current, supply_delta, next
                ));
            }
            cf_storage.push("state".to_string());
            keys_storage.push(chain_init::TOTAL_SUPPLY_KEY.to_vec());
            values_storage.push(serde_json::to_vec(&(next as u64)).expect("serialize supply"));
        }
```

(import `chain_init` module; Task 7 adds the `Burn` match arm here.)

- [ ] **Step 6: Run tests**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test --test mint_burn
```

Expected: PASS (5 tests).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat!: Mint transaction with authority check and exactly-once credit_ref

RLP tag 6. Only chain_params.mint_authority may mint; credit_ref
(64-hex, hash of the treasury intent id) is a write-once state marker,
so a replayed or duplicated mint intent can never credit twice.
total_supply updates once per block.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: Burn transaction — permissionless, redemption_ref for payout matching

**Files:**
- Create: `src/node/transactions/burn.rs`
- Modify: `function_call.rs`, `mod.rs`, `rlp_encoding.rs` (tag 7), `transaction.rs` (arms, `sender_direct_debit`, fee routing), `block.rs` (supply arm)
- Test: `tests/mint_burn.rs` (extend)

**Interfaces:**
- Consumes: `processed_ref_key` / `ref_is_valid` / `ref_already_processed` (Task 6), `apply_balance_change_with_fee` (Task 5).
- Produces:
  - `pub struct Burn { pub amount: u64, pub redemption_ref: Option<String> }` — RLP `[amount, redemption_ref-or-empty-string]`, tag 7. Ref optional: plain burns allowed; redemptions carry `hex(keccak256(intent_id))` so the Treasury payout worker matches Burn → intent.
  - `Burn::verify_state(&self, from, db)`; `Burn::state_transaction(&self, from, tx_hash, db, fee) -> Vec<StateUpdate>` — single sender balance write of `-(amount+fee)`, effects `Burn` + `TxFeePaid`, optional ref marker.
  - `Transaction::sender_direct_debit` includes `Burn.amount`; the standalone-fee branch in `Transaction::state_transaction` excludes Burn (fee merged like Transfer).
- Burn **pays the fee** (spam resistance; the burner has balance by definition).

- [ ] **Step 1: Write the failing tests** — append to `tests/mint_burn.rs`:

```rust
use clutch_node::node::transactions::burn::Burn;

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
```

- [ ] **Step 2: Run to verify failure**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test --test mint_burn
```

- [ ] **Step 3: Create `src/node/transactions/burn.rs`**

```rust
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::node::account_state::AccountState;
use crate::node::balance_effect::{BalanceEffectKind, StateUpdate};
use crate::node::database::Database;

use super::mint::{processed_ref_key, ref_already_processed, ref_is_valid};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Burn {
    pub amount: u64,
    /// hex(keccak256(intent_id)) for treasury redemptions; None for a plain burn.
    pub redemption_ref: Option<String>,
}

impl Burn {
    pub fn verify_state(&self, _from: &String, db: &Database) -> Result<(), String> {
        if self.amount == 0 {
            return Err("Burn rejected: amount must be positive".to_string());
        }
        if self.amount > i64::MAX as u64 {
            return Err("Burn rejected: amount exceeds i64::MAX".to_string());
        }
        if let Some(r) = &self.redemption_ref {
            if !ref_is_valid(r) {
                return Err("Burn rejected: redemption_ref must be 64 lowercase hex chars".to_string());
            }
            if ref_already_processed(db, r)? {
                return Err(format!(
                    "Burn rejected: redemption_ref '{}' already processed",
                    r
                ));
            }
        }
        // Balance sufficiency (amount + fee) is enforced centrally in validate_transaction.
        Ok(())
    }

    pub fn state_transaction(
        &self,
        from: &String,
        tx_hash: &String,
        db: &Database,
        fee: u64,
    ) -> Vec<StateUpdate> {
        let mut updates = AccountState::apply_balance_change_with_fee(
            from,
            -(self.amount as i64),
            fee,
            BalanceEffectKind::Burn,
            None,
            db,
        );
        if let Some(r) = &self.redemption_ref {
            updates.push(StateUpdate::storage_only(
                processed_ref_key(r),
                tx_hash.clone().into_bytes(),
            ));
        }
        updates
    }
}

impl Encodable for Burn {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(2);
        stream.append(&self.amount);
        // Same optional-string convention as referrers: empty string = None.
        let ref_str = self.redemption_ref.clone().unwrap_or_default();
        stream.append(&ref_str);
    }
}

impl Decodable for Burn {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let ref_str: String = rlp.val_at(1)?;
        Ok(Burn {
            amount: rlp.val_at(0)?,
            redemption_ref: if ref_str.is_empty() { None } else { Some(ref_str) },
        })
    }
}
```

- [ ] **Step 4: Wire it** — `mod.rs`, `function_call.rs`, `rlp_encoding.rs` tag `7u8` (mirror Task 6). `transaction.rs`:
- `verify_state` arm → `burn.verify_state(&self.from, db)`
- `function_call_type` → `"Burn"`
- `state_transaction` arm → `burn.state_transaction(&self.from, &self.hash, db, fee)` (the `fee` local from Task 5 Step 5)
- `sender_direct_debit`: add `FunctionCall::Burn(b) => b.amount,`
- `fee_handled_in_type` match: add `| FunctionCall::Burn(_)` (Burn merges its own fee like Transfer)
- `block.rs` supply loop: add `FunctionCall::Burn(b) => supply_delta -= b.amount as i128,`

- [ ] **Step 5: Run full suite, then commit**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test
```

```bash
git add -A
git commit -m "feat!: Burn transaction with optional exactly-once redemption_ref

RLP tag 7, permissionless. Redemptions carry hex(keccak256(intent_id))
so the treasury payout worker matches burns to intents; plain burns
allowed. Burner pays amount + fee in one balance write; supply shrinks.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: `get_chain_info` RPC

**Files:**
- Modify: `src/node/wss/websocket.rs` (match arm ~line 150 + handler)
- Test: `tests/chain_genesis.rs` already covers `Blockchain::get_chain_info`; this task adds the RPC surface + serialization test

**Interfaces:**
- Consumes: `Blockchain::get_chain_info()` (Task 3).
- Produces JSON-RPC method `get_chain_info`, no params, result:

```json
{
  "chain_id": 2077, "is_testnet": true, "tx_fee": 1000,
  "ride_request_referrer_fee_bps": 200, "ride_offer_referrer_fee_bps": 200,
  "mint_authority": "0x...", "total_supply": 1000000000000000,
  "latest_block_index": 42
}
```

- **Cross-repo contract:** Treasury reconciliation reads `total_supply` here; hub-api faucet reads `is_testnet`/`chain_id` to fail loudly on non-testnet chains.

- [ ] **Step 1: Handler** — `websocket.rs`, add match arm after `get_block_by_index`:

```rust
            "get_chain_info" => {
                Self::handle_get_chain_info(id, blockchain).await
            }
```

and the handler (mirror `handle_get_account_balance`'s shape):

```rust
    async fn handle_get_chain_info(
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
    ) -> Option<String> {
        let blockchain = blockchain.lock().await;
        let latest_index = match blockchain.get_latest_block() {
            Ok(Some(b)) => b.index,
            _ => 0,
        };
        match blockchain.get_chain_info() {
            Ok((params, total_supply)) => Some(json_rpc_success_response(
                serde_json::json!({
                    "chain_id": params.chain_id,
                    "is_testnet": params.is_testnet,
                    "tx_fee": params.tx_fee,
                    "ride_request_referrer_fee_bps": params.ride_request_referrer_fee_bps,
                    "ride_offer_referrer_fee_bps": params.ride_offer_referrer_fee_bps,
                    "mint_authority": params.mint_authority,
                    "total_supply": total_supply,
                    "latest_block_index": latest_index,
                }),
                id,
            )),
            Err(e) => {
                let error_msg = format!("Failed to get chain info: {}", e);
                error!("{}", error_msg);
                Some(json_rpc_error_response(-32000, &error_msg, id))
            }
        }
    }
```

- [ ] **Step 2: Test** — append to `tests/chain_genesis.rs` (integration test crates are independent — the two `signed_*` helpers are duplicated here from `tests/mint_burn.rs` by design):

```rust
#[test]
#[serial]
fn chain_info_supply_tracks_mint_and_burn() {
    use clutch_node::node::transactions::burn::Burn;
    use clutch_node::node::transactions::function_call::FunctionCall;
    use clutch_node::node::transactions::mint::Mint;
    use clutch_node::node::transactions::transaction::Transaction;

    const AUTHOR_SK: &str = "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";
    const FAUCET_SK: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
    const USER: &str = "0x4444444444444444444444444444444444444444";

    let ci = test_chain_init(); // chain_id 2077, mint_authority = author, testnet
    let mut chain = new_test_chain("test-supply-e2e", ci.clone());

    let (_, supply_genesis) = chain.get_chain_info().unwrap();
    assert_eq!(supply_genesis, ci.faucet_allocation);

    // Mint block: +5_000_000 to USER.
    let mut mint = Transaction::new_transaction(
        ci.mint_authority.clone(),
        1,
        ci.chain_id,
        FunctionCall::Mint(Mint {
            to: USER.to_string(),
            amount: 5_000_000,
            credit_ref: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                .to_string(),
        }),
    );
    mint.sign(AUTHOR_SK);
    chain.add_transaction_to_pool(&mint).unwrap();
    chain.author_new_block().unwrap();

    let (_, supply_after_mint) = chain.get_chain_info().unwrap();
    assert_eq!(supply_after_mint, supply_genesis + 5_000_000);

    // Burn block: faucet burns 2_000_000 (fee moves CLT, supply drops by burn only).
    let mut burn = Transaction::new_transaction(
        ci.faucet_address.clone(),
        1,
        ci.chain_id,
        FunctionCall::Burn(Burn {
            amount: 2_000_000,
            redemption_ref: Some(
                "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string(),
            ),
        }),
    );
    burn.sign(FAUCET_SK);
    chain.add_transaction_to_pool(&burn).unwrap();
    chain.author_new_block().unwrap();

    let (_, supply_after_burn) = chain.get_chain_info().unwrap();
    assert_eq!(supply_after_burn, supply_after_mint - 2_000_000);

    chain.shutdown_blockchain();
}
```

- [ ] **Step 3: Run, commit**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test
```

```bash
git add -A
git commit -m "feat: get_chain_info RPC exposing chain params and total supply

Treasury reconciliation's on-chain supply source; hub faucet's
testnet-flag check.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 9: Suite green, docs, and 3-node stack smoke test

**Files:**
- Modify: `docs/state_keys.csv`, `CLAUDE.md` (this repo's), any straggling tests
- Verify: full suite + `docker-compose.yml` 3-node net

- [ ] **Step 1: Sweep for stragglers**

```bash
grep -rn "block_reward\|fee_percent\|referrer_fee_ceiling" src/ tests/ config/ --include="*.rs" --include="*.toml"
```

Expected: only the `block_reward: u64 = 0` compat literal in `websocket.rs` and historical mentions in CHANGELOG. Fix anything else.

- [ ] **Step 2: Document state keys** — append to `docs/state_keys.csv` in its existing two-column `key,type` format:

```csv
chain_params,ChainInit (JSON)
total_supply,u64 (JSON)
processed_ref_{64-hex},tx hash (exactly-once Mint/Burn ref marker)
```

- [ ] **Step 3: Update `CLAUDE.md`** — Transaction Types list (+Mint tag 6, +Burn tag 7, +ChainInit tag 9, genesis-only), RPC list (+`get_chain_info`), Config section (new keys, removed `block_reward_amount`, bps rename), and the tx-hash convention line (preimage now includes chain_id).

- [ ] **Step 4: Full suite**

```bash
docker run --rm -v "${PWD}:/app" -v clutch-cargo-cache:/usr/local/cargo/registry -w /app clutch-node-test cargo test
```

Expected: PASS, zero ignored failures.

- [ ] **Step 5: 3-node stack smoke** — this repo's compose is **pull-only** (`image: ghcr.io/clutchprotocol/clutch-node:latest`, no `build:` directives) — `--build` is a no-op and would run the OLD published binary against the new TOMLs (crash-loop on the removed `block_reward_amount` key). Build the branch image locally and retag it over the compose image name first. `node2-docker.toml`/`node3-docker.toml` must carry the identical new chain params (Task 3) or the nodes will refuse to peer — that refusal is itself a feature test:

```powershell
docker build -t ghcr.io/clutchprotocol/clutch-node:latest .
docker compose up -d
Start-Sleep -Seconds 30
docker compose logs node1 --tail 20   # expect: blocks importing, no errors
docker compose logs node2 --tail 20   # expect: synced via handshake (same genesis hash)
```

Then verify `get_chain_info` over WebSocket (Node one-liner, no install needed inside the sdk repo's node_modules — or use any ws client):

```powershell
node -e "const W=require('ws');const w=new W('ws://localhost:8081/ws');w.on('open',()=>w.send(JSON.stringify({jsonrpc:'2.0',id:1,method:'get_chain_info',params:null})));w.on('message',m=>{console.log(m.toString());process.exit(0)})"
```

Expected: JSON with `chain_id: 2077`, `total_supply: 1000000000000000`, `is_testnet: true`.

Negative check (params commitment): temporarily set `chain_id = 9999` in `config/node/node3-docker.toml`, `docker compose up -d node3` (image already built above), expect node3's log to show handshake/genesis mismatch and no sync; revert.

```powershell
docker compose down -v
```

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "docs: state keys, CLAUDE.md, and stack smoke for treasury node break

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

- [ ] **Step 7: STOP — user review.** Do not merge to `main`, do not push. Present the branch diff. Downstream repos break until their follow-up plans land: clutch-deploy TOML copies (new config keys), clutch-hub-api (chain_id in faucet + JSON tx), clutch-hub-sdk-js (8-item RLP, chain_id, GraphQL Int→String), clutch-explorer (block_reward=0, new effect kinds).

---

## Cross-repo follow-ups (explicitly OUT of this plan)

1. **clutch-deploy**: copy the new/renamed TOML keys into `clutch-deploy/config/node/*.toml` — the deploy stack mounts its own configs and will crash-loop on missing fields until then.
2. **clutch-hub-sdk-js**: chain_id in signing (8-item RLP, 4-item preimage), `$fare: Int!` → String scalars, bigint threading, quote verification before signing.
3. **clutch-hub-api**: faucet adds chain_id + queries `get_chain_info` to refuse non-testnet; GraphQL scalar change; unsigned-tx blob gains chain_id.
4. **clutch-explorer**: breaks at **genesis**, not eventually — every genesis block now contains a ChainInit tx (tag 9) and all txs carry chain_id in an 8-item wire format, so indexing-from-zero fails immediately if its transaction parser mirrors the node enum. Must tolerate/index Mint/Burn/ChainInit (tags 6/7/9), the chain_id field, and the new `Mint/Burn/TxFeePaid/TxFeeEarned` effect kinds; drop block_reward column eventually; index total_supply.
5. **clutch-hub-demo-app**: consumes the SDK via `file:../clutch-hub-sdk-js` — breaks on the next `predev` SDK build after follow-up 2 lands (fare scalar, bigint amounts). Needs its own pass (plus the top-up/redeem screens per the dossier).
6. **clutch-docs**: workspace convention — document the new tx types (Mint/Burn/ChainInit), `get_chain_info`, the chain_id-in-preimage signing change, and the fee model.
7. **clutch-treasury** (new repo): Plans B/C — service skeletons per the dossier.
