# x402-rs AWS Infrastructure Architecture

> Auto-generated from Terraform analysis. Region: **us-east-2 (Ohio)**

---

## 1. High-Level Architecture Overview

```mermaid
graph TB
    subgraph Internet["Internet"]
        Client["Client / Browser"]
        Upstream["Blockchain RPCs<br/>(QuickNode, Alchemy, Public)"]
    end

    subgraph Route53["Route 53 DNS"]
        DNS_Main["facilitator.ultravioletadao.xyz"]
        DNS_Metrics["metrics.facilitator.ultravioletadao.xyz"]
    end

    subgraph ACM["ACM Certificates"]
        Cert_Main["TLS Cert (main)"]
        Cert_Metrics["TLS Cert (metrics)"]
    end

    subgraph VPC["VPC 10.1.0.0/16"]
        subgraph Public["Public Subnets"]
            ALB["Application Load Balancer<br/>:443 HTTPS / :80 redirect"]
            NAT["NAT Gateway + EIP"]
        end

        subgraph Private["Private Subnets"]
            subgraph ECS_Cluster["ECS Cluster: facilitator-production"]
                Facilitator["Facilitator Service<br/>1 vCPU / 2GB RAM<br/>Fargate"]
                Observability["Observability Service<br/>1 vCPU / 2GB RAM<br/>Fargate Spot"]
            end
            EFS["EFS<br/>(Prometheus, Tempo, Grafana data)"]
        end
    end

    subgraph AWS_Services["AWS Managed Services"]
        ECR["ECR<br/>(5 repos)"]
        SM["Secrets Manager<br/>(17 secrets)"]
        CW["CloudWatch<br/>(Logs, Metrics, Alarms)"]
        DDB["DynamoDB<br/>(facilitator-nonces)"]
        S3_Disc["S3<br/>(facilitator-discovery-prod)"]
        Lambda["Lambda<br/>(Balances API)"]
        APIGW["API Gateway<br/>(HTTP API)"]
    end

    subgraph TF_State["Terraform State"]
        S3_TF["S3<br/>(facilitator-terraform-state)"]
        DDB_TF["DynamoDB<br/>(facilitator-terraform-locks)"]
    end

    Client -->|HTTPS| DNS_Main
    Client -->|HTTPS| DNS_Metrics
    DNS_Main -->|A record| ALB
    DNS_Metrics -->|A record| ALB
    ALB -->|:8080| Facilitator
    ALB -->|:3000| Observability
    ALB -->|/api/balances| Lambda
    Cert_Main -.->|TLS termination| ALB
    Cert_Metrics -.->|TLS termination| ALB

    Facilitator -->|NAT| NAT
    NAT -->|RPC calls| Upstream
    Facilitator -->|nonces| DDB
    Facilitator -->|discovery| S3_Disc
    Facilitator -->|logs| CW
    Facilitator -->|pull image| ECR
    Facilitator -->|wallet keys, RPCs| SM

    Observability -->|persistent data| EFS
    Observability -->|logs| CW

    Lambda -->|logs| CW
    APIGW -->|invoke| Lambda

    style Facilitator fill:#2563eb,color:#fff,stroke:#1d4ed8
    style Observability fill:#7c3aed,color:#fff,stroke:#6d28d9
    style ALB fill:#059669,color:#fff,stroke:#047857
    style Lambda fill:#d97706,color:#fff,stroke:#b45309
    style SM fill:#dc2626,color:#fff,stroke:#b91c1c
    style NAT fill:#0891b2,color:#fff,stroke:#0e7490
```

---

## 2. Network Topology

