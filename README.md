# Clutch-Node

![Alpha](https://img.shields.io/badge/status-alpha-orange.svg)
![Experimental](https://img.shields.io/badge/stage-experimental-red.svg)
![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)
![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)

> ⚠️ **ALPHA SOFTWARE** - This project is in active development and is considered experimental. Use at your own risk. APIs may change without notice.

Clutch Node is the blockchain core for Clutch Protocol — Aura consensus, custom RLP transactions, and WebSocket JSON-RPC.

**Documentation:** https://docs.clutchprotocol.io/clutch-node/overview

**Created and maintained by [Mehran Mazhar](https://github.com/MehranMazhar)**

## Transaction types

| Tag | Type | Description |
|-----|------|-------------|
| 0 | Transfer | CLT transfer |
| 1 | RideRequest | Passenger requests a ride |
| 2 | RideOffer | Driver offer |
| 3 | RideAcceptance | Passenger accepts offer |
| 4 | RidePay | Payment (partial OK) |
| 5 | RideCancel | Cancel active trip |
| 8 | RideRequestCancel | Cancel pending request |

## JSON-RPC (WebSocket)

`send_raw_transaction`, `get_next_nonce`, `get_account_balance`, `list_ride_requests`, `list_ride_offers`, `list_active_trips`, `list_completed_trips`, `list_recent_trips`, `get_block_by_index`

Apps typically use [clutch-hub-api](https://github.com/clutchprotocol/clutch-hub-api) instead of calling the node directly.

## Features
- **Decentralized System**: Eliminates intermediaries, allowing users to connect directly.
- **Secure Transactions**: Utilizes blockchain technology to ensure the security and privacy of all transactions.
- **User Empowerment**: Provides users with more control over their ridesharing experiences.
- **Eco-friendly Options**: Encourages the use of electric and hybrid vehicles to reduce carbon footprint.

## Prerequisites
- Docker
- Docker Compose
- Rust 1.70+ (for local development)

## 🐳 Docker

### Automated Builds
This project automatically builds and publishes Docker images to Docker Hub at `9194010019/clutch-node` when code is pushed to the main branch.

#### 🚀 **Docker Optimizations**
Our Docker images feature several optimizations for production use:

- **📦 Minimal Size**: Debian Slim base (~50MB) with stripped binaries
- **🔒 Security**: Non-root user execution with minimal dependencies
- **⚡ Performance**: Optimized binary with clang compiler
- **🛡️ Health Checks**: Built-in container health monitoring
- **📱 Multi-Arch**: Supports AMD64 and ARM64 architectures
- **💨 Fast Builds**: Optimized layer caching for dependencies

### Using Pre-built Images
Our Docker images are highly optimized using Debian Slim for minimal size and maximum compatibility.

```bash
# Pull the latest image (typically ~100MB)
docker pull 9194010019/clutch-node:latest

# Run a single node
docker run --rm -p 8081:8081 9194010019/clutch-node:latest

# Run with custom config
docker run --rm -p 8081:8081 -v ${PWD}/config:/app/config 9194010019/clutch-node:latest --env node1

# Health check
docker run --rm 9194010019/clutch-node:latest --version
```

### Local Docker Development
```powershell
# Build locally
.\scripts\docker-build.ps1

# Build and push to Docker Hub
.\scripts\docker-build.ps1 -Push

# Build with custom tag
.\scripts\docker-build.ps1 -Tag "dev" -Push
```

### Docker Compose
```bash
# Run full development environment
docker-compose up -d

# View logs
docker-compose logs -f node1

# Stop all services
docker-compose down
```

### Setting Up Docker Hub Auto-Publishing

To enable automatic Docker image publishing, add these secrets to your GitHub repository:

1. Go to your repository → Settings → Secrets and variables → Actions
2. Add the following secrets:
   - `DOCKERHUB_USERNAME`: Your Docker Hub username
   - `DOCKERHUB_TOKEN`: Docker Hub access token (create at hub.docker.com/settings/security)

The GitHub Action will automatically:
- Build Docker images on push to main branch
- Push to `9194010019/clutch-node:latest`
- Create additional tags for branches and commits
- Update the Docker Hub repository description

## Running the Project Locally

To get started with Clutch-Node, follow these steps:

1. Clone the repository:
    ```bash
    git clone https://github.com/MehranMazhar/clutch-node
    cd clutch-node
    ```

2. Start the application:
    ```bash
    cargo run -- --env node1
    ```

## Transaction Fees

Block rewards are gone (`block_reward_amount` no longer exists). The block author is paid
out of transaction fees instead: a flat `tx_fee` per transaction, credited to `block.author`
once per block as a single aggregate. No CLT is created — fees are backed CLT changing hands.

`Mint` is fee-exempt (the mint authority may hold no balance), as is the genesis `ChainInit`.
A transaction whose sender is the block author pays no fee to itself.

## Consensus Parameters

These live in `config/node/{env}.toml` and are committed to state by the genesis `ChainInit`
transaction, so they are part of the genesis hash: **all nodes on a network must carry
identical values or they cannot peer**. None of them is optional — a config missing any key
below fails to deserialize at boot.

```toml
chain_id = 2077                        # signed into every tx hash; replay-isolates networks
is_testnet = true                      # false requires faucet_allocation = 0
tx_fee = 1000                          # flat fee per transaction, paid to the block author
mint_authority = "0x..."               # the only address allowed to sign Mint
faucet_address = "0x..."               # genesis-funded account (testnet only)
faucet_allocation = 1000000000000000   # its balance in base units, <= i64::MAX
ride_request_referrer_fee_bps = 200    # basis points (200 bps = 2%), floor-rounded
ride_offer_referrer_fee_bps = 200      # renamed from the old percent-based fields
```

## CLT Economics

CLT is a redeemable token pegged at **1 USD = 1,000,000 CLT** (six decimals in base units),
so `total_supply` must always match the off-chain reserve:

- **Mint** — the mint authority credits an address against a paid-in reserve deposit. Carries
  a `credit_ref` (`keccak256` of the treasury intent id) that may be claimed exactly once.
- **Burn** — permissionless; destroys the sender's CLT. An optional `redemption_ref` marks it
  as a redemption claim the treasury pays out off-chain, also exactly once.
- **RidePay** — referrer fees (default 200 bps request + 200 bps offer per installment,
  floor-rounded and capped so they can never exceed the fare); the driver takes the remainder.

Full details: [docs.clutchprotocol.io/clutch-node/clt-economics](https://docs.clutchprotocol.io/clutch-node/clt-economics)

## Installing Clang on Windows
Set the `LIBCLANG_PATH` environment variable:
```bash
ECHO %LIBCLANG_PATH%
SET LIBCLANG_PATH=C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\Llvm\x64\bin
```

## Contributing
Contributions are what make the open-source community such an amazing place to learn, inspire, and create. Any contributions you make are **greatly appreciated**.

## License
Distributed under the Apache License 2.0. See `LICENSE` for more information.

## Author & Maintainer

**Mehran Mazhar**
- GitHub: [@MehranMazhar](https://github.com/MehranMazhar)
- Website: [MehranMazhar.com](https://MehranMazhar.com)
- Email: mehran.mazhar@gmail.com

## Contact
If you have any questions or comments, please feel free to contact us at mehran.mazhar@gmail.com.

## Docker

### Building the Project
The project is built using Docker to ensure a consistent environment. The provided Dockerfile handles all dependencies and builds the project in release mode.

```bash
docker build -t clutch-node .
```

### Running Multiple Nodes on Different Networks
To run multiple nodes, you need to specify different networks and ports:

```bash
docker network create clutch-network1
docker network create clutch-network2
docker network create clutch-network3
docker-compose up node1
docker-compose up node2
docker-compose up node3
```