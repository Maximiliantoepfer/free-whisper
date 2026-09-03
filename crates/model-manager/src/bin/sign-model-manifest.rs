#![forbid(unsafe_code)]

use std::{env, fs, process::ExitCode};

use free_whisper_model_manager::{
    ManifestTrustScope, ModelManifest, public_key_for_release, sign_manifest_for_release,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("model manifest signing failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let key = env::var("FREE_WHISPER_MANIFEST_SIGNING_KEY")
        .map_err(|_| "FREE_WHISPER_MANIFEST_SIGNING_KEY is required".to_owned())?;
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.len() < 2 {
        return Err("usage: sign-model-manifest PATH-TO-manifest.json PATH-TO-manifest.sig [PATH-TO-manifest.public-key] [--scope alpha|stable]".to_owned());
    }
    let manifest = &arguments[0];
    let signature = &arguments[1];
    let mut public_key_path = None;
    let mut scope = None;
    let mut index = 2;
    while index < arguments.len() {
        if arguments[index] == "--scope" {
            let value = arguments
                .get(index + 1)
                .ok_or_else(|| "--scope requires alpha or stable".to_owned())?;
            scope = Some(parse_scope(value)?);
            index += 2;
        } else if public_key_path.is_none() {
            public_key_path = Some(arguments[index].as_str());
            index += 1;
        } else {
            return Err("unexpected signing argument".to_owned());
        }
    }

    let mut bytes = fs::read(manifest).map_err(|error| format!("cannot read manifest: {error}"))?;
    if let Some(scope) = scope {
        let mut parsed: ModelManifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("cannot parse manifest: {error}"))?;
        parsed.trust_scope = scope;
        bytes = serde_json::to_vec_pretty(&parsed)
            .map_err(|error| format!("cannot serialize manifest: {error}"))?;
        bytes.push(b'\n');
        fs::write(manifest, &bytes)
            .map_err(|error| format!("cannot write manifest scope: {error}"))?;
    }
    let signed = sign_manifest_for_release(&bytes, &key).map_err(|error| error.to_string())?;
    fs::write(signature, format!("{signed}\n"))
        .map_err(|error| format!("cannot write detached signature: {error}"))?;
    if let Some(public_key_path) = public_key_path {
        let public_key = public_key_for_release(&key).map_err(|error| error.to_string())?;
        fs::write(public_key_path, format!("{public_key}\n"))
            .map_err(|error| format!("cannot write public key: {error}"))?;
    }
    Ok(())
}

fn parse_scope(value: &str) -> Result<ManifestTrustScope, String> {
    match value {
        "alpha" => Ok(ManifestTrustScope::Alpha),
        "stable" => Ok(ManifestTrustScope::Stable),
        _ => Err("--scope must be alpha or stable".to_owned()),
    }
}