```mermaid
graph TB
    subgraph VPC["VPC: facilitator-production (10.1.0.0/16)"]

        IGW["Internet Gateway"]

        subgraph AZ_A["us-east-2a"]
            PubA["Public Subnet<br/>10.1.0.0/24"]
            PrivA["Private Subnet<br/>10.1.100.0/24"]
        end

        subgraph AZ_B["us-east-2b"]
            PubB["Public Subnet<br/>10.1.1.0/24"]
            PrivB["Private Subnet<br/>10.1.101.0/24"]
        end

        NAT["NAT Gateway<br/>+ Elastic IP<br/>(in PubA)"]

        subgraph SGs["Security Groups"]
            SG_ALB["ALB SG<br/>IN: 443, 80 from 0.0.0.0/0<br/>OUT: all"]
            SG_ECS["ECS Tasks SG<br/>IN: 8080 from ALB SG<br/>OUT: all"]
            SG_OBS["Observability SG<br/>IN: 3000, 9090, 4317<br/>from ALB/ECS SG"]
            SG_EFS["EFS SG<br/>IN: 2049 NFS<br/>from Observability SG"]
        end
    end

    Internet["Internet"] --> IGW
    IGW --> PubA
    IGW --> PubB

    PubA --> NAT
    NAT --> PrivA
    NAT --> PrivB

    SG_ALB -->|port 8080| SG_ECS
    SG_ALB -->|port 3000| SG_OBS
    SG_ECS -->|port 9090, 4317| SG_OBS
    SG_OBS -->|NFS 2049| SG_EFS

    style PubA fill:#bbf7d0,stroke:#16a34a
    style PubB fill:#bbf7d0,stroke:#16a34a
    style PrivA fill:#bfdbfe,stroke:#2563eb
    style PrivB fill:#bfdbfe,stroke:#2563eb
    style NAT fill:#fde68a,stroke:#d97706
    style IGW fill:#e0e7ff,stroke:#4f46e5
```

---

## 3. Request Flow (ALB Routing)

```mermaid
graph LR
    Client["Client"]

    subgraph ALB["ALB: facilitator-production"]
        L80["Listener :80<br/>HTTP"]
        L443["Listener :443<br/>HTTPS + TLS 1.3"]

        Rule_Bal["Rule P10<br/>path: /api/balances"]
        Rule_Met["Rule P5<br/>host: metrics.*"]
        Rule_Def["Default Rule<br/>all other traffic"]
    end

    TG_Fac["Target Group<br/>Facilitator<br/>:8080 (IP)"]
    TG_Lam["Target Group<br/>Lambda Balances"]
    TG_Graf["Target Group<br/>Grafana<br/>:3000 (IP)"]

    Fac["Facilitator Container"]
    Lam["Lambda Function"]
    Graf["Grafana Container"]

    Client -->|HTTP| L80
    L80 -->|301 redirect| L443
    Client -->|HTTPS| L443

    L443 --> Rule_Bal
    L443 --> Rule_Met
    L443 --> Rule_Def

    Rule_Bal --> TG_Lam --> Lam
    Rule_Met --> TG_Graf --> Graf
    Rule_Def --> TG_Fac --> Fac

    style L443 fill:#059669,color:#fff
    style Rule_Bal fill:#d97706,color:#fff
    style Rule_Met fill:#7c3aed,color:#fff
    style Rule_Def fill:#2563eb,color:#fff
    style Fac fill:#2563eb,color:#fff
    style Lam fill:#d97706,color:#fff
    style Graf fill:#7c3aed,color:#fff
```

---

## 4. ECS Services Detail

```mermaid
graph TB
    subgraph Cluster["ECS Cluster: facilitator-production<br/>Capacity: FARGATE + FARGATE_SPOT"]

        subgraph Svc1["Service: facilitator-production<br/>Launch: FARGATE | Desired: 1 | Max: 3"]
            subgraph TD1["Task Definition: facilitator-production<br/>1 vCPU / 2 GB"]
                C1["Container: facilitator<br/>Image: facilitator:v1.31.0<br/>Port: 8080<br/>Essential: true"]
                C2["Container: otel-collector<br/>Ports: 4317, 4318<br/>Essential: false<br/>(conditional)"]
            end
        end

        subgraph Svc2["Service: observability-production<br/>Launch: FARGATE_SPOT | Desired: 1<br/>(conditional: enable_observability)"]
            subgraph TD2["Task Definition: observability-production<br/>1 vCPU / 2 GB"]
                C3["Container: grafana<br/>Port: 3000<br/>Essential: true"]
                C4["Container: prometheus<br/>Port: 9090<br/>Essential: true<br/>Retention: 15d"]
                C5["Container: tempo<br/>Ports: 3200, 4317<br/>Essential: true"]
            end
        end
    end

    subgraph AutoScale["Auto Scaling"]
        AS_CPU["CPU Policy<br/>Target: 75%"]
        AS_MEM["Memory Policy<br/>Target: 80%"]
    end

    subgraph Storage["Persistent Storage"]
        EFS_P["EFS: /prometheus"]
        EFS_T["EFS: /tempo"]
        EFS_G["EFS: /grafana"]
    end

    subgraph Discovery["Service Discovery"]
        CM["Cloud Map<br/>observability.facilitator.local"]
    end

    C2 -->|OTLP gRPC :4317| C5
    C2 -->|Remote Write :9090| C4

    C4 --> EFS_P
    C5 --> EFS_T
    C3 --> EFS_G

    Svc2 -.->|register| CM
    C2 -.->|DNS lookup| CM

    AS_CPU --> Svc1
    AS_MEM --> Svc1

    style C1 fill:#2563eb,color:#fff
    style C2 fill:#0891b2,color:#fff
    style C3 fill:#ea580c,color:#fff
    style C4 fill:#dc2626,color:#fff
    style C5 fill:#7c3aed,color:#fff
    style Svc2 fill:#f3e8ff,stroke:#7c3aed,stroke-dasharray: 5 5
```

