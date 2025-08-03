# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

- **Build CLI (release)**: `cargo build -p leaf-cli --release` or `make cli`
- **Build CLI (dev)**: `cargo build -p leaf-cli` or `make cli-dev`
- **Run tests**: `cargo test -p leaf -- --nocapture` or `make test`
- **Regenerate protobuf files**: `./scripts/regenerate_proto_files.sh` or `make proto-gen`
- **Run CLI**: `./target/debug/leaf --help` (after building)

## Architecture Overview

Leaf is a versatile proxy framework written in Rust with a modular architecture:

### Core Components

- **`leaf/`**: Main library crate containing the core proxy engine
- **`leaf-cli/`**: Command-line interface binary
- **`leaf-ffi/`**: C FFI bindings for mobile integration
- **`leaf-uniffi/`**: UniFFI bindings for cross-platform mobile apps
- **`leaf-plugins/shadowsocks/`**: Shadowsocks protocol plugin

### Key Modules

- **`app/`**: Application layer including dispatcher, managers, and runtime
  - `dispatcher.rs`: Routes traffic between inbounds and outbounds
  - `inbound/manager.rs`: Manages inbound connections (TUN, SOCKS, HTTP, etc.)
  - `outbound/manager.rs`: Manages outbound connections and protocols
  - `nat_manager.rs`: Handles NAT translation for TUN mode
  - `router.rs`: Request routing based on rules (domain, IP, GEOIP)
  - `dns_client.rs`: DNS resolution with fake DNS support

- **`proxy/`**: Protocol implementations
  - Each protocol has inbound/outbound implementations
  - Supports: Direct, Shadowsocks, Trojan, VMess, SOCKS, HTTP, TLS, WebSocket
  - Special outbounds: Failover, TryAll, Chain, Static for HA/load balancing

- **`config/`**: Configuration parsing (JSON and INI formats)
- **`common/`**: Shared utilities (crypto, networking, I/O)

### Key Features

- **Multiplexing**: AMux (stream-based) and QUIC (UDP-based) transports
- **Transparent Proxying**: TUN interface for VPN-like functionality
- **High Availability**: Failover, retry, and load balancing outbounds
- **Rule-based Routing**: Route requests by domain, IP, GEOIP, port
- **Mobile Support**: iOS/Android apps via FFI bindings

### Configuration

- Supports both INI and JSON config formats
- JSON examples in `leaf/tests/` directory
- TUN mode syntax: `tun = auto` (macOS/Linux)
- Gateway mode: `GATEWAY_MODE=true leaf -c config.conf`

### Testing

- Integration tests in `leaf/tests/` covering various proxy chains
- Each test file represents a specific proxy configuration scenario
- Tests use both client and server-side configurations

### Development Notes

- Uses feature flags extensively for optional protocol support
- Async/await with Tokio runtime
- Runtime manager supports auto-reload with file watching
- Cross-compilation support via Cross.toml
- Mobile build scripts in `scripts/` directory