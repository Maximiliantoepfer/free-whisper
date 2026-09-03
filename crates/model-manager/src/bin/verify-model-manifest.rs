#![forbid(unsafe_code)]

use std::{env, fs, process::ExitCode};

use free_whisper_model_manager::{ManifestTrustScope, ManifestVerifier};

fn main() -> ExitCode {
    match run() {
        Ok(scope) => {
            println!("model manifest verification passed for {}", scope.as_str());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("model manifest verification failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ManifestTrustScope, String> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.len() != 5 || arguments[3] != "--scope" {
        return Err(
            "usage: verify-model-manifest MANIFEST SIG PUBLIC-KEY --scope alpha|stable".to_owned(),
        );
    }
    let scope = match arguments.get(4) {
        Some(value) if value == "alpha" => ManifestTrustScope::Alpha,
        Some(value) if value == "stable" => ManifestTrustScope::Stable,
        _ => return Err("--scope must be alpha or stable".to_owned()),
    };
    let manifest =
        fs::read(&arguments[0]).map_err(|error| format!("cannot read manifest: {error}"))?;
    let signature = fs::read_to_string(&arguments[1])
        .map_err(|error| format!("cannot read signature: {error}"))?;
    let public_key = fs::read_to_string(&arguments[2])
        .map_err(|error| format!("cannot read public key: {error}"))?;
    ManifestVerifier::from_base64(public_key.trim())
        .map_err(|error| error.to_string())?
        .verify_for_scope(&manifest, &signature, scope)
        .map_err(|error| error.to_string())?;
    Ok(scope)
}
