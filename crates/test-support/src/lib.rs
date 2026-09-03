#![forbid(unsafe_code)]

use std::sync::Mutex;

use async_trait::async_trait;
use free_whisper_domain::{
    ModelId, ProviderCapabilities, RequestId, TranscriptionRequest, TranscriptionResult,
};
use free_whisper_providers::{
    AvailableModel, ProviderError, ProviderHealth, TranscriptionProvider,
};

/// Deterministic provider fake for application and integration tests.
#[derive(Debug)]
pub struct FakeProvider {
    capabilities: ProviderCapabilities,
    result: Mutex<Option<Result<TranscriptionResult, ProviderError>>>,
    requests: Mutex<Vec<RequestId>>,
}

impl FakeProvider {
    #[must_use]
    pub fn new(
        capabilities: ProviderCapabilities,
        result: Result<TranscriptionResult, ProviderError>,
    ) -> Self {
        Self {
            capabilities,
            result: Mutex::new(Some(result)),
            requests: Mutex::new(Vec::new()),
        }
    }

    #[must_use]
    pub fn received_requests(&self) -> Vec<RequestId> {
        self.requests
            .lock()
            .expect("test mutex must not poison")
            .clone()
    }
}

#[async_trait]
impl TranscriptionProvider for FakeProvider {
    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        Ok(ProviderHealth {
            healthy: true,
            detail: "fake provider ready".to_owned(),
        })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn list_models(&self) -> Result<Vec<AvailableModel>, ProviderError> {
        Ok(Vec::new())
    }

    async fn ensure_model_ready(&self, _model: &ModelId) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResult, ProviderError> {
        self.requests
            .lock()
            .expect("test mutex must not poison")
            .push(request.id);
        self.result
            .lock()
            .expect("test mutex must not poison")
            .take()
            .unwrap_or_else(|| Err(ProviderError::Rejected("fake result exhausted".to_owned())))
    }

    async fn cancel(&self, request_id: RequestId) -> Result<(), ProviderError> {
        Err(ProviderError::Cancelled(request_id))
    }
}
