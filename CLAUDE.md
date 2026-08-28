# clutch-node — Blockchain Core

Rust node implementing Aura (Proof-of-Authority) consensus, custom RLP-encoded transactions, libp2p gossip/sync, RocksDB storage, and a WebSocket JSON-RPC server. See the parent `../CLAUDE.md` for the multi-repo workspace, ports, and cross-repo flow — this file covers internals only.

## Source Layout (everything lives under `src/node/`)

| Path | Purpose |
|------|---------|
| `src/main.rs` | Entry: clap `--env <name>` → `AppConfig::load_configuration` → `setup_tracing` → `Blockchain::new` → `start_network_services` |
| `src/lib.rs` | Exposes `pub mod node` so integration tests can `use clutch_node::node::...` |
| `src/node/blockchain.rs` | Central facade: owns `Database` + `Aura`; `import_block`, `author_new_block`, `add_transaction_to_pool`, all `list_*` queries |
| `src/node/node_services.rs` | Spawns the tokio tasks: libp2p server, WebSocket server, 1s block-authoring loop, initial peer sync; Ctrl+C shutdown |
| `src/node/aura.rs`, `consensus.rs` | Aura impl of the `Consensus` trait: `slot = timestamp / step_duration`, author = `authorities[slot % len]`; `step_duration = 60 / authorities.len()` |
| `src/node/blocks/block.rs` | Block struct (SHA-256 hash), validation, `add_block_to_chain` (single atomic RocksDB WriteBatch), genesis creation |
| `src/node/transactions/` | One file per tx type + `transaction.rs` (envelope, Keccak-256 hash matching the SDK/faucet, secp256k1 sig), `function_call.rs` (enum), `transaction_pool.rs` (mempool in RocksDB) |
| `src/node/account_state.rs` | Balance/nonce state; `apply_balance_change` returns `StateUpdate` (storage write + optional `BalanceEffect`) |
| `src/node/balance_effect.rs` | Balance-effect audit records persisted per tx / per block / per account (explorer & RPC consume these) |
| `src/node/p2p_server/` | libp2p: `server.rs` (swarm, gossipsub + mdns + request-response), `gossipsub_handler.rs` (incoming tx/block), `handshake.rs` + `get_block_header/bodies` (sync protocol), `commands.rs` (mpsc command enum other tasks use to talk to the swarm) |
| `src/node/wss/websocket.rs` | WebSocket JSON-RPC 2.0 server — all RPC methods live here |
| `src/node/rlp_encoding.rs` | Hand-written `Encodable`/`Decodable` for Transaction, Block, FunctionCall, sync messages + generic `encode`/`decode` |
| `src/node/database.rs` | RocksDB wrapper; column families: `block`, `state`, `blockchain`, `tx_pool` |
| `src/node/configuration.rs` | `AppConfig` loaded from `config/node/{env}.toml` + `APP_*` env overrides |
| `src/node/metric.rs` | Prometheus gauges (`latest_block_index`, `latest_block`) served via axum on `serve_metric_addr`. `latest_block_index` is published from the STORED block at startup as well as on every `add_block_to_chain` — it used to be set only by the latter, so it read 0 from boot until the next block arrived and a synced idle node reported an empty chain. |
| `src/node/signature_keys.rs`, `coordinate.rs`, `time_utils.rs`, `seq.rs`, `tracing.rs`, `file_utils.rs` | secp256k1 sign/verify+recovery, lat/lng, unix time, Seq log sink, tracing setup, JSON dumps to `output/` |

## Transaction Flow

