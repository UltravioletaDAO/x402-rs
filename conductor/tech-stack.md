# Technology Stack

## Overview

The x402-rs project is built primarily with Rust, leveraging its performance, reliability, and robust ecosystem for developing secure and efficient blockchain-agnostic payment facilitation services. The architecture is designed for high throughput and scalability, supporting multiple blockchain networks and integrating with modern deployment practices.

## Core Technologies

### Programming Language
*   **Rust:** The primary programming language for the entire project, chosen for its memory safety, performance, and concurrency features, which are critical for blockchain-related applications.

### Web Framework
*   **Axum (0.8.4):** A web application framework built with Tokio and Hyper, used for handling HTTP requests and defining API endpoints for the facilitator service. It provides a robust and flexible foundation for building the web interface.

### Asynchronous Runtime
*   **Tokio (1.45.0):** The leading asynchronous runtime for Rust, enabling highly concurrent and non-blocking operations essential for managing numerous network requests and blockchain interactions efficiently.

### Serialization & Deserialization
*   **Serde, Serde JSON:** Essential libraries for efficient and flexible serialization and deserialization of data structures, crucial for handling API payloads and inter-service communication.

### HTTP Client
*   **Reqwest (0.12):** An ergonomic, batteries-included HTTP client for Rust, used for making outbound HTTP requests, particularly for interacting with external services or blockchain RPC endpoints not covered by specific SDKs (e.g., Stellar/Soroban interactions).

### Blockchain Interaction Libraries

The project integrates with various blockchain networks through specialized libraries:
*   **Alloy (1.0.12):** A comprehensive and modular library for interacting with Ethereum Virtual Machine (EVM) compatible blockchains, providing functionalities for transaction signing, contract interaction, and data encoding.
*   **Solana SDK (2.3.1):** The official Software Development Kit for Solana, enabling direct interaction with the Solana blockchain, including transaction construction, program interaction, and token operations (SPL tokens).
*   **NEAR Protocol Libraries (near-jsonrpc-client, near-primitives, near-crypto, etc.):** A suite of crates for building applications on the NEAR Protocol, facilitating JSON-RPC communication, handling cryptographic operations, and managing NEAR-specific data structures for NEP-366 meta-transactions.
*   **Stellar/Soroban Libraries (stellar-strkey, stellar-xdr):** Libraries for interacting with the Stellar network and its smart contract platform, Soroban, handling address encoding/decoding and eXternal Data Representation (XDR) for protocol data structures.

### Observability
*   **Tracing, Tracing-Subscriber, OpenTelemetry:** A powerful stack for structured logging, metrics, and distributed tracing, enabling comprehensive monitoring and debugging of the application in production environments.

### Configuration
*   **Dotenvy:** Used for loading environment variables from `.env` files, simplifying configuration management for different deployment environments.

### Local Utility Crates
The project is structured as a Cargo workspace, including several local crates that enhance modularity and reusability:
*   `x402-axum`: Likely contains the Axum web server implementation specific to x402.
*   `x402-compliance`: Handles compliance-related logic, potentially blockchain-specific checks.
*   `x402-reqwest`: May abstract external HTTP calls or specific `reqwest` usages.

## Deployment & Infrastructure

*   **Docker:** Used for containerizing the application, ensuring consistent environments across development, testing, and production.
*   **AWS Elastic Container Service (ECS):** The primary platform for deploying and orchestrating the containerized facilitator service in a production environment.
*   **Terraform:** Infrastructure as Code (IaC) tool used to define, provision, and manage the AWS cloud infrastructure (e.g., ECS clusters, load balancers, networking) for the project.

This comprehensive tech stack ensures that x402-rs is robust, scalable, and adaptable to the evolving multi-chain landscape.