---

## 5. Secrets Manager Layout

```mermaid
graph LR
    subgraph Wallets["Wallet Keys (14 secrets)"]
        subgraph EVM["EVM"]
            EVM_M["evm-mainnet-private-key"]
            EVM_T["evm-testnet-private-key"]
            EVM_L["evm-private-key<br/>(legacy)"]
        end
        subgraph SOL["Solana"]
            SOL_M["solana-mainnet-keypair"]
            SOL_T["solana-testnet-keypair"]
            SOL_L["solana-keypair<br/>(legacy)"]
        end
        subgraph NEAR["NEAR"]
            NEAR_M["near-mainnet-keypair<br/>{private_key, account_id}"]
            NEAR_T["near-testnet-keypair<br/>{private_key, account_id}"]
        end
        subgraph Stellar["Stellar"]
            STEL_M["stellar-keypair-mainnet"]
            STEL_T["stellar-keypair-testnet"]
        end
        subgraph Sui["Sui"]
            SUI_M["sui-keypair-mainnet"]
            SUI_T["sui-keypair-testnet"]
        end
        subgraph Algo["Algorand"]
            ALGO_M["algorand-mnemonic-mainnet"]
            ALGO_T["algorand-mnemonic-testnet"]
        end
    end

    subgraph RPCs["RPC URLs (2 secrets)"]
        RPC_M["rpc-mainnet<br/>{base, avalanche, polygon,<br/>optimism, celo, hyperevm,<br/>ethereum, arbitrum, unichain,<br/>solana, near}"]
        RPC_T["rpc-testnet<br/>{solana-devnet, arbitrum-sepolia, near}"]
    end

    subgraph Obs["Observability (1 secret)"]
        GRAF["grafana-admin-password"]
    end

    subgraph Consumers["Consumers"]
        Fac["Facilitator Task<br/>(Execution Role)"]
        Lam["Lambda Function<br/>(Execution Role)"]
        ObsSvc["Observability Task<br/>(Execution Role)"]
    end

    Fac -->|reads| Wallets
    Fac -->|reads| RPCs
    Lam -->|reads| RPCs
    ObsSvc -->|reads| Obs

    style EVM_M fill:#dc2626,color:#fff
    style EVM_T fill:#f87171,color:#fff
    style RPC_M fill:#2563eb,color:#fff
    style RPC_T fill:#60a5fa,color:#fff
    style GRAF fill:#7c3aed,color:#fff
```

---

## 6. IAM Roles and Permissions

```mermaid
graph TB
    subgraph Roles["IAM Roles"]
        R1["facilitator-production<br/>-ecs-execution"]
        R2["facilitator-production<br/>-ecs-task"]
        R3["facilitator-production<br/>-balances-lambda"]
        R4["observability-production<br/>-ecs-execution"]
        R5["observability-production<br/>-ecs-task"]
    end

    subgraph Managed["AWS Managed Policies"]
        MP1["AmazonECSTask<br/>ExecutionRolePolicy"]
        MP2["AWSLambdaBasic<br/>ExecutionRole"]
    end

    subgraph Permissions["Custom Permissions"]
        P_SM["secretsmanager:<br/>GetSecretValue<br/>(16 secrets)"]
        P_DDB["dynamodb:<br/>PutItem, GetItem<br/>(facilitator-nonces)"]
        P_S3["s3: Get/Put/Delete<br/>(discovery-prod)"]
        P_SM_RPC["secretsmanager:<br/>GetSecretValue<br/>(rpc-* only)"]
        P_SM_GRAF["secretsmanager:<br/>GetSecretValue<br/>(grafana password)"]
        P_EFS["elasticfilesystem:<br/>ClientMount/Write<br/>/RootAccess"]
    end

    R1 --> MP1
    R1 --> P_SM

    R2 --> P_DDB
    R2 --> P_S3

    R3 --> MP2
    R3 --> P_SM_RPC

    R4 --> MP1
    R4 --> P_SM_GRAF

    R5 --> P_EFS

    style R1 fill:#2563eb,color:#fff
    style R2 fill:#2563eb,color:#fff
    style R3 fill:#d97706,color:#fff
    style R4 fill:#7c3aed,color:#fff
    style R5 fill:#7c3aed,color:#fff
```