1. Signed tx arrives via WS RPC (`send_transaction` JSON or `send_raw_transaction` hex RLP) or via gossipsub (`gossipsub_handler.rs`).
2. `Blockchain::add_transaction_to_pool` → `Transaction::validate_transaction`: signature (recover & compare to `from`), nonce (`== last + 1`), then per-type `verify_state` (e.g. RideRequest checks balance ≥ fare and no concurrent open request via `passenger_concurrent.rs`). Valid txs land in the `tx_pool` CF and are re-gossiped.
3. Authoring loop (`node_services.rs::start_authoring_job`, every 1s) calls `author_new_block`: drains pool, builds+signs block, then `import_block`. Aura rejects it unless this node is the current slot's author, so most ticks are no-ops (`Err` logged at debug).
4. `import_block` = `verify_block_author` (Aura slot check) + `validate_block` (sig, index, prev_hash) + re-validate all txs + `Block::add_block_to_chain`, which batches into one `db.write()`: block, latest-block pointer, per-tx state updates (`state_transaction`), balance effects, one aggregate `tx_fee` credit to the block author, `total_supply` delta from any Mint/Burn, tx_pool deletions. Accepted blocks are gossiped; peers import the same way.
5. Sync: on startup, a node sends an RLP `Handshake` to the first connected peer, then pulls `GetBlockHeaders`/`GetBlockBodies` over libp2p request-response.

## Transaction Types

`FunctionCall` enum in `src/node/transactions/function_call.rs`: Transfer, RideRequest, RideOffer, RideAcceptance, RidePay, RideCancel, Mint, Burn, RideRequestCancel, ChainInit. Each variant's struct file defines `verify_state` (validation) and `state_transaction` (state writes + balance effects). To add a type: new file + enum variant, wire `verify_state`/`state_transaction`/`function_call_type` matches in `transaction.rs`, and add RLP tag arms in `rlp_encoding.rs`. **RLP tags are not contiguous**: RideRequestCancel is tag `8`, Mint is tag `6`, Burn is tag `7`; tags must match the JS SDK's encoder exactly. **ChainInit is tag `9` and genesis-only** — it carries consensus parameters (`chain_id`, `is_testnet`, `tx_fee`, `mint_authority`, faucet allocation, referrer-fee bps) into state at block 0 and is rejected by `verify_state` at any other height.

## RPC (WebSocket JSON-RPC 2.0)

All methods are matched by string in `WebSocket::handle_json_rpc_request` in `src/node/wss/websocket.rs`. Current methods: `send_transaction`, `send_raw_transaction`, `get_next_nonce`, `get_account_balance`, `get_account_balance_effects`, `get_block_by_index`, `get_chain_info`, `list_ride_requests`, `list_ride_offers`, `list_active_trips`, `list_completed_trips`, `list_recent_trips`. (`import_block`/`author_new_block` are `Blockchain` facade methods, not RPC-exposed strings — see Source Layout above.) `get_chain_info` returns the genesis-committed consensus params plus `total_supply`, `latest_block_index`, and **`is_syncing` / `best_peer_block_index` / `blocks_behind`** — the last three so a caller can tell "this is the tip" from "this is how far I have got". Without them a node ~115,000 blocks behind answered cheerfully and the treasury believed it for a day. They are derived from the peer handshakes the sync path already receives (`p2p_server/sync_state.rs`), keeping the HIGHEST height seen so a lagging peer cannot lower the apparent tip; `best_peer_block_index == 0` means no peer has been heard from, which is reported as not-syncing because a lone node is alone rather than behind. `total_supply` is serialized as a **decimal string**, not a bare JSON number — it's the one field that can exceed 2^53 and lose precision in JS. To add a new method: write a `handle_*` fn (parse params with an inline serde struct, lock `blockchain`, return `json_rpc_success_response`/`json_rpc_error_response`), add a match arm, expose any new query on `Blockchain`, then update clutch-hub-api → SDK → docs per workspace convention.

## Config

