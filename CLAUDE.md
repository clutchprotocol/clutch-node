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
| `src/node/transactions/` | One file per tx type + `transaction.rs` (envelope, SHA3-256 hash, secp256k1 sig), `function_call.rs` (enum), `transaction_pool.rs` (mempool in RocksDB) |
| `src/node/account_state.rs` | Balance/nonce state; `apply_balance_change` returns `StateUpdate` (storage write + optional `BalanceEffect`) |
| `src/node/balance_effect.rs` | Balance-effect audit records persisted per tx / per block / per account (explorer & RPC consume these) |
| `src/node/p2p_server/` | libp2p: `server.rs` (swarm, gossipsub + mdns + request-response), `gossipsub_handler.rs` (incoming tx/block), `handshake.rs` + `get_block_header/bodies` (sync protocol), `commands.rs` (mpsc command enum other tasks use to talk to the swarm) |
| `src/node/wss/websocket.rs` | WebSocket JSON-RPC 2.0 server — all RPC methods live here |
| `src/node/rlp_encoding.rs` | Hand-written `Encodable`/`Decodable` for Transaction, Block, FunctionCall, sync messages + generic `encode`/`decode` |
| `src/node/database.rs` | RocksDB wrapper; column families: `block`, `state`, `blockchain`, `tx_pool` |
| `src/node/configuration.rs` | `AppConfig` loaded from `config/node/{env}.toml` + `APP_*` env overrides |
| `src/node/metric.rs` | Prometheus gauges (`latest_block_index`, `latest_block`) served via axum on `serve_metric_addr` |
| `src/node/signature_keys.rs`, `coordinate.rs`, `time_utils.rs`, `seq.rs`, `tracing.rs`, `file_utils.rs` | secp256k1 sign/verify+recovery, lat/lng, unix time, Seq log sink, tracing setup, JSON dumps to `output/` |

## Transaction Flow

1. Signed tx arrives via WS RPC (`send_transaction` JSON or `send_raw_transaction` hex RLP) or via gossipsub (`gossipsub_handler.rs`).
2. `Blockchain::add_transaction_to_pool` → `Transaction::validate_transaction`: signature (recover & compare to `from`), nonce (`== last + 1`), then per-type `verify_state` (e.g. RideRequest checks balance ≥ fare and no concurrent open request via `passenger_concurrent.rs`). Valid txs land in the `tx_pool` CF and are re-gossiped.
3. Authoring loop (`node_services.rs::start_authoring_job`, every 1s) calls `author_new_block`: drains pool, builds+signs block, then `import_block`. Aura rejects it unless this node is the current slot's author, so most ticks are no-ops (`Err` logged at debug).
4. `import_block` = `verify_block_author` (Aura slot check) + `validate_block` (sig, index, prev_hash) + re-validate all txs + `Block::add_block_to_chain`, which batches into one `db.write()`: block, latest-block pointer, per-tx state updates (`state_transaction`), balance effects, block reward mint, tx_pool deletions. Accepted blocks are gossiped; peers import the same way.
5. Sync: on startup, a node sends an RLP `Handshake` to the first connected peer, then pulls `GetBlockHeaders`/`GetBlockBodies` over libp2p request-response.

## Transaction Types

`FunctionCall` enum in `src/node/transactions/function_call.rs`: Transfer, RideRequest, RideOffer, RideAcceptance, RidePay, RideCancel, RideRequestCancel. Each variant's struct file defines `verify_state` (validation) and `state_transaction` (state writes + balance effects). To add a type: new file + enum variant, wire `verify_state`/`state_transaction`/`function_call_type` matches in `transaction.rs`, and add RLP tag arms in `rlp_encoding.rs`. **RLP tags are not contiguous** — RideRequestCancel is tag `8` (6–7 skipped); tags must match the JS SDK's encoder exactly.

## RPC (WebSocket JSON-RPC 2.0)

All methods are matched by string in `WebSocket::handle_json_rpc_request` in `src/node/wss/websocket.rs`. Current methods: `send_transaction`, `send_raw_transaction`, `import_block`, `author_new_block`, `get_next_nonce`, `get_account_balance`, `get_account_balance_effects`, `get_block_by_index`, `list_ride_requests`, `list_ride_offers`, `list_active_trips`, `list_completed_trips`, `list_recent_trips`. To add one: write a `handle_*` fn (parse params with an inline serde struct, lock `blockchain`, return `json_rpc_success_response`/`json_rpc_error_response`), add a match arm, expose any new query on `Blockchain`, then update clutch-hub-api → SDK → docs per workspace convention.

