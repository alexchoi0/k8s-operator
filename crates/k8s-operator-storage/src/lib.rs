mod types;

pub use openraft_memstore::{MemStore as MemoryStorage, TypeConfig as MemStoreTypeConfig};
pub use types::*;

pub type LogEntry = Request;
pub type LogResponse = Response;

#[cfg(feature = "rocksdb")]
pub type RocksDbStorage = MemoryStorage;
