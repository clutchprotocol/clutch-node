# Clutch-Node

![Alpha](https://img.shields.io/badge/status-alpha-orange.svg)
![Experimental](https://img.shields.io/badge/stage-experimental-red.svg)
![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)
![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)

> ⚠️ **ALPHA SOFTWARE** - This project is in active development and is considered experimental. Use at your own risk. APIs may change without notice.

Clutch-Node is a blockchain-based ridesharing platform that aims to improve urban mobility by leveraging blockchain technology to create a decentralized, efficient, and secure system for ridesharing.

**Created and maintained by [Mehran Mazhar](https://github.com/MehranMazhar)**

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

## Block Reward

`clutch-node` supports a fixed author block reward configured per node environment file:

```toml
block_reward_amount = 50
```

- The reward is minted on every accepted non-genesis block.
- The full reward (`100%`) is credited to the block author account (`block.author`).
- Genesis block does not mint any author reward.

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