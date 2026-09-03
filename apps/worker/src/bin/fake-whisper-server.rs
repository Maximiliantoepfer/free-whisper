#![forbid(unsafe_code)]

//! Test-only stand-in for the pinned whisper.cpp HTTP server. It deliberately
//! accepts the command-line subset used by `WhisperCppEngine` and emits a
//! v1.8.6-shaped verbose JSON response after a short delay.

use std::{net::SocketAddr, process::ExitCode, time::Duration};

use axum::{
    Json, Router,
    extract::Multipart,
    http::StatusCode,
    routing::{get, post},
};
use serde_json::json;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fake-whisper-server failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let port = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|pair| pair[0] == "--port")
        .map(|pair| pair[1].parse::<u16>())
        .ok_or_else(|| "--port is required".to_owned())?
        .map_err(|_| "--port must be a u16".to_owned())?;
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .map_err(|error| format!("cannot bind fake server: {error}"))?;
    tokio::select! {
        result = axum::serve(
            listener,
            Router::new()
                .route("/health", get(|| async { StatusCode::OK }))
                .route("/inference", post(inference)),
        ) => result.map_err(|error| format!("fake server stopped: {error}")),
        // The real worker owns process shutdown. The fixture also has a short
        // deterministic lifetime so a forcibly killed outer test worker cannot
        // leave a Windows child process holding its executable open.
        _ = tokio::time::sleep(Duration::from_secs(2)) => Ok(()),
    }
}

async fn inference(
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, (StatusCode, &'static str)> {
    let mut language = None;
    let mut translate = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "malformed multipart"))?
    {
        match field.name().unwrap_or_default() {
            "language" => {
                language = Some(
                    field
                        .text()
                        .await
                        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid language field"))?,
                );
            }
            "translate" => {
                translate = Some(
                    field
                        .text()
                        .await
                        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid translate field"))?,
                );
            }
            _ => {}
        }
    }
    // This fixture enforces the pinned contract on the actual inner HTTP
    // request. Without these fields whisper.cpp would fall back to English.
    if language.as_deref() != Some("auto") || translate.as_deref() != Some("false") {
        return Err((StatusCode::BAD_REQUEST, "missing transcription controls"));
    }
    // Leaves enough time for the outer worker's busy and cancellation paths.
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(Json(json!({
        "text": "prozess-test",
        "language": "german",
        "detected_language": "german",
        "detected_language_probability": 0.998,
        "segments": [{ "id": 0, "text": "prozess-test", "start": 0.0, "end": 0.5 }]
    })))
}
