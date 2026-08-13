//! Request-isolated generation-state seam for Candle Qwen3-VL.
//!
//! Candle's `Qwen3VLModel` owns private mutable KV caches and exposes no reset.
//! This seam therefore deliberately does not retain a model between requests.
//! [`Qwen3VlGenerationFactory`] retains a cloneable immutable `VarBuilder`
//! backend plus configuration, and constructs a fresh model/session for every
//! independent request. Dropping the session discards all request-local model
//! and KV state on success, error, cancellation, panic, or deadline future
//! termination. No Candle fork or private-field access is used.

use candle_nn::VarBuilder;
use candle_transformers::models::qwen3_vl::{Config, Qwen3VLModel};
use std::sync::Arc;

/// Failure to construct fresh request-local generation state.
#[derive(Debug)]
pub struct Qwen3VlGenerationStateError(candle::Error);

impl std::fmt::Display for Qwen3VlGenerationStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("failed to construct isolated Qwen3-VL generation state")
    }
}

impl std::error::Error for Qwen3VlGenerationStateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// Persistent immutable construction resources for request-local Qwen3-VL
/// sessions. It owns neither active generation state nor a model with a
/// populated KV cache.
pub struct Qwen3VlGenerationFactory {
    config: Config,
    weights: VarBuilder<'static>,
    serialized: Arc<tokio::sync::Mutex<()>>,
}

