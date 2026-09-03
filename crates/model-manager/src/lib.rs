#![forbid(unsafe_code)]

//! Signed, explicit model installation. The manager has no background update
//! path: callers must provide a manifest already verified by the release key.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use free_whisper_domain::ModelId;
use futures_util::StreamExt;
use reqwest::{StatusCode, header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const MANIFEST_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPreset {
    Fast,
    Balanced,
    HighQuality,
}

impl ModelPreset {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::HighQuality => "high_quality",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelBackend {
    Cpu,
    Cuda,
    Vulkan,
    OpenVino,
}

/// The signed distribution channel for a model catalogue. A public key alone
/// is not enough to distinguish a pre-release catalogue from a stable release
/// catalogue, so the channel is part of the canonical, signed JSON payload.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestTrustScope {
    Alpha,
    Stable,
}

impl ManifestTrustScope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Alpha => "alpha",
            Self::Stable => "stable",
        }
    }
}

impl ModelBackend {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
            Self::Vulkan => "vulkan",
            Self::OpenVino => "open_vino",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelManifest {
    pub schema_version: u16,
    pub manifest_version: String,
    pub trust_scope: ManifestTrustScope,
    pub models: Vec<ModelManifestEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelManifestEntry {
    pub id: String,
    pub display_name: String,
    pub preset: ModelPreset,
    pub revision: String,
    pub download_url: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub license: String,
    pub license_url: String,
    pub source_url: String,
    pub backend: ModelBackend,
    pub quantization: String,
    pub estimated_ram_bytes: u64,
}

impl ModelManifestEntry {
    pub fn model_id(&self) -> Result<ModelId, ModelManagerError> {
        ModelId::parse(self.id.clone())
            .map_err(|error| ModelManagerError::InvalidManifest(error.to_string()))
    }

    fn validate(&self) -> Result<(), ModelManagerError> {
        self.model_id()?;
        if self.display_name.trim().is_empty()
            || self.revision.trim().is_empty()
            || self.license.trim().is_empty()
            || self.quantization.trim().is_empty()
            || self.backend != ModelBackend::Cpu
            || self.size_bytes == 0
            || self.estimated_ram_bytes == 0
        {
            return Err(ModelManagerError::InvalidManifest(
                "model metadata is incomplete or requests an unsupported backend".to_owned(),
            ));
        }
        let url = reqwest::Url::parse(&self.download_url).map_err(|error| {
            ModelManagerError::InvalidManifest(format!("invalid download URL: {error}"))
        })?;
        if url.scheme() != "https" || url.query().is_some() || url.fragment().is_some() {
            return Err(ModelManagerError::InvalidManifest(
                "model downloads must use a query-free HTTPS URL".to_owned(),
            ));
        }
        for url in [&self.license_url, &self.source_url] {
            if !reqwest::Url::parse(url)
                .map(|url| url.scheme() == "https")
                .unwrap_or(false)
            {
                return Err(ModelManagerError::InvalidManifest(
                    "model provenance URLs must use HTTPS".to_owned(),
                ));
            }
        }
        if Path::new(&self.file_name)
            .file_name()
            .and_then(|value| value.to_str())
            != Some(self.file_name.as_str())
        {
            return Err(ModelManagerError::InvalidManifest(
                "model file name must not contain a path".to_owned(),
            ));
        }
        let digest = hex::decode(&self.sha256).map_err(|_| {
            ModelManagerError::InvalidManifest("SHA-256 is not hexadecimal".to_owned())
        })?;
        if digest.len() != 32 {
            return Err(ModelManagerError::InvalidManifest(
                "SHA-256 must contain exactly 32 bytes".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedManifest(ModelManifest);

impl VerifiedManifest {
    #[must_use]
    pub fn manifest(&self) -> &ModelManifest {
        &self.0
    }

    pub fn model(&self, id: &ModelId) -> Result<&ModelManifestEntry, ModelManagerError> {
        self.0
            .models
            .iter()
            .find(|entry| entry.id == id.as_str())
            .ok_or_else(|| ModelManagerError::UnknownModel(id.as_str().to_owned()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestVerifier {
    public_key: VerifyingKey,
}

impl ManifestVerifier {
    pub fn from_base64(value: &str) -> Result<Self, ModelManagerError> {
        let bytes = BASE64
            .decode(value)
            .map_err(|_| ModelManagerError::InvalidPublicKey)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| ModelManagerError::InvalidPublicKey)?;
        Ok(Self {
            public_key: VerifyingKey::from_bytes(&bytes)
                .map_err(|_| ModelManagerError::InvalidPublicKey)?,
        })
    }

    pub fn verify(
        &self,
        manifest_bytes: &[u8],
        signature_base64: &str,
    ) -> Result<VerifiedManifest, ModelManagerError> {
        let manifest: ModelManifest = serde_json::from_slice(manifest_bytes).map_err(|error| {
            ModelManagerError::InvalidManifest(format!("invalid JSON: {error}"))
        })?;
        validate_manifest(&manifest)?;
        let signature = BASE64
            .decode(signature_base64.trim())
            .map_err(|_| ModelManagerError::InvalidSignature)?;
        let signature =
            Signature::from_slice(&signature).map_err(|_| ModelManagerError::InvalidSignature)?;
        let canonical = canonical_manifest_bytes(&manifest)?;
        self.public_key
            .verify(&canonical, &signature)
            .map_err(|_| ModelManagerError::InvalidSignature)?;
        Ok(VerifiedManifest(manifest))
    }

    /// Verifies both cryptographic integrity and the release channel expected
    /// by the embedding application. This prevents a valid Alpha key from
    /// authorising a stable binary.
    pub fn verify_for_scope(
        &self,
        manifest_bytes: &[u8],
        signature_base64: &str,
        expected_scope: ManifestTrustScope,
    ) -> Result<VerifiedManifest, ModelManagerError> {
        let verified = self.verify(manifest_bytes, signature_base64)?;
        if verified.manifest().trust_scope != expected_scope {
            return Err(ModelManagerError::UnexpectedTrustScope {
                expected: expected_scope.as_str(),
                actual: verified.manifest().trust_scope.as_str(),
            });
        }
        Ok(verified)
    }
}

pub fn sign_manifest_for_release(
    manifest_bytes: &[u8],
    private_key_base64: &str,
) -> Result<String, ModelManagerError> {
    let manifest: ModelManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| ModelManagerError::InvalidManifest(format!("invalid JSON: {error}")))?;
    validate_manifest(&manifest)?;
    let signature =
        signing_key_from_base64(private_key_base64)?.sign(&canonical_manifest_bytes(&manifest)?);
    Ok(BASE64.encode(signature.to_bytes()))
}

/// Derives the release public key without exposing or persisting the private
/// signing seed. Release automation writes this value into the distributable.
pub fn public_key_for_release(private_key_base64: &str) -> Result<String, ModelManagerError> {
    Ok(BASE64.encode(
        signing_key_from_base64(private_key_base64)?
            .verifying_key()
            .to_bytes(),
    ))
}

fn signing_key_from_base64(private_key_base64: &str) -> Result<SigningKey, ModelManagerError> {
    let private_key = BASE64
        .decode(private_key_base64)
        .map_err(|_| ModelManagerError::InvalidSigningKey)?;
    let private_key: [u8; 32] = private_key
        .try_into()
        .map_err(|_| ModelManagerError::InvalidSigningKey)?;
    Ok(SigningKey::from_bytes(&private_key))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelDownloadProgress {
    pub model_id: ModelId,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub bytes_per_second: u64,
    pub estimated_remaining_seconds: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledModel {
    pub model_id: ModelId,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    pub backend: String,
    pub quantization: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteConfirmation {
    ConfirmedByUser,
}

#[derive(Debug)]
pub struct ModelManager {
    root: PathBuf,
    client: reqwest::Client,
    verifier: ManifestVerifier,
}

impl ModelManager {
    pub fn new(root: PathBuf, verifier: ManifestVerifier) -> Result<Self, ModelManagerError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30 * 60))
            .build()
            .map_err(ModelManagerError::HttpClient)?;
        Ok(Self {
            root,
            client,
            verifier,
        })
    }

    pub fn verify_manifest(
        &self,
        manifest_bytes: &[u8],
        signature_base64: &str,
    ) -> Result<VerifiedManifest, ModelManagerError> {
        self.verifier.verify(manifest_bytes, signature_base64)
    }

    pub async fn install(
        &self,
        manifest: &VerifiedManifest,
        id: &ModelId,
        cancellation: &CancellationToken,
        mut progress: impl FnMut(ModelDownloadProgress),
    ) -> Result<InstalledModel, ModelManagerError> {
        let entry = manifest.model(id)?;
        let destination = self.model_path(entry)?;
        if destination.is_file() {
            return self.validate_installed(entry, destination).await;
        }
        fs::create_dir_all(self.download_directory()).await?;
        let partial = self.partial_path(entry);
        let resume = self.resume_path(entry);
        self.recover_legacy_staging_partial(entry, &partial, &resume)
            .await?;
        if cancellation.is_cancelled() {
            return Err(ModelManagerError::Cancelled);
        }
        let resume_info = resume_offset(&partial, &resume, entry).await?;
        let mut downloaded = resume_info.downloaded_bytes;
        if downloaded == entry.size_bytes {
            verify_file_hash(&partial, &entry.sha256).await?;
            return self.activate(entry, &partial, &resume).await;
        }
        let mut request = self.client.get(&entry.download_url);
        if downloaded > 0 {
            request = request.header(header::RANGE, format!("bytes={downloaded}-"));
            if let Some(etag) = resume_info.etag {
                request = request.header(header::IF_RANGE, etag);
            }
        }
        let request = request.send();
        tokio::pin!(request);
        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(ModelManagerError::Cancelled),
            response = &mut request => response.map_err(ModelManagerError::Download)?,
        };
        if downloaded > 0 && response.status() == StatusCode::OK {
            fs::remove_file(&partial).await?;
            let _ = fs::remove_file(&resume).await;
            downloaded = 0;
        } else if !(response.status() == StatusCode::OK
            || response.status() == StatusCode::PARTIAL_CONTENT)
        {
            return Err(ModelManagerError::DownloadStatus(response.status()));
        }
        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        write_resume_state(&resume, entry, etag).await?;
        let mut file = if downloaded == 0 {
            fs::File::create(&partial).await?
        } else {
            fs::OpenOptions::new().append(true).open(&partial).await?
        };
        let mut stream = response.bytes_stream();
        let download_started = std::time::Instant::now();
        loop {
            let chunk = tokio::select! {
                _ = cancellation.cancelled() => {
                    file.flush().await?;
                    return Err(ModelManagerError::Cancelled);
                }
                chunk = stream.next() => chunk,
            };
            let Some(chunk) = chunk else {
                break;
            };
            if cancellation.is_cancelled() {
                file.flush().await?;
                return Err(ModelManagerError::Cancelled);
            }
            let chunk = chunk.map_err(ModelManagerError::Download)?;
            downloaded = downloaded
                .checked_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX))
                .ok_or(ModelManagerError::SizeOverflow)?;
            if downloaded > entry.size_bytes {
                return Err(ModelManagerError::SizeMismatch {
                    expected: entry.size_bytes,
                    actual: downloaded,
                });
            }
            file.write_all(&chunk).await?;
            let elapsed = download_started.elapsed().as_secs_f64();
            let bytes_per_second = if elapsed > 0.0 {
                (downloaded as f64 / elapsed) as u64
            } else {
                0
            };
            let remaining = entry.size_bytes.saturating_sub(downloaded);
            progress(ModelDownloadProgress {
                model_id: id.clone(),
                downloaded_bytes: downloaded,
                total_bytes: entry.size_bytes,
                bytes_per_second,
                estimated_remaining_seconds: (bytes_per_second > 0)
                    .then(|| remaining.div_ceil(bytes_per_second)),
            });
        }
        file.flush().await?;
        // Windows does not allow a still-open download handle to be moved into
        // the atomic staging directory. Closing it here also makes the hash
        // validation observe the fully flushed file.
        drop(file);
        if downloaded != entry.size_bytes {
            return Err(ModelManagerError::SizeMismatch {
                expected: entry.size_bytes,
                actual: downloaded,
            });
        }
        verify_file_hash(&partial, &entry.sha256).await?;
        self.activate(entry, &partial, &resume).await
    }

    pub async fn delete(
        &self,
        id: &ModelId,
        _: DeleteConfirmation,
    ) -> Result<(), ModelManagerError> {
        let directory = self.model_directory(id)?;
        if !directory.is_dir() {
            return Err(ModelManagerError::UnknownModel(id.as_str().to_owned()));
        }
        fs::remove_dir_all(directory).await?;
        Ok(())
    }

    /// Revalidates an installed model without opening a network connection.
    /// Model activation exclusively uses this path.
    pub async fn validate_installed_model(
        &self,
        manifest: &VerifiedManifest,
        id: &ModelId,
    ) -> Result<InstalledModel, ModelManagerError> {
        let entry = manifest.model(id)?;
        let destination = self.model_path(entry)?;
        if !destination.is_file() {
            return Err(ModelManagerError::NotInstalled(id.as_str().to_owned()));
        }
        self.validate_installed(entry, destination).await
    }

    fn download_directory(&self) -> PathBuf {
        self.root.join(".downloads")
    }
    fn model_path(&self, entry: &ModelManifestEntry) -> Result<PathBuf, ModelManagerError> {
        Ok(self
            .model_directory(&entry.model_id()?)?
            .join(&entry.file_name))
    }

    /// Model IDs are namespaced (for example `whisper.cpp/tiny`), so their
    /// directory must be derived segment-by-segment. This prevents a manifest
    /// or command argument from escaping the configured model root on Windows.
    fn model_directory(&self, id: &ModelId) -> Result<PathBuf, ModelManagerError> {
        let mut directory = self.root.clone();
        for segment in id.as_str().split('/') {
            if segment.is_empty()
                || matches!(segment, "." | "..")
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                return Err(ModelManagerError::UnsafeModelStoragePath(
                    id.as_str().to_owned(),
                ));
            }
            directory.push(segment);
        }
        Ok(directory)
    }
    fn partial_path(&self, entry: &ModelManifestEntry) -> PathBuf {
        self.download_directory()
            .join(format!("{}.partial", download_key(entry)))
    }
    fn resume_path(&self, entry: &ModelManifestEntry) -> PathBuf {
        self.download_directory()
            .join(format!("{}.resume.json", download_key(entry)))
    }

    /// Restores the only layout emitted by the affected alpha build. That
    /// build embedded a namespaced ID in its staging path, e.g.
    /// `.whisper.cpp/tiny-<uuid>.staging`. Recovery is deliberately narrow:
    /// it only moves one matching staged file when the signed resume metadata
    /// is present, then normal hash validation runs as usual.
    async fn recover_legacy_staging_partial(
        &self,
        entry: &ModelManifestEntry,
        partial: &Path,
        resume: &Path,
    ) -> Result<(), ModelManagerError> {
        if partial.is_file() || !resume.is_file() {
            return Ok(());
        }
        let (parent, prefix) = self.legacy_staging_parent(entry)?;
        let mut candidates = match fs::read_dir(&parent).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(ModelManagerError::Io(error)),
        };
        let mut recovered = None;
        while let Some(candidate) = candidates.next_entry().await? {
            let name = candidate.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&prefix) || !name.ends_with(".staging") {
                continue;
            }
            let staged_file = candidate.path().join(&entry.file_name);
            if !staged_file.is_file() {
                continue;
            }
            if recovered.replace((candidate.path(), staged_file)).is_some() {
                return Err(ModelManagerError::AmbiguousLegacyStaging(entry.id.clone()));
            }
        }
        let Some((staging, staged_file)) = recovered else {
            return Ok(());
        };
        fs::rename(&staged_file, partial).await?;
        remove_staging_directory(&staging).await
    }

    fn legacy_staging_parent(
        &self,
        entry: &ModelManifestEntry,
    ) -> Result<(PathBuf, String), ModelManagerError> {
        let id = entry.model_id()?;
        // Run the same segment validation as the current storage layout before
        // constructing the legacy path from the signed ID.
        self.model_directory(&id)?;
        let mut segments = id.as_str().split('/').collect::<Vec<_>>();
        let leaf = segments
            .pop()
            .ok_or_else(|| ModelManagerError::UnsafeModelStoragePath(entry.id.clone()))?;
        let mut parent = self.root.clone();
        let prefix = if let Some(first) = segments.first() {
            parent.push(format!(".{first}"));
            for segment in &segments[1..] {
                parent.push(segment);
            }
            format!("{leaf}-")
        } else {
            format!(".{leaf}-")
        };
        Ok((parent, prefix))
    }

    async fn validate_installed(
        &self,
        entry: &ModelManifestEntry,
        path: PathBuf,
    ) -> Result<InstalledModel, ModelManagerError> {
        let size = fs::metadata(&path).await?.len();
        if size != entry.size_bytes {
            return Err(ModelManagerError::SizeMismatch {
                expected: entry.size_bytes,
                actual: size,
            });
        }
        verify_file_hash(&path, &entry.sha256).await?;
        Ok(installed(entry, path))
    }

    async fn activate(
        &self,
        entry: &ModelManifestEntry,
        partial: &Path,
        resume: &Path,
    ) -> Result<InstalledModel, ModelManagerError> {
        let destination = self.model_directory(&entry.model_id()?)?;
        let parent = destination
            .parent()
            .ok_or_else(|| ModelManagerError::UnsafeModelStoragePath(entry.id.clone()))?;
        fs::create_dir_all(parent).await?;
        if destination.exists() {
            return Err(ModelManagerError::DestinationExists(entry.id.clone()));
        }

        // Keep the staging directory beside the final model directory. The
        // generated name contains no model ID, so namespaced IDs never become
        // accidental paths such as `.whisper.cpp/tiny-*.staging` on Windows.
        let staging = parent.join(format!(".install-{}.staging", Uuid::new_v4()));
        fs::create_dir_all(&staging).await?;
        let staged_file = staging.join(&entry.file_name);
        if let Err(error) = fs::rename(partial, &staged_file).await {
            remove_staging_directory(&staging).await?;
            return Err(ModelManagerError::Io(error));
        }
        if destination.exists() {
            restore_partial(&staged_file, partial, &staging).await?;
            return Err(ModelManagerError::DestinationExists(entry.id.clone()));
        }
        if let Err(error) = fs::rename(&staging, &destination).await {
            restore_partial(&staged_file, partial, &staging).await?;
            if destination.exists() {
                return Err(ModelManagerError::DestinationExists(entry.id.clone()));
            }
            return Err(ModelManagerError::Io(error));
        }
        remove_file_if_present(resume).await?;
        Ok(installed(entry, destination.join(&entry.file_name)))
    }
}

async fn restore_partial(
    staged_file: &Path,
    partial: &Path,
    staging: &Path,
) -> Result<(), ModelManagerError> {
    fs::rename(staged_file, partial).await?;
    remove_staging_directory(staging).await
}

async fn remove_staging_directory(path: &Path) -> Result<(), ModelManagerError> {
    match fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ModelManagerError::Io(error)),
    }
}

async fn remove_file_if_present(path: &Path) -> Result<(), ModelManagerError> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ModelManagerError::Io(error)),
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ResumeState {
    download_url: String,
    size_bytes: u64,
    sha256: String,
    etag: Option<String>,
}