---

## 7. Lambda Balances API

```mermaid
graph LR
    Client["Client"]

    subgraph ALB_Route["ALB"]
        Rule["/api/balances<br/>Priority 10"]
    end

    subgraph APIGW["API Gateway (HTTP)"]
        Route["GET /balances"]
        CORS["CORS: *<br/>GET, OPTIONS"]
    end

    subgraph Lambda_Fn["Lambda: facilitator-production-balances"]
        Runtime["Python 3.12<br/>256 MB / 30s timeout"]
        ENV["RPC URLs<br/>(12 networks)"]
    end

    subgraph Chains["Blockchain RPCs"]
        Base["Base"]
        Avalanche["Avalanche"]
        Polygon["Polygon"]
        Optimism["Optimism"]
        Ethereum["Ethereum"]
        Arbitrum["Arbitrum"]
        Celo["Celo"]
        HyperEVM["HyperEVM"]
        Unichain["Unichain"]
        Monad["Monad"]
        BSC["BSC"]
        Sui["Sui"]
    end

    CW["CloudWatch Logs<br/>7-day retention"]

    Client -->|HTTPS| ALB_Route
    Client -->|HTTPS| APIGW
    ALB_Route --> Lambda_Fn
    APIGW --> Lambda_Fn
    Lambda_Fn --> Chains
    Lambda_Fn -->|logs| CW

    style Lambda_Fn fill:#d97706,color:#fff
    style APIGW fill:#0891b2,color:#fff
```

---

## 8. CloudWatch Monitoring

```mermaid
graph TB
    subgraph LogGroups["Log Groups (7-day retention)"]
        LG1["/ecs/facilitator-production"]
        LG2["/ecs/observability-production"]
        LG3["/aws/lambda/...-balances"]
        LG4["/aws/apigateway/...-balances"]
    end

    subgraph MetricFilters["Metric Filters (13 total)"]
        subgraph NEAR_Filters["NEAR Protocol (5)"]
            NF1["Settlement Success"]
            NF2["Settlement Failure"]
            NF3["RPC Error"]
            NF4["Verification Success"]
            NF5["Verification Failure"]
        end
        subgraph V2_Filters["x402 v2 Protocol (8)"]
            VF1["V1 Requests"]
            VF2["V2 Requests"]
            VF3["CAIP-2 Parse Errors"]
            VF4["Unsupported Version"]
            VF5["V2 Settle Success"]
            VF6["V2 Settle Failure"]
            VF7["V2 Verify Success"]
            VF8["V2 Verify Failure"]
        end
    end

    subgraph Alarms["CloudWatch Alarms (5)"]
        A1["NEAR Settlement<br/>Failure Rate High<br/>> 5 in 5min"]
        A2["NEAR RPC<br/>Errors High<br/>> 10 in 5min"]
        A3["CAIP-2 Parse<br/>Errors High<br/>> 5 in 5min"]
        A4["V2 Settlement<br/>Failure Rate High<br/>> 5 in 5min"]
        A5["V1 Traffic<br/>Sudden Drop<br/>< 5/hr (disabled)"]
    end

    subgraph Dashboards["CloudWatch Dashboards (2)"]
        D1["facilitator-near-operations"]
        D2["facilitator-x402-v2-migration"]
    end

    LG1 --> NEAR_Filters
    LG1 --> V2_Filters
    NEAR_Filters --> A1
    NEAR_Filters --> A2
    V2_Filters --> A3
    V2_Filters --> A4
    V2_Filters --> A5
    NEAR_Filters --> D1
    V2_Filters --> D2

    CI["Container Insights<br/>(ECS cluster-level)"]

    style A1 fill:#dc2626,color:#fff
    style A2 fill:#dc2626,color:#fff
    style A3 fill:#f97316,color:#fff
    style A4 fill:#dc2626,color:#fff
    style A5 fill:#9ca3af,color:#fff
    style D1 fill:#2563eb,color:#fff
    style D2 fill:#7c3aed,color:#fff
```

---

## 9. Observability Data Flow

