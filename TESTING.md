# Testing Guide

This document covers how to test k8s-operator, with a focus on simulating HA clusters locally using tokio.

## Running Tests

```bash
# Run all tests
cargo nextest run

# Run only HA cluster tests
cargo nextest run --features ha

# Run with RocksDB persistence tests
cargo nextest run --features ha-persist

# Run with coverage
cargo llvm-cov nextest --features ha
```

## Simulating HA Clusters with Tokio

The HA tests simulate a full Raft cluster using multiple `RaftNodeManager` instances running on different ports within the same process. This approach lets you test leader election, failover, and membership changes without Kubernetes.

### Basic Setup

```rust
use std::time::Duration;
use k8s_operator::raft::{RaftConfig, RaftNodeManager};

fn test_config(node_id: u64) -> RaftConfig {
    RaftConfig::new("test-cluster")
        .node_id(node_id)
        .service_name("test")
        .namespace("default")
        .election_timeout(Duration::from_millis(500))
        .heartbeat_interval(Duration::from_millis(100))
}
```

### Single Node Test

The simplest test verifies a single node becomes leader:

```rust
#[tokio::test]
async fn single_node_becomes_leader() {
    // Create the node
    let node = RaftNodeManager::new(test_config(0)).await.unwrap();

    // Start gRPC server on a unique port
    node.start_grpc_server(19000).await.unwrap();

    // Allow server to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Bootstrap the cluster (first node only)
    node.bootstrap_or_join().await.unwrap();

    // Start watching for leadership changes
    node.start_leadership_watcher();

    // Wait for election
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify leadership
    assert!(node.leader_election().is_leader());

    // Clean shutdown
    node.shutdown().await.unwrap();
}
```

### Three-Node Cluster Test

To test a realistic cluster:

```rust
#[tokio::test]
async fn three_node_cluster_elects_one_leader() {
    // Create three nodes
    let node0 = RaftNodeManager::new(test_config(0)).await.unwrap();
    let node1 = RaftNodeManager::new(test_config(1)).await.unwrap();
    let node2 = RaftNodeManager::new(test_config(2)).await.unwrap();

    // Start gRPC servers on different ports
    node0.start_grpc_server(19100).await.unwrap();
    node1.start_grpc_server(19101).await.unwrap();
    node2.start_grpc_server(19102).await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Bootstrap with first node
    node0.bootstrap_or_join().await.unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Add other nodes to the cluster
    node0.add_member(1, "127.0.0.1:19101".into()).await.unwrap();
    node0.add_member(2, "127.0.0.1:19102".into()).await.unwrap();

    // Start leadership watchers
    node0.start_leadership_watcher();
    node1.start_leadership_watcher();
    node2.start_leadership_watcher();

    // Wait for election to complete
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify exactly one leader
    let leader_count = [&node0, &node1, &node2]
        .iter()
        .filter(|n| n.leader_election().is_leader())
        .count();

    assert_eq!(leader_count, 1, "Expected exactly one leader");

    // Cleanup
    node0.shutdown().await.unwrap();
    node1.shutdown().await.unwrap();
    node2.shutdown().await.unwrap();
}
```

### Testing Leader Failover

Simulate a leader crash and verify a new leader is elected:

```rust
#[tokio::test]
async fn leader_failure_triggers_new_election() {
    let node0 = RaftNodeManager::new(test_config(0)).await.unwrap();
    let node1 = RaftNodeManager::new(test_config(1)).await.unwrap();
    let node2 = RaftNodeManager::new(test_config(2)).await.unwrap();

    node0.start_grpc_server(19200).await.unwrap();
    node1.start_grpc_server(19201).await.unwrap();
    node2.start_grpc_server(19202).await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    node0.bootstrap_or_join().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    node0.add_member(1, "127.0.0.1:19201".into()).await.unwrap();
    node0.add_member(2, "127.0.0.1:19202".into()).await.unwrap();

    node0.start_leadership_watcher();
    node1.start_leadership_watcher();
    node2.start_leadership_watcher();

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Find and shut down the leader
    let nodes = vec![node0, node1, node2];
    let initial_leader_idx = nodes
        .iter()
        .position(|n| n.leader_election().is_leader())
        .expect("Should have a leader");

    let old_leader_id = nodes[initial_leader_idx].config().node_id;

    // Simulate leader crash
    nodes[initial_leader_idx].shutdown().await.unwrap();

    // Wait for new election (~500ms timeout + election)
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify new leader elected from remaining nodes
    let remaining: Vec<_> = nodes
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != initial_leader_idx)
        .map(|(_, n)| n)
        .collect();

    let new_leader_count = remaining
        .iter()
        .filter(|n| n.leader_election().is_leader())
        .count();

    assert_eq!(new_leader_count, 1, "New leader should be elected");

    // Cleanup remaining nodes
    for node in remaining {
        let _ = node.shutdown().await;
    }
}
```

### Testing Dynamic Membership

Add nodes to a running cluster:

```rust
#[tokio::test]
async fn node_can_be_added_to_cluster() {
    let node0 = RaftNodeManager::new(test_config(0)).await.unwrap();
    node0.start_grpc_server(19300).await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    node0.bootstrap_or_join().await.unwrap();
    node0.start_leadership_watcher();

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(node0.leader_election().is_leader());

    // Add a new node to the cluster
    let node1 = RaftNodeManager::new(test_config(1)).await.unwrap();
    node1.start_grpc_server(19301).await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Register the new node with the leader
    node0.add_member(1, "127.0.0.1:19301".into()).await.unwrap();

    node1.start_leadership_watcher();

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Verify cluster still has exactly one leader
    let leader_count = [&node0, &node1]
        .iter()
        .filter(|n| n.leader_election().is_leader())
        .count();
    assert_eq!(leader_count, 1);

    node0.shutdown().await.unwrap();
    node1.shutdown().await.unwrap();
}
```

Remove nodes from a running cluster:

```rust
#[tokio::test]
async fn node_can_be_removed_from_cluster() {
    let node0 = RaftNodeManager::new(test_config(0)).await.unwrap();
    let node1 = RaftNodeManager::new(test_config(1)).await.unwrap();
    let node2 = RaftNodeManager::new(test_config(2)).await.unwrap();

    node0.start_grpc_server(19400).await.unwrap();
    node1.start_grpc_server(19401).await.unwrap();
    node2.start_grpc_server(19402).await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    node0.bootstrap_or_join().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    node0.add_member(1, "127.0.0.1:19401".into()).await.unwrap();
    node0.add_member(2, "127.0.0.1:19402".into()).await.unwrap();

    node0.start_leadership_watcher();
    node1.start_leadership_watcher();
    node2.start_leadership_watcher();

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Find current leader
    let nodes = [&node0, &node1, &node2];
    let leader = nodes
        .iter()
        .find(|n| n.leader_election().is_leader())
        .expect("Should have a leader");

    // Remove node2 from cluster
    leader.remove_member(2).await.unwrap();

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Verify remaining nodes still have a leader
    let leader_count = [&node0, &node1]
        .iter()
        .filter(|n| n.leader_election().is_leader())
        .count();
    assert_eq!(leader_count, 1);

    node0.shutdown().await.unwrap();
    node1.shutdown().await.unwrap();
    node2.shutdown().await.unwrap();
}
```

## Testing with Parallel Tokio Tasks

For more advanced scenarios, you can spawn nodes in separate tokio tasks:

```rust
use tokio::sync::mpsc;

#[tokio::test]
async fn parallel_node_startup() {
    let (tx, mut rx) = mpsc::channel(3);

    // Spawn nodes in parallel
    for i in 0..3 {
        let tx = tx.clone();
        tokio::spawn(async move {
            let node = RaftNodeManager::new(test_config(i)).await.unwrap();
            node.start_grpc_server(19500 + i as u16).await.unwrap();
            tx.send((i, node)).await.unwrap();
        });
    }

    drop(tx);

    let mut nodes = Vec::new();
    while let Some((id, node)) = rx.recv().await {
        nodes.push((id, node));
    }

    nodes.sort_by_key(|(id, _)| *id);

    // Continue with cluster setup...
}
```

## Testing RocksDB Persistence

With the `ha-persist` feature, test that state survives restarts:

```rust
#[tokio::test]
#[cfg(feature = "ha-persist")]
async fn state_persists_across_restart() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path().to_str().unwrap();

    // Create and bootstrap node
    {
        let node = RaftNodeManager::new_with_rocksdb(
            test_config(0),
            data_path
        ).await.unwrap();

        node.start_grpc_server(19600).await.unwrap();
        node.bootstrap_or_join().await.unwrap();
        node.start_leadership_watcher();

        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(node.leader_election().is_leader());

        node.shutdown().await.unwrap();
    }

    // Restart and verify state
    {
        let node = RaftNodeManager::new_with_rocksdb(
            test_config(0),
            data_path
        ).await.unwrap();

        node.start_grpc_server(19600).await.unwrap();
        node.start_leadership_watcher();

        tokio::time::sleep(Duration::from_secs(2)).await;

        // Node should recover its state and become leader again
        assert!(node.leader_election().is_leader());

        node.shutdown().await.unwrap();
    }
}
```

## Best Practices

### Port Management

Use unique port ranges for each test to avoid conflicts:

```rust
// Test 1: ports 19000-19009
// Test 2: ports 19100-19109
// Test 3: ports 19200-19209
```

### Serial Execution

Use `#[serial]` from `serial_test` crate when tests share resources:

```rust
use serial_test::serial;

#[tokio::test]
#[serial]
async fn my_test() {
    // This test won't run concurrently with other #[serial] tests
}
```

### Timing Considerations

- Allow 100-200ms after starting gRPC servers
- Allow 500ms after bootstrap for initial setup
- Allow 2-3s for leader election to complete
- Allow 3s after leader failure for new election

### Cleanup

Always shut down nodes, even in failure cases:

```rust
#[tokio::test]
async fn test_with_cleanup() {
    let node = RaftNodeManager::new(test_config(0)).await.unwrap();
    node.start_grpc_server(19700).await.unwrap();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| async {
        // Test logic that might panic
    }));

    // Always cleanup
    let _ = node.shutdown().await;

    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
```

## Integration Testing with Kind

For full integration tests with Kubernetes:

```bash
# Create a kind cluster
kind create cluster --name test-cluster

# Build and load operator image
docker build -t my-operator:test .
kind load docker-image my-operator:test --name test-cluster

# Deploy and test
kubectl apply -f deploy/
kubectl wait --for=condition=ready pod -l app=my-operator --timeout=60s

# Run integration tests
cargo nextest run --features integration

# Cleanup
kind delete cluster --name test-cluster
```