## Config

- Files: `config/node/{default,node1,node2,node3}.toml`, selected by `--env <name>` (default `default`). Env overrides use `APP_` prefix (e.g. `APP_LOG_LEVEL`); `.env` is loaded via dotenv. Config path is **relative to cwd** — run from the repo root.
- `default` ≈ node1 (authority 1, ws 8081, p2p 4001, metrics 3001, no bootstrap, local Seq). node2/node3 differ in: `blockchain_name` (separate DB dir), author keypair (authorities 2/3), ports (8082/4002/3002, 8083/4003/3003), and `bootstrap_nodes` — `/ip4/127.0.0.1/tcp/4001` (node1 on the same host; mdns also discovers local peers).
- This repo's `docker-compose.yml` uses `node2-docker.toml`/`node3-docker.toml` (`--env node2-docker`), which bootstrap via `/dns4/node1/tcp/4001` — env override is not an option because `bootstrap_nodes` is a `Vec<String>` and the config loader does no list parsing. clutch-deploy mounts its own config copies (`clutch-deploy/config/node/*.toml`, also `/dns4/node1/...`) and is unaffected by this repo's TOMLs.
- All three well-known authority keypairs (and the genesis-funded account `0xdeb4...6cc0` holding `i64::MAX`) are committed in configs/tests — dev-only keys.
- `developer_mode = true` deletes the RocksDB and dumps chain+pool JSON to `output/` on shutdown.
- DB path: `{DB_PATH or cwd}/{blockchain_name}.db`.

## Commands

```powershell
cargo run                          # single node, config/node/default.toml
cargo run -- --env node2           # pick another config
cargo build --release
cargo test                         # unit + integration tests
docker compose up -d               # 3-node local net from ghcr image (this repo's docker-compose.yml)
.\scripts\docker-build.ps1         # local image build
```

- Tests in `tests/` (`ride_sharing.rs`, `block_reward.rs`, `balance_effects.rs`, `transfer.rs`, `referrer_account.rs`, `rlp_decode_test.rs`, `p2p_server_tests.rs`) hit **real RocksDB instances in the cwd**; DB-touching tests are `#[serial]` (serial_test crate) — keep that attribute on any new test that opens a database, and clean up via `blockchain.shutdown_blockchain()` (developer_mode).
- CI: `.github/workflows/docker-build-push.yml` builds multi-arch images to GHCR + Docker Hub on push to main / `v*` tags, then repository-dispatches `deploy-stage` to clutch-deploy. There is **no CI job running `cargo test`** — run tests locally before pushing.

## Gotchas / Conventions

- Error handling is `Result<_, String>` everywhere (no anyhow/thiserror); DB read failures on hot paths (`get_latest_block`, `add_block_to_chain`) `panic!`.
- Logging via `tracing` macros; logs also ship to Seq (`seq_url`/`seq_api_key` in config).
- State keys are string-prefixed in the `state` CF: `account_state_{addr}`, `account_nonce_{addr}`, `ride_request_{hash}`, `ride_request_{hash}:ride_acceptance`, `ride_acceptance_{hash}:fare_paid`, `tx_effects_{hash}`, `block_effects_{height}`, `account_effect_{addr}_{reverse_height}...` — see `docs/state_keys.csv` and `balance_effect.rs`.
- Addresses: canonical form is `0x` + lowercase hex (`src/node/transactions/address.rs`); readers fall back to legacy no-prefix keys (`legacy_account_address_hex`) — preserve that dual-read when touching account state.
- `Blockchain` is shared as `Arc<Mutex<...>>` (tokio Mutex) across the WS, p2p, authoring, and sync tasks; other tasks talk to the libp2p swarm only through `P2PServerCommand` over an mpsc channel.
- Gossip payloads are `[1-byte GossipMessageType (0x01 tx, 0x02 block)] + RLP bytes` (`p2p_server/commands.rs`).
- Transaction hash covers only `(from, nonce, data)` RLP; block hash covers `(index, previous_hash, tx hashes)` — timestamp/author are *not* hashed but the Aura author check uses `block.timestamp`.
- RLP decode of `from` accepts both string (Rust) and raw-bytes (JS SDK) encodings — keep compatibility when touching `rlp_encoding.rs`.
- Stray `clutch-node-*.db` dirs and `output/*.json` at repo root are test/dev leftovers — safe to delete, don't commit new ones.