- Files: `config/node/{default,node1,node2,node3}.toml`, selected by `--env <name>` (default `default`). Env overrides use `APP_` prefix (e.g. `APP_LOG_LEVEL`); `.env` is loaded via dotenv. Config path is **relative to cwd** — run from the repo root.
- `default` ≈ node1 (authority 1, ws 8081, p2p 4001, metrics 3001, no bootstrap, local Seq). node2/node3 differ in: `blockchain_name` (separate DB dir), author keypair (authorities 2/3), ports (8082/4002/3002, 8083/4003/3003), and `bootstrap_nodes` — `/ip4/127.0.0.1/tcp/4001` (node1 on the same host; mdns also discovers local peers).
- This repo's `docker-compose.yml` uses `node2-docker.toml`/`node3-docker.toml` (`--env node2-docker`), which bootstrap via `/dns4/node1/tcp/4001` — env override is not an option because `bootstrap_nodes` is a `Vec<String>` and the config loader does no list parsing. clutch-deploy mounts its own config copies (`clutch-deploy/config/node/*.toml`, also `/dns4/node1/...`) and is unaffected by this repo's TOMLs.
- All three well-known authority keypairs (and the genesis-funded faucet account `0xdeb4...6cc0`, which now holds `faucet_allocation` — `1e15` base units, i.e. $1B at the 1 USD = 1,000,000 CLT peg — rather than the old `i64::MAX`) are committed in configs/tests — dev-only keys.
- **`developer_mode = true` DELETES the RocksDB on shutdown** (`shutdown_blockchain` → `cleanup_db` → `delete_database`) and dumps chain+pool JSON to `output/`. It reads like a debugging convenience and is a chain-destroying flag.

  All three stage node configs carried it for a month. Every deploy erased the chain of whichever node completed its graceful stop inside the 30s grace period, while the ones SIGKILLed first kept theirs — so the data loss moved between nodes and was blamed on the missing volume (a real but separate bug, fixed earlier), then on resyncing, then on the deploy script. Symptom downstream: the treasury read a supply frozen at genesis, judged its reserve against it, and submitted mints into it.

  `shutdown_blockchain` now **refuses to delete when `DB_PATH` is set**, treating that as the signal the data is meant to outlive the process. Unset `DB_PATH` for a throwaway chain; never rely on the flag alone.
- DB path: `{DB_PATH or cwd}/{blockchain_name}.db`.
- **Consensus params now live in config and are committed to state by the genesis `ChainInit` tx**, not hardcoded: `chain_id` (u64, e.g. `2077`), `is_testnet` (bool), `tx_fee` (u64, flat fee per tx paid to the block author), `mint_authority` (address allowed to sign `Mint`), `faucet_address`/`faucet_allocation` (genesis funding), `ride_request_referrer_fee_bps`/`ride_offer_referrer_fee_bps` (u16 basis points, floor-rounded — renamed from the old percent-based `fee_percent` fields). All three node configs must carry **identical** values for these or peers refuse the p2p handshake (genesis hash mismatch — see Gotchas).
- **`block_reward_amount` is removed** — block rewards no longer exist; the block author is now paid via `tx_fee` revenue instead (see Transaction Flow and the Gotchas fee-crediting note).

## Commands

```powershell
cargo run                          # single node, config/node/default.toml
cargo run -- --env node2           # pick another config
cargo build --release
cargo test                         # unit + integration tests
docker compose up -d               # 3-node local net from ghcr image (this repo's docker-compose.yml)
.\scripts\docker-build.ps1         # local image build
```

- Tests in `tests/` (`ride_sharing.rs`, `author_block.rs`, `balance_effects.rs`, `transfer.rs`, `referrer_account.rs`, `rlp_decode_test.rs`, `p2p_server_tests.rs`, `chain_genesis.rs`, `chain_init.rs`, `mint_burn.rs`, `tx_fee.rs`, `db_error_handling.rs`) hit **real RocksDB instances in the cwd**; DB-touching tests are `#[serial]` (serial_test crate) — keep that attribute on any new test that opens a database, and clean up via `blockchain.shutdown_blockchain()` (developer_mode) — which only deletes when `DB_PATH` is unset, so do not set it in a test environment or the cleanup silently stops happening and stray `clutch-node-*.db` dirs accumulate.
- CI: `.github/workflows/docker-build-push.yml` builds multi-arch images to GHCR + Docker Hub on push to main / `v*` tags, then repository-dispatches `deploy-stage` to clutch-deploy. There is **no CI job running `cargo test`** — run tests locally before pushing.

## Gotchas / Conventions

