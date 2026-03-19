#![allow(unused_crate_dependencies)]

pub mod assignment_comm;
mod authority_store;
pub mod client;
pub mod process;
pub mod service;
pub mod supervisor;

pub use client::{EventSubscription, OrcasIpcClient};
pub use process::{
    OrcasDaemonLaunch, OrcasDaemonProcessManager, OrcasDaemonSocketStatus, OrcasRuntimeOverrides,
    apply_runtime_overrides,
};
pub use service::OrcasDaemonService;
