//! Ephemeral, non-serializable request headers that may contain secrets.
//!
//! This module is a thin re-export façade: the canonical owner is the
//! `spider_transport` leaf crate. Values are always marked sensitive on
//! insertion, cloning, and application; the container exposes no value
//! iterator, plaintext map, persistence API, or network execution behavior.

pub use spider_transport::{SecretHeaderError, SecretRequestHeaders};