#[derive(Debug)]
struct ResumeInfo {
    downloaded_bytes: u64,
    etag: Option<String>,
}

async fn resume_offset(
    partial: &Path,
    resume: &Path,
    entry: &ModelManifestEntry,
) -> Result<ResumeInfo, ModelManagerError> {
    if !partial.is_file() || !resume.is_file() {
        return Ok(ResumeInfo {
            downloaded_bytes: 0,
            etag: None,
        });
    }
    let state: ResumeState = serde_json::from_slice(&fs::read(resume).await?)
        .map_err(|error| ModelManagerError::InvalidResume(error.to_string()))?;
    if state.download_url != entry.download_url
        || state.size_bytes != entry.size_bytes
        || state.sha256 != entry.sha256
    {
        fs::remove_file(partial).await?;
        let _ = fs::remove_file(resume).await;
        return Ok(ResumeInfo {
            downloaded_bytes: 0,
            etag: None,
        });
    }
    let downloaded_bytes = fs::metadata(partial).await?.len();
    if downloaded_bytes > entry.size_bytes {
        return Err(ModelManagerError::SizeMismatch {
            expected: entry.size_bytes,
            actual: downloaded_bytes,
        });
    }
    Ok(ResumeInfo {
        downloaded_bytes,
        etag: state.etag,
    })
}

