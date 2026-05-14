//! Runner-side workflow behavior for Spool.
//!
//! `spool-runner` owns behavior used by Worker Loops, Agent Launchers,
//! Delivery Adapters, and Managed Source Repository operation locks. The CLI
//! should remain a command facade over these APIs.

pub mod commit_metadata;
pub mod delegate;
pub mod local_worktree_delivery;
pub mod repo_lock;
pub mod review;

pub mod supervisor;
pub mod worker;
