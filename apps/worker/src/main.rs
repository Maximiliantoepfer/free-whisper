#![forbid(unsafe_code)]

use std::{net::SocketAddr, path::PathBuf, process::ExitCode, time::Duration};

use free_whisper_protocol::ExecutionBackendV1;
use free_whisper_worker::{WhisperCppEngine, WhisperCppEngineConfig, WorkerState, serve};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("free-whisper-worker failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let config = CliConfig::parse(std::env::args().skip(1).collect())?;
    let token = read_token_from_stdin().await?;
    let engine = WhisperCppEngine::new(WhisperCppEngineConfig {
        executable: config.whisper_server,
        model_path: config.model,
        model_id: config.model_id,
        backend: config.backend,
        startup_timeout: Duration::from_secs(config.startup_timeout_seconds),
    })
    .map_err(|error| error.to_string())?;
    let listener = TcpListener::bind(config.bind)
        .await
        .map_err(|error| format!("cannot bind worker listener: {error}"))?;
    let bound = listener
        .local_addr()
        .map_err(|error| format!("cannot inspect worker listener: {error}"))?;
    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(format!("FREE_WHISPER_BOUND {bound}\n").as_bytes())
        .await
        .map_err(|error| format!("cannot report worker listener: {error}"))?;
    stdout
        .flush()
        .await
        .map_err(|error| format!("cannot flush worker listener report: {error}"))?;
    serve(
        listener,
        WorkerState::new(std::sync::Arc::new(engine), token),
    )
    .await
    .map_err(|error| error.to_string())
}

#[derive(Debug)]
struct CliConfig {
    bind: SocketAddr,
    whisper_server: PathBuf,
    model: PathBuf,
    model_id: String,
    backend: ExecutionBackendV1,
    startup_timeout_seconds: u64,
}

impl CliConfig {
    fn parse(arguments: Vec<String>) -> Result<Self, String> {
        let mut values = arguments.into_iter();
        let mut bind = "127.0.0.1:8787"
            .parse::<SocketAddr>()
            .map_err(|error| format!("invalid default bind address: {error}"))?;
        let mut whisper_server = None;
        let mut model = None;
        let mut model_id = None;
        let mut backend = ExecutionBackendV1::Cpu;
        let mut startup_timeout_seconds = 30;
        let mut token_stdin = false;
        while let Some(argument) = values.next() {
            let value = |flag: &str, values: &mut std::vec::IntoIter<String>| {
                values
                    .next()
                    .ok_or_else(|| format!("{flag} requires a value"))
            };
            match argument.as_str() {
                "--bind" => {
                    bind = value("--bind", &mut values)?
                        .parse()
                        .map_err(|_| "--bind must be a socket address".to_owned())?;
                    if !bind.ip().is_loopback() {
                        return Err("the worker only accepts loopback HTTP; use a TLS reverse proxy for remote access".to_owned());
                    }
                }
                "--whisper-server" => {
                    whisper_server = Some(PathBuf::from(value("--whisper-server", &mut values)?))
                }
                "--model" => model = Some(PathBuf::from(value("--model", &mut values)?)),
                "--model-id" => model_id = Some(value("--model-id", &mut values)?),
                "--backend" => {
                    backend = parse_backend(&value("--backend", &mut values)?)?;
                }
                "--startup-timeout-seconds" => {
                    startup_timeout_seconds = value("--startup-timeout-seconds", &mut values)?
                        .parse()
                        .map_err(|_| "--startup-timeout-seconds must be an integer".to_owned())?;
                    if startup_timeout_seconds == 0 || startup_timeout_seconds > 300 {
                        return Err(
                            "--startup-timeout-seconds must be between 1 and 300".to_owned()
                        );
                    }
                }
                "--token-stdin" => token_stdin = true,
                "--help" => return Err(usage()),
                _ => return Err(format!("unknown argument {argument}\n{}", usage())),
            }
        }
        if !token_stdin {
            return Err(
                "--token-stdin is required; command-line and environment tokens are refused"
                    .to_owned(),
            );
        }
        Ok(Self {
            bind,
            whisper_server: whisper_server
                .ok_or_else(|| "--whisper-server is required".to_owned())?,
            model: model.ok_or_else(|| "--model is required".to_owned())?,
            model_id: model_id.ok_or_else(|| "--model-id is required".to_owned())?,
            backend,
            startup_timeout_seconds,
        })
    }
}

fn parse_backend(value: &str) -> Result<ExecutionBackendV1, String> {
    match value {
        "cpu" => Ok(ExecutionBackendV1::Cpu),
        "cuda" => Ok(ExecutionBackendV1::Cuda),
        "vulkan" => Ok(ExecutionBackendV1::Vulkan),
        "open_vino" => Ok(ExecutionBackendV1::OpenVino),
        _ => Err("--backend must be cpu, cuda, vulkan or open_vino".to_owned()),
    }
}

async fn read_token_from_stdin() -> Result<String, String> {
    let mut line = String::new();
    let bytes = BufReader::new(tokio::io::stdin())
        .read_line(&mut line)
        .await
        .map_err(|error| format!("cannot read bearer token from stdin: {error}"))?;
    let token = line.trim_end_matches(['\r', '\n']).to_owned();
    if bytes == 0 || token.len() < 16 || token.len() > 512 {
        return Err("stdin must provide a bearer token between 16 and 512 characters".to_owned());
    }
    Ok(token)
}

fn usage() -> String {
    "usage: free-whisper-worker --token-stdin --whisper-server PATH --model PATH --model-id ID [--bind 127.0.0.1:PORT] [--backend cpu]".to_owned()
}
