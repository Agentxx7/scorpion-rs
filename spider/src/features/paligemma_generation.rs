//! Request-isolated generation-state seam for Candle PaliGemma
//! (`candle_transformers::models::paligemma::Model`, SigLIP + Gemma-1):
//! Candle's `paligemma::Model` owns private mutable KV caches and exposes no
//! reset, so this seam deliberately does not retain a model between
//! requests. [`PaligemmaGenerationFactory`] retains a cloneable immutable
//! `VarBuilder` backend plus configuration, and constructs a fresh
//! model/session for every independent request. Dropping the session
//! discards all request-local model and KV state on success, error,
//! cancellation, panic, or deadline future termination. No Candle fork or
//! private-field access is used.

use candle_nn::VarBuilder;
use candle_transformers::models::paligemma::{Config, Model};
use std::sync::Arc;

/// Failure to construct fresh request-local generation state.
#[derive(Debug)]
pub struct PaligemmaGenerationStateError(candle::Error);

impl std::fmt::Display for PaligemmaGenerationStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("failed to construct isolated PaliGemma generation state")
    }
}

impl std::error::Error for PaligemmaGenerationStateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// Persistent immutable construction resources for request-local PaliGemma
/// sessions. It owns neither active generation state nor a model with a
/// populated KV cache.
pub struct PaligemmaGenerationFactory {
    config: Config,
    weights: VarBuilder<'static>,
    serialized: Arc<tokio::sync::Mutex<()>>,
}

impl PaligemmaGenerationFactory {
    /// Bind an already-created offline weight backend and pinned model config.
    /// This performs no network access, artifact lookup, or model discovery.
    pub fn new(config: Config, weights: VarBuilder<'static>) -> Self {
        Self {
            config,
            weights,
            serialized: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Construct one fresh, non-clonable request session. Candle creates new
    /// empty KV caches in every `Model::new` invocation. The cloned
    /// `VarBuilder` shares only its immutable backend through `Arc`.
    pub async fn begin_request(
        &self,
    ) -> Result<PaligemmaGenerationSession, PaligemmaGenerationStateError> {
        let permit = Arc::clone(&self.serialized).lock_owned().await;
        let model = Model::new(&self.config, self.weights.clone())
            .map_err(PaligemmaGenerationStateError)?;
        Ok(PaligemmaGenerationSession {
            model,
            _serialized_permit: permit,
        })
    }

    /// Explicitly unload factory-owned references. Active request sessions
    /// cannot coexist with this consuming call unless separately owned by the
    /// caller; the type exposes no shared model or cache handle.
    pub fn unload(self) {}
}

/// One independent request's model and mutable KV state.
///
/// The session intentionally has no `Clone`, cache accessor, reset operation,
/// or way to return its model to the factory. Drop is infallible state discard.
pub struct PaligemmaGenerationSession {
    model: Model,
    _serialized_permit: tokio::sync::OwnedMutexGuard<()>,
}

impl PaligemmaGenerationSession {
    /// Mutable access to the request-local model. No other session or the
    /// factory can observe or reach this instance's state.
    pub fn model(&mut self) -> &mut Model {
        &mut self.model
    }
}
