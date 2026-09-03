#![cfg(feature = "test-fixture")]
#![forbid(unsafe_code)]

use std::{path::PathBuf, process::Stdio, time::Duration};

use free_whisper_protocol::{
    API_VERSION, LanguageRequestV1, TranscriptionOptionsV1, TranscriptionResponseV1,
};
use reqwest::{Client, StatusCode, multipart};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use uuid::Uuid;

const TOKEN: &str = "process-contract-token-1234";

#[tokio::test]
async fn worker_build_info_is_token_free_and_identifies_the_pinned_adapter() {
    let output = Command::new(worker_binary())
        .arg("--build-info")
        .output()
        .await
        .expect("worker build information");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("UTF-8 build information")
            .trim(),
        format!(
            "free-whisper-worker|{}|{API_VERSION}|whispercpp-v1.8.6-verbose-json-start-end",
            env!("CARGO_PKG_VERSION")
        )
    );
}

#[tokio::test]
async fn managed_worker_process_reports_readiness_enforces_busy_and_restarts_after_cancel() {
    let directory = tempfile::tempdir().expect("temporary model directory");
    let model = directory.path().join("test-model.bin");
    tokio::fs::write(&model, b"not-a-real-model")
        .await
        .expect("model placeholder only tests process wiring");
    let mut child = Command::new(worker_binary())
        .arg("--token-stdin")
        .arg("--bind")
        .arg("127.0.0.1:0")
        .arg("--whisper-server")
        .arg(fake_server_binary())
        .arg("--model")
        .arg(&model)
        .arg("--model-id")
        .arg("process-test")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("worker starts");
    let mut stdin = child.stdin.take().expect("worker stdin");
    stdin
        .write_all(format!("{TOKEN}\n").as_bytes())
        .await
        .expect("token");
    stdin.shutdown().await.expect("stdin closes");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("worker stdout"))
        .read_line(&mut line)
        .await
        .expect("readiness line");
    let address = line
        .strip_prefix("FREE_WHISPER_BOUND ")
        .expect("token-free readiness prefix")
        .trim()
        .parse::<std::net::SocketAddr>()
        .expect("loopback address");
    assert!(address.ip().is_loopback());
    assert_ne!(address.port(), 0);

    let client = Client::new();
    let base = format!("http://{address}");
    assert_eq!(
        client
            .get(format!("{base}/healthz"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .expect("health response")
            .status(),
        StatusCode::OK
    );

    let request_id = Uuid::new_v4();
    let first = tokio::spawn(send_transcription(client.clone(), base.clone(), request_id));
    tokio::time::sleep(Duration::from_millis(75)).await;
    let busy = send_transcription(client.clone(), base.clone(), Uuid::new_v4())
        .await
        .expect("busy response");
    assert_eq!(busy.status(), StatusCode::TOO_MANY_REQUESTS);

    let cancelled = client
        .post(format!("{base}/v1/transcriptions/{request_id}/cancel"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("cancel response");
    assert_eq!(cancelled.status(), StatusCode::OK);
    assert_eq!(
        cancelled
            .json::<free_whisper_protocol::CancelResponseV1>()
            .await
            .expect("cancel JSON")
            .request_id,
        request_id
    );
    assert_eq!(
        first
            .await
            .expect("transcription task")
            .expect("cancelled response")
            .status(),
        StatusCode::REQUEST_TIMEOUT
    );

    let response = send_transcription(client, base, Uuid::new_v4())
        .await
        .expect("restarted response");
    assert_eq!(response.status(), StatusCode::OK);
    let transcription = response
        .json::<TranscriptionResponseV1>()
        .await
        .expect("v1 transcription JSON");
    assert_eq!(transcription.api_version, API_VERSION);
    assert_eq!(transcription.language_detected.as_deref(), Some("german"));
    assert_eq!(transcription.language_confidence_milli, Some(998));
    assert_eq!(transcription.segments[0].end_ms, 500);

    child.kill().await.expect("outer worker stops");
    // The test-only inner process exits deterministically after two seconds;
    // wait for it so a subsequent Windows build can replace its executable.
    tokio::time::sleep(Duration::from_millis(2_100)).await;
}

async fn send_transcription(
    client: Client,
    base: String,
    request_id: Uuid,
) -> Result<reqwest::Response, reqwest::Error> {
    let options = TranscriptionOptionsV1 {
        request_id,
        language: LanguageRequestV1::Auto,
        prompt_terms: Vec::new(),
        temperature_milli: 0,
        beam_size: 5,
        word_timestamps: false,
    };
    let form = multipart::Form::new()
        .text(
            "options",
            serde_json::to_string(&options).expect("options JSON"),
        )
        .text("audio_encoding", "pcm_f32le_mono_16khz")
        .part(
            "audio",
            multipart::Part::bytes(0.0_f32.to_le_bytes().to_vec()).file_name("audio.pcm"),
        );
    client
        .post(format!("{base}/v1/transcriptions"))
        .bearer_auth(TOKEN)
        .multipart(form)
        .send()
        .await
}

fn worker_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_free-whisper-worker"))
}

fn fake_server_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-whisper-server"))
}
