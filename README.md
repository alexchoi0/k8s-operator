# k8s-operator

A batteries-included Kubernetes operator framework for Rust with built-in high availability.

[![Crates.io](https://img.shields.io/crates/v/k8s-operator.svg)](https://crates.io/crates/k8s-operator)
[![License](https://img.shields.io/crates/l/k8s-operator.svg)](LICENSE)

## Why k8s-operator?

Building Kubernetes operators in Rust typically means wrestling with `kube-rs` and `k8s-openapi` — powerful libraries, but with a steep learning curve and lots of boilerplate. This framework handles the heavy lifting so you can focus on your business logic.

**Before** (raw kube-rs):
```rust
// 50+ lines of imports, API clients, watchers, error handling...
```

**After** (k8s-operator):
```rust
Operator::<MyResource>::new()
    .reconcile(my_reconcile_fn)
    .run()
    .await
```

## Features

- **Zero k8s-openapi exposure** — Intuitive typed builders for Deployments, Services, ConfigMaps, and more
- **Simple reconciliation** — Just implement an async function
- **Automatic finalizers** — Optional cleanup hooks built in
- **HA/Raft support** — Leader election for high availability with ~500ms failover
- **Type-safe** — Catch mistakes at compile time, not in production

## Quick Start

```toml
[dependencies]
k8s-operator = { version = "0.3", features = ["ha"] }
tokio = { version = "1", features = ["full"] }
```

```rust
use k8s_operator::prelude::*;
use k8s_operator::{RaftConfig, RaftNodeManager};

#[derive(CustomResource, Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[kube(group = "example.com", version = "v1", kind = "WebApp", namespaced)]
pub struct WebAppSpec {
    pub image: String,
    pub replicas: u32,
}

async fn reconcile(ctx: Context<WebApp>) -> Result<Action> {
    let app = ctx.resource();

    ctx.apply(
        Deployment::new(ctx.name())
            .replicas(app.spec.replicas)
            .container(Container::new("app").image(&app.spec.image).port(80))
    ).await?;

    ctx.apply(
        Service::new(ctx.name())
            .port(80, 80)
            .selector(Selector::new().insert("app", ctx.name()))
    ).await?;

    Ok(Action::requeue(Duration::from_secs(300)))
}

#[tokio::main]
async fn main() -> Result<()> {
    let node_id = RaftNodeManager::node_id_from_hostname()?;

    Operator::<WebApp>::new()
        .reconcile(reconcile)
        .with_leader_election(
            RaftConfig::new("my-cluster")
                .node_id(node_id)
                .service_name("my-operator-headless")
                .namespace("default")
        )
        .run()
        .await
}
```

## Kubernetes Deployment

Deploy as a StatefulSet with a headless service for peer discovery:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: my-operator-headless
spec:
  clusterIP: None
  selector:
    app: my-operator
  ports:
    - port: 8080
      name: raft
---
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: my-operator
spec:
  serviceName: my-operator-headless
  replicas: 3
  selector:
    matchLabels:
      app: my-operator
  template:
    metadata:
      labels:
        app: my-operator
    spec:
      containers:
        - name: operator
          image: my-operator:latest
          ports:
            - containerPort: 8080
              name: raft
          env:
            - name: HOSTNAME
              valueFrom:
                fieldRef:
                  fieldPath: metadata.name
```

## How HA Works

```
  ┌─────────────┐        ┌─────────────┐        ┌─────────────┐
  │   Pod 0     │◄──────►│   Pod 1     │◄──────►│   Pod 2     │
  │  (LEADER)   │  Raft  │ (follower)  │  Raft  │ (follower)  │
  └──────┬──────┘        └─────────────┘        └─────────────┘
         │                     │                      │
         │ reconciles          │ standby              │ standby
         ▼                     ▼                      ▼
  ┌─────────────┐        ┌─────────────┐        ┌─────────────┐
  │  Creates    │        │   Waits     │        │   Waits     │
  │ Deployments │        │   (noop)    │        │   (noop)    │
  └─────────────┘        └─────────────┘        └─────────────┘
```

Only the leader runs reconciliation. If it fails, a new leader is elected in ~500ms.

## Feature Flags

| Feature | Description |
|---------|-------------|
| `ha` | Enable Raft-based leader election (in-memory storage) |
| `ha-persist` | HA with RocksDB persistence (survives pod restarts) |

## Documentation

| Document | Description |
|----------|-------------|
| [User Manual](USER_MANUAL.md) | Complete usage guide with all resource builders |
| [Architecture](ARCHITECTURE.md) | Technical design and diagrams |
| [Testing](TESTING.md) | How to test HA clusters locally with tokio |

## Available Resource Builders

| Category | Types |
|----------|-------|
| **Workloads** | `Deployment`, `StatefulSet`, `DaemonSet`, `Job`, `CronJob`, `Pod` |
| **Config** | `ConfigMap`, `Secret` |
| **Networking** | `Service`, `Ingress`, `NetworkPolicy` |
| **Storage** | `PersistentVolumeClaim`, `Volume` |
| **RBAC** | `ServiceAccount`, `Role`, `RoleBinding`, `ClusterRole`, `ClusterRoleBinding` |

## Contributing

Found a bug? Have a feature request? Contributions are welcome! Feel free to open an issue or submit a pull request.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