- Error handling is `Result<_, String>` everywhere (no anyhow/thiserror); DB read/write failures on hot paths (`get_latest_block`, `add_block_to_chain`) propagate as `Err`, not `panic!`.
- **One transaction per account per block, and one claim per exactly-once ref per block.** Block state is validated then applied as one deferred RocksDB batch (commit at end of `add_block_to_chain`), so a second tx from the same account would validate/apply against stale pre-block state — two Transfers from one account mint CLT via last-write-wins on the balance key. The same staleness breaks exactly-once for the shared `processed_ref_{ref}` marker: two txs in one block bearing one ref (Mint's `credit_ref` or Burn's `redemption_ref`, one namespace) both see it unused and their two identical marker writes collapse. `validate_transactions` rejects a block with a duplicate sender (`first_duplicate_sender`) or a repeated ref (`first_duplicate_ref` — needed because Burn is permissionless, so the sender guard no longer implies ref uniqueness); `Blockchain::drop_intra_block_conflicts` enforces both at authoring time (losers wait for later blocks). Lift only once intra-block state is applied incrementally.
- Logging via `tracing` macros; logs also ship to Seq (`seq_url`/`seq_api_key` in config).
- State keys are string-prefixed in the `state` CF: `account_state_{addr}`, `account_nonce_{addr}`, `ride_request_{hash}`, `ride_request_{hash}:ride_acceptance`, `ride_acceptance_{hash}:fare_paid`, `tx_effects_{hash}`, `block_effects_{height}`, `account_effect_{addr}_{reverse_height}...`, plus three added by the treasury release: `chain_params` (the genesis `ChainInit`, the runtime's only source of consensus params), `total_supply` (u64, moved once per block by Mint/Burn), and `processed_ref_{64-hex}` (the exactly-once marker, one namespace shared by Mint and Burn). See `docs/state_keys.csv` and `balance_effect.rs`.
- **The block author's `tx_fee` credit merges into a pending write rather than being appended.** Because the batch is deferred, appending the aggregate credit after the per-tx loop made it the *last* write to the author's balance key — which silently discarded the author's own transaction in the same block (its debit vanished while the recipient kept the funds: unbacked CLT). `add_block_to_chain` therefore looks for an already-staged write to `account_state_{author}` and folds the fee into that value; a failed merge returns `Err` and aborts the import rather than falling back to appending. `effective_fee` separately returns 0 when the sender *is* the author, so no self-fee is charged. See `tests/tx_fee.rs::author_own_transaction_survives_the_block_fee_credit`.
- Addresses: canonical form is `0x` + lowercase hex (`src/node/transactions/address.rs`); readers fall back to legacy no-prefix keys (`legacy_account_address_hex`) — preserve that dual-read when touching account state.
- `Blockchain` is shared as `Arc<Mutex<...>>` (tokio Mutex) across the WS, p2p, authoring, and sync tasks; other tasks talk to the libp2p swarm only through `P2PServerCommand` over an mpsc channel.
- Gossip payloads are `[1-byte GossipMessageType (0x01 tx, 0x02 block)] + RLP bytes` (`p2p_server/commands.rs`).
- Transaction hash = **Keccak-256** over RLP `[from (no 0x), nonce, chain_id, data]`, meant to be byte-for-byte identical to clutch-hub-sdk-js `signTransaction` and the clutch-hub-api faucet. The wire format is the 8-item list `[from, nonce, chain_id, signature_r, signature_s, signature_v, hash, data]` — `chain_id` at index 2. No test currently pins this against externally-produced (real SDK) bytes: the old cross-language fixture in `transaction.rs` predates `chain_id` and was removed rather than left to assert something untrue; re-pinning is pending the SDK gaining `chain_id` support (see `TODO(sdk-v3)` in `transaction.rs`). `validate_transaction` recomputes and rejects a mismatched `hash` (the hash doubles as a state key, so a forged one could shadow ride state). Block hash covers `(index, previous_hash, tx hashes)` via SHA-256 — timestamp/author are *not* hashed but the Aura author check uses `block.timestamp`.
- RLP decode of `from` accepts both string (Rust) and raw-bytes (JS SDK) encodings — keep compatibility when touching `rlp_encoding.rs`.
- Stray `clutch-node-*.db` dirs and `output/*.json` at repo root are test/dev leftovers — safe to delete, don't commit new ones.