async fn write_resume_state(
    path: &Path,
    entry: &ModelManifestEntry,
    etag: Option<String>,
) -> Result<(), ModelManagerError> {
    let state = ResumeState {
        download_url: entry.download_url.clone(),
        size_bytes: entry.size_bytes,
        sha256: entry.sha256.clone(),
        etag,
    };
    fs::write(path, serde_json::to_vec(&state)?).await?;
    Ok(())
}

async fn verify_file_hash(path: &Path, expected: &str) -> Result<(), ModelManagerError> {
    let mut file = fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hex::encode(hasher.finalize());
    if actual != expected.to_ascii_lowercase() {
        return Err(ModelManagerError::HashMismatch {
            expected: expected.to_owned(),
            actual,
        });
    }
    Ok(())
}

fn installed(entry: &ModelManifestEntry, path: PathBuf) -> InstalledModel {
    InstalledModel {
        model_id: ModelId::parse(entry.id.clone()).expect("validated manifest has a model id"),
        path,
        size_bytes: entry.size_bytes,
        sha256: entry.sha256.clone(),
        backend: entry.backend.as_str().to_owned(),
        quantization: entry.quantization.clone(),
    }
}

fn download_key(entry: &ModelManifestEntry) -> String {
    let mut hasher = Sha256::new();
    hasher.update(entry.id.as_bytes());
    hex::encode(hasher.finalize())
}

