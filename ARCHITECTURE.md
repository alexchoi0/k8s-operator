# Architecture

This document describes the internal design and architecture of k8s-operator's High Availability (HA) system.

## Overview

The HA system uses Raft consensus to ensure only one operator instance (the leader) performs reconciliation at any time. This prevents race conditions and duplicate resource creation while providing automatic failover.

## Layer Architecture

```
  ┌────────────────────────────────────────────────────────────────┐
  │                        APPLICATION LAYER                       │
  │  ┌──────────────────────────────────────────────────────────┐  │
  │  │                    Reconciler Logic                      │  │
  │  │         (only executes on leader, skipped on followers)  │  │
  │  └──────────────────────────────────────────────────────────┘  │
  ├────────────────────────────────────────────────────────────────┤
  │                        CONSENSUS LAYER                         │
  │  ┌──────────────────────────────────────────────────────────┐  │
  │  │                   Raft (openraft)                        │  │
  │  │    • Leader Election    • Log Replication    • Voting    │  │
  │  └──────────────────────────────────────────────────────────┘  │
  ├────────────────────────────────────────────────────────────────┤
  │                        TRANSPORT LAYER                         │
  │  ┌──────────────────────────────────────────────────────────┐  │
  │  │                    gRPC (tonic)                          │  │
  │  │              Vote | AppendEntries | Snapshot             │  │
  │  └──────────────────────────────────────────────────────────┘  │
  ├────────────────────────────────────────────────────────────────┤
  │                        DISCOVERY LAYER                         │
  │  ┌──────────────────────────────────────────────────────────┐  │
  │  │              Headless Service DNS Lookup                 │  │
  │  │         my-operator-0.my-operator-headless.svc           │  │
  │  └──────────────────────────────────────────────────────────┘  │
  └────────────────────────────────────────────────────────────────┘
```

## Single Pod Architecture

Each operator pod contains both the reconciler and Raft components:

```
  ┌────────────────────────────────────────────────┐
  │                 Operator Pod                   │
  │  ┌──────────────┐    ┌──────────────────────┐  │
  │  │  Reconciler  │    │   RaftNodeManager    │  │
  │  │              │    │  ┌────────────────┐  │  │
  │  │ if is_leader │◄───│  │   Raft Node    │  │  │
  │  │   reconcile  │    │  └───────┬────────┘  │  │
  │  └──────────────┘    │          │           │  │
  │                      │  ┌───────▼────────┐  │  │
  │                      │  │  gRPC Server   │  │  │
  │                      │  │    :8080       │  │  │
  │                      │  └────────────────┘  │  │
  │                      └──────────────────────┘  │
  └────────────────────────────────────────────────┘
```

## Cluster Topology

A typical 3-replica StatefulSet deployment:

```
  ┌───────────────────────────────────────────────────────────────────┐
  │                       StatefulSet (3 replicas)                    │
  │                                                                   │
  │   ┌─────────────┐        ┌─────────────┐        ┌─────────────┐   │
  │   │   Pod 0     │◄──────►│   Pod 1     │◄──────►│   Pod 2     │   │
  │   │  (LEADER)   │  gRPC  │ (follower)  │  gRPC  │ (follower)  │   │
  │   └──────┬──────┘        └─────────────┘        └─────────────┘   │
  │          │                     │                      │           │
  │          │ reconciles          │ standby              │ standby   │
  │          ▼                     ▼                      ▼           │
  │   ┌─────────────┐        ┌─────────────┐        ┌─────────────┐   │
  │   │  Creates    │        │   Waits     │        │   Waits     │   │
  │   │ Deployments │        │   (noop)    │        │   (noop)    │   │
  │   │  Services   │        │             │        │             │   │
  │   └─────────────┘        └─────────────┘        └─────────────┘   │
  └───────────────────────────────────────────────────────────────────┘
```

```
                      ┌───────────────────────┐
                      │   Kubernetes API      │
                      └───────────┬───────────┘
                                  │
                                  │ only leader writes
                                  │
                ┌─────────────────┼─────────────────┐
                │                 │                 │
          ┌─────▼─────┐     ┌─────▼─────┐     ┌─────▼─────┐
          │  Pod 0    │     │  Pod 1    │     │  Pod 2    │
          │ ┌───────┐ │     │ ┌───────┐ │     │ ┌───────┐ │
          │ │Leader │ │     │ │Follow │ │     │ │Follow │ │
          │ │  ✓    │ │     │ │  ✗    │ │     │ │  ✗    │ │
          │ └───────┘ │     │ └───────┘ │     │ └───────┘ │
          └─────┬─────┘     └─────┬─────┘     └─────┬─────┘
                │                 │                 │
                └────────────┬────┴────────────────┘
                             │
                      Raft consensus
                      (gRPC :8080)
```

## Raft State Machine

Each node transitions between these states:

```

                      ┌─────────────────┐
                      │    STARTUP      │
                      └────────┬────────┘
                               │
                               ▼
                      ┌─────────────────┐
              ┌──────►│   CANDIDATE     │◄──────┐
              │       │  (requesting    │       │
              │       │    votes)       │       │
              │       └────────┬────────┘       │
              │                │                │
              │   election     │ wins           │ election
              │   timeout      │ election       │ timeout
              │                ▼                │
      ┌───────┴───────┐    ┌─────────────────┐  │
      │   FOLLOWER    │◄───│     LEADER      │──┘
      │               │    │                 │
      │ • Receives    │    │ • Sends         │
      │   heartbeats  │    │   heartbeats    │
      │ • Replicates  │    │ • Runs          │
      │   logs        │    │   reconciler    │
      │ • NO work     │    │ • ALL work      │
      └───────────────┘    └─────────────────┘
              ▲                    │
              │   higher term      │
              │   discovered       │
              └────────────────────┘
```

## Leader Election Flow

When the cluster starts or a leader fails:

```

      Pod 0              Pod 1              Pod 2           Kubernetes
        │                  │                  │                 │
        │◄─────────────────┼──────────────────┼─── Start pods ──┤
        │                  │                  │                 │
        ├── Vote Request ──►                  │                 │
        ├── Vote Request ──┼─────────────────►│                 │
        │                  │                  │                 │
        │◄── Vote Granted ─┤                  │                 │
        │◄── Vote Granted ─┼──────────────────┤                 │
        │                  │                  │                 │
        │  ╔═══════════════════════════════╗  │                 │
        │  ║  Pod 0 becomes LEADER         ║  │                 │
        │  ╚═══════════════════════════════╝  │                 │
        │                  │                  │                 │
        ├── Heartbeat ────►│                  │                 │
        ├── Heartbeat ─────┼─────────────────►│                 │
        │                  │                  │                 │
        ├── Reconcile ─────┼──────────────────┼────────────────►│
        │                  │                  │     (creates    │
        │                  │                  │    Deployment)  │
        │                  │                  │                 │
       ─┼─ Pod 0 crashes ──┼──────────────────┼─────────────────┤
        X                  │                  │                 │
                           │◄─ No heartbeat ──┤                 │
                           │   (timeout)      │                 │
                           │                  │                 │
                           ├── Vote Request ──►                 │
                           │◄── Vote Granted ─┤                 │
                           │                  │                 │
                           │  ╔════════════════════════════╗    │
                           │  ║ Pod 1 becomes LEADER       ║    │
                           │  ╚════════════════════════════╝    │
                           │                  │                 │
                           ├── Reconcile ─────┼────────────────►│
                           │                  │                 │

```

## mTLS Communication

In a Raft cluster with mTLS enabled, each node acts as both client and server:

```
  Node A                                   Node B
  ┌─────────────────────┐                  ┌─────────────────────┐
  │  gRPC Server        │◄─────────────────│  gRPC Client        │
  │  (presents cert,    │   TLS Handshake  │  (presents cert,    │
  │   verifies client)  │                  │   verifies server)  │
  └─────────────────────┘                  └─────────────────────┘
  │  gRPC Client        │─────────────────►│  gRPC Server        │
  │  (presents cert,    │   TLS Handshake  │  (presents cert,    │
  │   verifies server)  │                  │   verifies client)  │
  └─────────────────────┘                  └─────────────────────┘
```

Each node needs:
- **CA certificate** (`ca.crt`): To verify other nodes' certificates
- **Node certificate** (`tls.crt`): To present its own identity
- **Private key** (`tls.key`): To prove ownership of the certificate

## Storage Options

### In-Memory (default with `ha` feature)

Logs and state are stored in memory. Fast but lost on pod restart.

### RocksDB (with `ha-persist` feature)

Logs and state machine data are persisted to disk:
- Survives pod restarts
- Requires PersistentVolume mounted to data directory
- Recommended for production deployments

## Autoscaling Behavior

| Operation | Behavior |
|-----------|----------|
| Scale up | New pods discovered via DNS, added as learners, then voters |
| Scale down | Removed pods detected, membership updated, logs redistributed |
| Pod restart | Same node ID rejoins, catches up from leader's log |
| Leader dies | New election in ~500ms, reconciliation continues |

## Key Components

| Component | Responsibility |
|-----------|----------------|
| `RaftNodeManager` | Manages the Raft node lifecycle and provides leader status |
| `ClusterManager` | Watches DNS for membership changes and updates Raft cluster |
| `RaftNetwork` | gRPC implementation of Raft RPCs (AppendEntries, Vote, Snapshot) |
| `RaftStore` | Log and state machine storage (in-memory or RocksDB) |

## Dependencies

- [openraft](https://github.com/datafuselabs/openraft) - Raft consensus implementation
- [tonic](https://github.com/hyperium/tonic) - gRPC framework
- [rocksdb](https://github.com/rust-rocksdb/rust-rocksdb) - Optional persistent storage