impl Qwen3VlGenerationFactory {
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
    /// empty KV caches in every `Qwen3VLModel::new` invocation. The cloned
    /// `VarBuilder` shares only its immutable backend through `Arc`.
    pub async fn begin_request(
        &self,
    ) -> Result<Qwen3VlGenerationSession, Qwen3VlGenerationStateError> {
        let permit = Arc::clone(&self.serialized).lock_owned().await;
        let model = Qwen3VLModel::new(&self.config, self.weights.clone())
            .map_err(Qwen3VlGenerationStateError)?;
        Ok(Qwen3VlGenerationSession {
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
pub struct Qwen3VlGenerationSession {
    model: Qwen3VLModel,
    _serialized_permit: tokio::sync::OwnedMutexGuard<()>,
}

impl Qwen3VlGenerationSession {
    /// Execute through the request-local model. All cache mutations performed
    /// by Candle remain confined to this session's private model.
    pub fn model(&self) -> &Qwen3VLModel {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::{Qwen3VlGenerationFactory, Qwen3VlGenerationSession};
    use candle::{DType, Device, Tensor};
    use candle_nn::{Activation, VarBuilder, VarMap};
    use candle_transformers::models::qwen3_vl::{
        config::{TextConfig, VisionConfig},
        Config,
    };
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Structural reference model for the production factory's ownership
    /// rule. It permits deterministic state tests without model artifacts.
    #[derive(Default)]
    struct FixtureWeights;

    struct FixtureFactory {
        immutable: Arc<FixtureWeights>,
        completed: Arc<Mutex<Vec<&'static str>>>,
        serialized: Arc<tokio::sync::Mutex<()>>,
    }

    struct FixtureSession {
        _immutable: Arc<FixtureWeights>,
        kv: Vec<&'static str>,
        completed: Arc<Mutex<Vec<&'static str>>>,
        _permit: tokio::sync::OwnedMutexGuard<()>,
    }

    impl FixtureFactory {
        async fn begin(&self) -> FixtureSession {
            FixtureSession {
                _immutable: Arc::clone(&self.immutable),
                kv: Vec::new(),
                completed: Arc::clone(&self.completed),
                _permit: Arc::clone(&self.serialized).lock_owned().await,
            }
        }
    }

    impl FixtureSession {
        fn generate(&mut self, input: &'static str) -> &'static str {
            assert!(self.kv.is_empty(), "request inherited KV state");
            self.kv.push(input);
            input
        }
    }

    impl Drop for FixtureSession {
        fn drop(&mut self) {
            self.completed.lock().unwrap().extend(self.kv.drain(..));
        }
    }

    fn fixture_factory() -> FixtureFactory {
        FixtureFactory {
            immutable: Arc::new(FixtureWeights),
            completed: Arc::new(Mutex::new(Vec::new())),
            serialized: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn real_tiny_factory() -> Qwen3VlGenerationFactory {
        let varmap = Box::leak(Box::new(VarMap::new()));
        let weights = VarBuilder::from_varmap(varmap, DType::F32, &Device::Cpu);
        let config = Config {
            text_config: TextConfig {
                head_dim: 8,
                vocab_size: 16,
                hidden_size: 8,
                intermediate_size: 16,
                num_hidden_layers: 1,
                num_attention_heads: 1,
                num_key_value_heads: 1,
                hidden_act: Activation::Silu,
                max_position_embeddings: 32,
                rms_norm_eps: 1e-6,
                tie_word_embeddings: true,
                rope_theta: 10_000.0,
                sliding_window: None,
            },
            vision_config: VisionConfig {
                depth: 0,
                hidden_size: 8,
                out_hidden_size: 8,
                hidden_act: Activation::Gelu,
                intermediate_size: 16,
                num_heads: 1,
                in_chans: 3,
                patch_size: 2,
                spatial_merge_size: 2,
                temporal_patch_size: 2,
                num_position_embeddings: 4,
                deepstack_visual_indexes: Vec::new(),
            },
            image_token_id: 12,
            video_token_id: 13,
            vision_start_token_id: 14,
            vision_end_token_id: 15,
        };
        Qwen3VlGenerationFactory::new(config, weights)
    }

    fn text_forward(session: &Qwen3VlGenerationSession, token: u32, offset: usize) -> Vec<f32> {
        let input = Tensor::new(&[[token]], &Device::Cpu).unwrap();
        session
            .model()
            .forward(
                &input,
                None,
                None,
                None,
                None,
                vec![1],
                vec![Vec::new()],
                vec![Vec::new()],
                &[offset],
            )
            .unwrap()
            .flatten_all()
            .unwrap()
            .to_vec1::<f32>()
            .unwrap()
    }

    #[tokio::test]
    async fn actual_candle_qwen_request_b_matches_fresh_session_b() {
        let factory = real_tiny_factory();
        {
            let request_a = factory.begin_request().await.unwrap();
            let _ = text_forward(&request_a, 1, 0);
            let _ = text_forward(&request_a, 2, 1);
        }
        let output_b = {
            let request_b = factory.begin_request().await.unwrap();
            text_forward(&request_b, 3, 0)
        };
        let fresh_output_b = {
            let fresh_b = factory.begin_request().await.unwrap();
            text_forward(&fresh_b, 3, 0)
        };
        assert_eq!(output_b, fresh_output_b);
    }

    #[tokio::test]
    async fn sequential_request_b_matches_fresh_runtime_b() {
        let factory = fixture_factory();
        let output_a = {
            let mut request_a = factory.begin().await;
            request_a.generate("A")
        };
        let output_b = {
            let mut request_b = factory.begin().await;
            request_b.generate("B")
        };
        let fresh_output_b = {
            let fresh = fixture_factory();
            let mut request_b = fresh.begin().await;
            request_b.generate("B")
        };
        assert_eq!(output_a, "A");
        assert_eq!(output_b, fresh_output_b);
        assert_eq!(*factory.completed.lock().unwrap(), ["A", "B"]);
    }

    #[tokio::test]
    async fn success_and_model_error_discard_state() {
        let factory = fixture_factory();
        for termination in ["success", "model-error"] {
            {
                let mut request = factory.begin().await;
                assert_eq!(request.generate(termination), termination);
                // Every Rust termination path drops this request-owned value;
                // no cleanup callback can fail or restore it to the factory.
            }
            let mut next = factory.begin().await;
            assert_eq!(next.generate("fresh"), "fresh");
        }
    }

    #[tokio::test]
    async fn cancellation_and_deadline_drop_request_state_and_permit() {
        let factory = Arc::new(fixture_factory());
        let cancelled_factory = Arc::clone(&factory);
        let (session_started, session_started_rx) = tokio::sync::oneshot::channel();
        let cancelled = tokio::spawn(async move {
            let mut request = cancelled_factory.begin().await;
            request.generate("cancelled");
            session_started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        session_started_rx.await.unwrap();
        cancelled.abort();
        assert!(cancelled.await.unwrap_err().is_cancelled());
        let mut after_cancel = factory.begin().await;
        assert_eq!(after_cancel.generate("after-cancel"), "after-cancel");
        drop(after_cancel);

        let deadline_factory = Arc::clone(&factory);
        let deadline = tokio::time::timeout(Duration::from_millis(1), async move {
            let mut request = deadline_factory.begin().await;
            request.generate("deadline");
            std::future::pending::<()>().await;
        })
        .await;
        assert!(deadline.is_err());
        let mut after_deadline = factory.begin().await;
        assert_eq!(after_deadline.generate("after-deadline"), "after-deadline");
    }

    #[tokio::test]
    async fn immutable_resources_are_reused_but_mutable_state_is_not() {
        let factory = fixture_factory();
        let first = factory.begin().await;
        let immutable = Arc::clone(&first._immutable);
        assert!(first.kv.is_empty());
        drop(first);
        let second = factory.begin().await;
        assert!(Arc::ptr_eq(&immutable, &second._immutable));
        assert!(second.kv.is_empty());
    }
}