fn canonical_manifest_bytes(manifest: &ModelManifest) -> Result<Vec<u8>, ModelManagerError> {
    serde_jcs::to_vec(manifest).map_err(|error| {
        ModelManagerError::InvalidManifest(format!("cannot canonicalize manifest: {error}"))
    })
}

fn validate_manifest(manifest: &ModelManifest) -> Result<(), ModelManagerError> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION
        || manifest.manifest_version.trim().is_empty()
        || manifest.models.is_empty()
    {
        return Err(ModelManagerError::InvalidManifest(
            "unsupported schema or empty model list".to_owned(),
        ));
    }
    let mut ids = std::collections::BTreeSet::new();
    for model in &manifest.models {
        model.validate()?;
        if !ids.insert(&model.id) {
            return Err(ModelManagerError::InvalidManifest(format!(
                "duplicate model id {}",
                model.id
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ModelManagerError {
    #[error("model manifest is invalid: {0}")]
    InvalidManifest(String),
    #[error("model manifest public key is invalid")]
    InvalidPublicKey,
    #[error("model manifest signature is invalid")]
    InvalidSignature,
    #[error("model manifest trust scope mismatch: expected {expected}, got {actual}")]
    UnexpectedTrustScope {
        expected: &'static str,
        actual: &'static str,
    },
    #[error("release signing key is invalid")]
    InvalidSigningKey,
    #[error("model {0} is not in the verified manifest")]
    UnknownModel(String),
    #[error("model is not installed: {0}")]
    NotInstalled(String),
    #[error("model download failed: {0}")]
    Download(reqwest::Error),
    #[error("model download returned HTTP {0}")]
    DownloadStatus(StatusCode),
    #[error("cannot create HTTP client: {0}")]
    HttpClient(reqwest::Error),
    #[error("model download was cancelled")]
    Cancelled,
    #[error("model size mismatch: expected {expected} bytes, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("model size overflow")]
    SizeOverflow,
    #[error("model SHA-256 mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("model resume metadata is invalid: {0}")]
    InvalidResume(String),
    #[error("model destination already exists: {0}")]
    DestinationExists(String),
    #[error("multiple interrupted legacy installations need attention for model {0}")]
    AmbiguousLegacyStaging(String),
    #[error("model ID cannot be represented safely below the configured model root: {0}")]
    UnsafeModelStoragePath(String),
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: [u8; 32] = [7; 32];

    fn manifest() -> ModelManifest {
        ModelManifest {
            schema_version: 1,
            manifest_version: "test-1".to_owned(),
            trust_scope: ManifestTrustScope::Alpha,
            models: vec![ModelManifestEntry {
                id: "whisper.cpp/tiny".to_owned(),
                display_name: "Tiny".to_owned(),
                preset: ModelPreset::Fast,
                revision: "immutable".to_owned(),
                download_url: "https://example.test/tiny.bin".to_owned(),
                file_name: "tiny.bin".to_owned(),
                size_bytes: 4,
                sha256: "9f64a747e1b97f131fabb6b447296c9b6f0201e79fb3c5356e6c77e89b6a806a"
                    .to_owned(),
                license: "MIT".to_owned(),
                license_url: "https://example.test/license".to_owned(),
                source_url: "https://example.test/source".to_owned(),
                backend: ModelBackend::Cpu,
                quantization: "F16".to_owned(),
                estimated_ram_bytes: 8,
            }],
        }
    }

    fn verifier() -> ManifestVerifier {
        let public = SigningKey::from_bytes(&SECRET).verifying_key();
        ManifestVerifier::from_base64(&BASE64.encode(public.to_bytes())).expect("key")
    }

    #[test]
    fn valid_signed_manifest_verifies_and_tampering_fails() {
        let bytes = serde_json::to_vec(&manifest()).expect("JSON");
        let signature =
            sign_manifest_for_release(&bytes, &BASE64.encode(SECRET)).expect("signature");
        assert!(verifier().verify(&bytes, &signature).is_ok());
        let mut tampered = bytes;
        let index = tampered
            .iter()
            .position(|byte| *byte == b'T')
            .expect("Tiny byte");
        tampered[index] = b'X';
        assert!(matches!(
            verifier().verify(&tampered, &signature),
            Err(ModelManagerError::InvalidSignature)
        ));
        assert_eq!(
            public_key_for_release(&BASE64.encode(SECRET)).expect("public key"),
            BASE64.encode(SigningKey::from_bytes(&SECRET).verifying_key().to_bytes())
        );
    }

    #[test]
    fn valid_signature_cannot_cross_the_manifest_trust_scope() {
        let bytes = serde_json::to_vec(&manifest()).expect("JSON");
        let signature =
            sign_manifest_for_release(&bytes, &BASE64.encode(SECRET)).expect("signature");

        assert!(
            verifier()
                .verify_for_scope(&bytes, &signature, ManifestTrustScope::Alpha)
                .is_ok()
        );
        assert!(matches!(
            verifier().verify_for_scope(&bytes, &signature, ManifestTrustScope::Stable),
            Err(ModelManagerError::UnexpectedTrustScope {
                expected: "stable",
                actual: "alpha"
            })
        ));
    }

    #[tokio::test]
    async fn a_corrupt_existing_model_is_never_reported_as_installed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let manager =
            ModelManager::new(directory.path().to_path_buf(), verifier()).expect("manager");
        let verified = VerifiedManifest(manifest());
        let entry = verified.manifest().models.first().expect("entry");
        let path = manager.model_path(entry).expect("safe model path");
        fs::create_dir_all(path.parent().expect("parent"))
            .await
            .expect("directory");
        fs::write(&path, [0_u8; 4]).await.expect("file");
        let token = CancellationToken::new();
        assert!(matches!(
            manager
                .install(&verified, &entry.model_id().expect("id"), &token, |_| {})
                .await,
            Err(ModelManagerError::HashMismatch { .. })
        ));
    }

    #[test]
    fn manifest_rejects_non_https_downloads_and_unknown_backends() {
        let mut insecure = manifest();
        insecure.models[0].download_url = "http://example.test/tiny.bin".to_owned();
        let insecure_bytes = serde_json::to_vec(&insecure).expect("JSON");
        // Sign the malformed data directly to prove verification rejects the
        // schema before it considers an otherwise valid detached signature.
        let insecure_signature = BASE64.encode(
            SigningKey::from_bytes(&SECRET)
                .sign(&canonical_manifest_bytes(&insecure).expect("canonical JSON"))
                .to_bytes(),
        );
        assert!(matches!(
            verifier().verify(&insecure_bytes, &insecure_signature),
            Err(ModelManagerError::InvalidManifest(_))
        ));

        let mut unsupported = manifest();
        unsupported.models[0].backend = ModelBackend::Cuda;
        let bytes = serde_json::to_vec(&unsupported).expect("JSON");
        let signature = sign_manifest_for_release(&bytes, &BASE64.encode(SECRET))
            .expect_err("invalid manifest must not be signable");
        assert!(matches!(signature, ModelManagerError::InvalidManifest(_)));
    }

    #[tokio::test]
    async fn deletion_requires_the_explicit_confirmation_type() {
        let directory = tempfile::tempdir().expect("tempdir");
        let manager =
            ModelManager::new(directory.path().to_path_buf(), verifier()).expect("manager");
        let id = ModelId::parse("whisper.cpp/tiny").expect("id");
        fs::create_dir_all(directory.path().join(id.as_str()))
            .await
            .expect("model directory");
        manager
            .delete(&id, DeleteConfirmation::ConfirmedByUser)
            .await
            .expect("delete");
        assert!(!directory.path().join(id.as_str()).exists());
    }

    #[tokio::test]
    async fn storage_never_interprets_model_ids_as_parent_paths() {
        let directory = tempfile::tempdir().expect("tempdir");
        let manager =
            ModelManager::new(directory.path().to_path_buf(), verifier()).expect("manager");
        let unsafe_id = ModelId::parse("whisper.cpp/../outside").expect("opaque domain id");

        assert!(matches!(
            manager
                .delete(&unsafe_id, DeleteConfirmation::ConfirmedByUser)
                .await,
            Err(ModelManagerError::UnsafeModelStoragePath(_))
        ));
        assert!(
            !directory
                .path()
                .parent()
                .expect("parent")
                .join("outside")
                .exists()
        );
    }

    #[tokio::test]
    async fn cancellation_is_checked_before_a_network_request() {
        let directory = tempfile::tempdir().expect("tempdir");
        let manager =
            ModelManager::new(directory.path().to_path_buf(), verifier()).expect("manager");
        let verified = VerifiedManifest(manifest());
        let token = CancellationToken::new();
        token.cancel();
        assert!(matches!(
            manager
                .install(
                    &verified,
                    &verified.manifest().models[0].model_id().expect("id"),
                    &token,
                    |_| {}
                )
                .await,
            Err(ModelManagerError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn matching_resume_state_preserves_offset_and_etag() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut sample = manifest();
        let entry = sample.models.remove(0);
        let partial = directory.path().join("model.partial");
        let resume = directory.path().join("model.resume.json");
        fs::write(&partial, [1_u8, 2]).await.expect("partial");
        write_resume_state(&resume, &entry, Some("\"immutable-etag\"".to_owned()))
            .await
            .expect("resume state");

        let info = resume_offset(&partial, &resume, &entry)
            .await
            .expect("resume info");
        assert_eq!(info.downloaded_bytes, 2);
        assert_eq!(info.etag.as_deref(), Some("\"immutable-etag\""));
    }

    #[tokio::test]
    async fn install_activates_a_namespaced_model_id_without_leaving_staging_data() {
        let directory = tempfile::tempdir().expect("tempdir");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("local address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("connection");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.expect("request");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n\x01\x02\x03\x04",
                )
                .await
                .expect("response");
        });

        let mut sample = manifest();
        sample.models[0].download_url = format!("http://{address}/tiny.bin");
        let manager =
            ModelManager::new(directory.path().to_path_buf(), verifier()).expect("manager");
        let cancellation = CancellationToken::new();

        let installed = manager
            .install(
                &VerifiedManifest(sample),
                &ModelId::parse("whisper.cpp/tiny").expect("id"),
                &cancellation,
                |_| {},
            )
            .await
            .expect("installation");

        let expected = directory
            .path()
            .join("whisper.cpp")
            .join("tiny")
            .join("tiny.bin");
        assert_eq!(installed.path, expected);
        assert_eq!(
            fs::read(&installed.path).await.expect("installed model"),
            [1, 2, 3, 4]
        );
        assert!(
            !directory
                .path()
                .join(".downloads")
                .join(format!("{}.partial", download_key(&manifest().models[0])))
                .exists()
        );
        assert!(
            std::fs::read_dir(directory.path().join("whisper.cpp"))
                .expect("model namespace")
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().contains(".staging"))
        );
    }

    #[tokio::test]
    async fn install_recovers_the_complete_partial_from_the_affected_alpha_staging_layout() {
        let directory = tempfile::tempdir().expect("tempdir");
        let manager =
            ModelManager::new(directory.path().to_path_buf(), verifier()).expect("manager");
        let sample = manifest();
        let entry = sample.models.first().expect("entry");
        let partial = manager.partial_path(entry);
        let resume = manager.resume_path(entry);
        fs::create_dir_all(manager.download_directory())
            .await
            .expect("download directory");
        write_resume_state(&resume, entry, Some("\"fixture\"".to_owned()))
            .await
            .expect("resume state");
        let legacy = directory
            .path()
            .join(".whisper.cpp")
            .join("tiny-00000000-0000-0000-0000-000000000000.staging");
        fs::create_dir_all(&legacy).await.expect("legacy staging");
        fs::write(legacy.join("tiny.bin"), [1_u8, 2, 3, 4])
            .await
            .expect("legacy model");

        let installed = manager
            .install(
                &VerifiedManifest(sample),
                &ModelId::parse("whisper.cpp/tiny").expect("id"),
                &CancellationToken::new(),
                |_| {},
            )
            .await
            .expect("recovered installation");

        assert!(installed.path.is_file());
        assert!(!legacy.exists());
        assert!(!partial.exists());
        assert!(!resume.exists());
    }

    #[tokio::test]
    async fn activation_never_leaves_a_staging_directory_on_destination_race() {
        let directory = tempfile::tempdir().expect("tempdir");
        let manager =
            ModelManager::new(directory.path().to_path_buf(), verifier()).expect("manager");
        let mut sample = manifest();
        let entry = sample.models.remove(0);
        let partial = directory.path().join("download.partial");
        let resume = directory.path().join("download.resume.json");
        fs::write(&partial, [1_u8, 2, 3, 4]).await.expect("partial");
        fs::create_dir_all(directory.path().join(&entry.id))
            .await
            .expect("existing destination");

        assert!(matches!(
            manager.activate(&entry, &partial, &resume).await,
            Err(ModelManagerError::DestinationExists(_))
        ));
        let leftovers = std::fs::read_dir(directory.path())
            .expect("directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".staging"))
            .count();
        assert_eq!(leftovers, 0);
        assert!(
            partial.is_file(),
            "a destination race must keep the resumable partial file"
        );
    }
}