```mermaid
graph LR
    subgraph FacTask["Facilitator Task"]
        App["Facilitator<br/>:8080"]
        OTel["OTel Collector<br/>Sidecar<br/>:4317 :4318"]
    end

    subgraph ObsTask["Observability Task (Fargate Spot)"]
        Prom["Prometheus<br/>:9090<br/>15d retention"]
        Tempo["Tempo<br/>:4317<br/>Traces"]
        Grafana["Grafana<br/>:3000<br/>Dashboards"]
    end

    subgraph EFS_Vol["EFS Volumes"]
        EFS_P["/prometheus"]
        EFS_T["/tempo"]
        EFS_G["/grafana"]
    end

    CloudMap["Cloud Map DNS<br/>observability.facilitator.local"]

    App -->|OTLP HTTP :4318| OTel
    OTel -->|Remote Write| Prom
    OTel -->|OTLP gRPC :4317| Tempo
    OTel -.->|DNS resolve| CloudMap
    CloudMap -.->|IP address| ObsTask

    Grafana -->|query| Prom
    Grafana -->|query| Tempo

    Prom --> EFS_P
    Tempo --> EFS_T
    Grafana --> EFS_G

    ALB["ALB<br/>metrics.*"] -->|:3000| Grafana

    style App fill:#2563eb,color:#fff
    style OTel fill:#0891b2,color:#fff
    style Prom fill:#dc2626,color:#fff
    style Tempo fill:#7c3aed,color:#fff
    style Grafana fill:#ea580c,color:#fff
```

---

## 10. Cost Breakdown

```mermaid
pie title Monthly Cost Estimate (~$60-75/month)
    "ECS Fargate (Facilitator)" : 29
    "ALB" : 16
    "Secrets Manager" : 7
    "CloudWatch Logs" : 5
    "Observability Stack" : 10
    "DynamoDB + S3" : 2
    "ECR + Route53" : 2
    "Lambda + API GW" : 1
    "NAT Gateway EIP" : 3
```

---

## 11. ECR Repositories

| Repository | Purpose | Always Kept |
|------------|---------|-------------|
| `facilitator` | Main facilitator image | Yes |
| `facilitator-otel-collector` | OTel Collector sidecar | Yes |
| `facilitator-prometheus` | Prometheus for metrics | Yes |
| `facilitator-tempo` | Tempo for traces | Yes |
| `facilitator-grafana` | Grafana for dashboards | Yes |

> All 5 ECR repos are always kept even when observability is disabled, to enable instant re-activation.

---

## 12. Conditional Resources (enable_observability toggle)

Resources that only exist when `enable_observability = true`:

| Resource | Type |
|----------|------|
| Observability ECS Service | `aws_ecs_service` |
| Observability Task Definition | `aws_ecs_task_definition` |
| EFS File System + Mount Targets | `aws_efs_file_system` |
| EFS Access Points (3x) | `aws_efs_access_point` |
| Observability Security Group | `aws_security_group` |
| EFS Security Group | `aws_security_group` |
| Grafana Target Group | `aws_lb_target_group` |
| Metrics ALB Listener Rule | `aws_lb_listener_rule` |
| Metrics ACM Certificate | `aws_acm_certificate` |
| Metrics Route53 Record | `aws_route53_record` |
| Cloud Map Namespace + Service | `aws_service_discovery_*` |
| OTel Collector Sidecar (in Facilitator task) | Container definition |
| Observability CloudWatch Log Group | `aws_cloudwatch_log_group` |

**Cost when OFF:** ~$0/month (ECR repos + ACM cert are free to keep)
**Cost when ON:** ~$10-15/month additional (Fargate Spot + EFS)

---

## Quick Reference

| Component | Value |
|-----------|-------|
| **Region** | us-east-2 (Ohio) |
| **VPC CIDR** | 10.1.0.0/16 |
| **Domain** | facilitator.ultravioletadao.xyz |
| **Metrics** | metrics.facilitator.ultravioletadao.xyz |
| **ECS Cluster** | facilitator-production |
| **Facilitator CPU/RAM** | 1 vCPU / 2 GB |
| **Auto-scale** | 1-3 tasks (CPU 75%, Mem 80%) |
| **ALB Idle Timeout** | 180s (for slow settlements) |
| **Log Retention** | 7 days |
| **Nonce Store** | DynamoDB facilitator-nonces (TTL) |
| **Discovery Store** | S3 facilitator-discovery-prod |
| **Terraform State** | S3 facilitator-terraform-state |
| **Lambda Runtime** | Python 3.12, 256 MB, 30s |
| **TLS Policy** | ELBSecurityPolicy-TLS13-1-2-2021-06 |
