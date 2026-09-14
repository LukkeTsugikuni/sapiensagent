use crate::policy::Policy;
use anyhow::{Context, Result, bail};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub origin: String,
    pub entrypoint: String,
    pub permissions: Vec<String>,
    pub sha256: String,
    #[serde(default)]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginVerification {
    pub name: String,
    pub version: String,
    pub origin: String,
    pub artifact: PathBuf,
    pub sha256: String,
    pub hash_matches: bool,
    pub signature_present: bool,
    pub signature_verified: bool,
    pub origin_trusted: bool,
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginInstallation {
    pub name: String,
    pub version: String,
    pub path: PathBuf,
    pub updated: bool,
    pub enabled: bool,
    pub rollback_backup: Option<PathBuf>,
}

pub fn load_manifest(path: &Path) -> Result<PluginManifest> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read manifest {}", path.display()))?;
    let manifest = match path.extension().and_then(|extension| extension.to_str()) {
        Some("json") => serde_json::from_str(&text)
            .with_context(|| format!("parse JSON manifest {}", path.display()))?,
        _ => toml::from_str(&text)
            .with_context(|| format!("parse TOML manifest {}", path.display()))?,
    };
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub fn verify(path: &Path, policy: &Policy) -> Result<PluginVerification> {
    policy.check_path(path)?;
    let manifest = load_manifest(path)?;
    let parent = path
        .parent()
        .context("plugin manifest has no parent directory")?;
    let artifact = parent.join(&manifest.entrypoint);
    policy.check_path(&artifact)?;
    if !artifact.is_file() {
        bail!("plugin entrypoint is not a file");
    }
    let bytes = fs::read(&artifact)?;
    let digest = hex_digest(&bytes);
    let hash_matches = digest.eq_ignore_ascii_case(&manifest.sha256);
    if !hash_matches {
        bail!(
            "plugin hash mismatch for {}; expected {}, got {}",
            manifest.name,
            manifest.sha256,
            digest
        );
    }
    let signature_verified = if let Some(signature) = manifest.signature.as_deref() {
        let key = std::env::var("SAPIENS_PLUGIN_SIGNING_KEY")
            .context("plugin signature present but SAPIENS_PLUGIN_SIGNING_KEY is unavailable")?;
        verify_signature(signature, &bytes, &key)?
    } else {
        false
    };
    let origin_trusted = trusted_origin(&manifest.origin);
    Ok(PluginVerification {
        name: manifest.name,
        version: manifest.version,
        origin: manifest.origin,
        artifact,
        sha256: digest,
        hash_matches,
        signature_present: manifest.signature.is_some(),
        signature_verified,
        origin_trusted,
        executable: false,
    })
}

pub fn remove(path: &Path, policy: &Policy) -> Result<()> {
    policy.check_path(path)?;
    let candidate = fs::canonicalize(path)?;
    let workspace = fs::canonicalize(&policy.workspace)?;
    if candidate == workspace {
        bail!("refusing to remove the workspace root");
    }
    if candidate.is_dir() {
        fs::remove_dir_all(candidate)?;
    } else {
        fs::remove_file(candidate)?;
    }
    Ok(())
}

pub fn install(
    manifest_path: &Path,
    destination_root: &Path,
    policy: &Policy,
    update: bool,
) -> Result<PluginInstallation> {
    policy.check_path(manifest_path)?;
    let manifest = load_manifest(manifest_path)?;
    let verification = verify(manifest_path, policy)?;
    if !verification.origin_trusted {
        bail!("plugin origin is not trusted; installation refused");
    }
    let parent = destination_root
        .parent()
        .context("plugin destination has no parent")?;
    policy.check_path(parent)?;
    fs::create_dir_all(destination_root)?;
    policy.check_path(destination_root)?;
    // Keep one active installation per plugin name. This lets `update` replace
    // an older version atomically instead of creating a second active copy.
    let target = destination_root.join(&manifest.name);
    policy.check_path(&target)?;
    let updated = target.exists();
    if updated && !update {
        bail!("plugin version already installed; use plugin update");
    }
    let staging = destination_root.join(format!(
        ".staging-{}-{}-{}",
        manifest.name,
        std::process::id(),
        unix_now()
    ));
    policy.check_path(&staging)?;
    fs::create_dir_all(&staging)?;
    let result = (|| -> Result<PluginInstallation> {
        let artifact_relative = Path::new(&manifest.entrypoint);
        if artifact_relative.is_absolute()
            || artifact_relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            bail!("plugin entrypoint must be a relative path without '..'");
        }
        let staged_artifact = staging.join(artifact_relative);
        if let Some(parent) = staged_artifact.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&verification.artifact, &staged_artifact)?;
        let manifest_name = manifest_path
            .file_name()
            .context("plugin manifest has no filename")?;
        fs::copy(manifest_path, staging.join(manifest_name))?;
        fs::write(staging.join("DISABLED"), b"sandbox-required\n")?;
        let backup = if updated {
            let backup = destination_root.join(format!(
                ".{}.previous-{}",
                target
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("plugin"),
                unix_now()
            ));
            fs::rename(&target, &backup)?;
            Some(backup)
        } else {
            None
        };
        if let Err(error) = fs::rename(&staging, &target) {
            if let Some(backup) = &backup {
                let _ = fs::rename(backup, &target);
            }
            return Err(error.into());
        }
        Ok(PluginInstallation {
            name: manifest.name,
            version: manifest.version,
            path: target,
            updated,
            enabled: false,
            rollback_backup: backup,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

pub fn list(destination_root: &Path, policy: &Policy) -> Result<Vec<PathBuf>> {
    let parent = destination_root
        .parent()
        .context("plugin destination has no parent")?;
    policy.check_path(parent)?;
    if !destination_root.exists() {
        return Ok(Vec::new());
    }
    policy.check_path(destination_root)?;
    let mut entries = fs::read_dir(destination_root)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            (path.is_dir() && !name.starts_with('.')).then_some(path)
        })
        .collect::<Vec<_>>();
    entries.sort();
    Ok(entries)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn validate_manifest(manifest: &PluginManifest) -> Result<()> {
    if manifest.name.trim().is_empty()
        || manifest
            .name
            .chars()
            .any(|character| !(character.is_ascii_alphanumeric() || "-_.".contains(character)))
    {
        bail!("plugin name is invalid");
    }
    if manifest.version.trim().is_empty() {
        bail!("plugin version is required");
    }
    if manifest.origin.trim().is_empty() {
        bail!("plugin origin is required");
    }
    if manifest.entrypoint.trim().is_empty() || manifest.sha256.len() != 64 {
        bail!("plugin entrypoint and 64-character sha256 are required");
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn verify_signature(signature: &str, artifact: &[u8], key: &str) -> Result<bool> {
    let expected = decode_hex(signature).context("plugin signature must be hexadecimal")?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).context("plugin signing key is invalid")?;
    mac.update(artifact);
    mac.verify_slice(&expected)
        .map(|_| true)
        .map_err(|_| anyhow::anyhow!("plugin signature verification failed"))
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        bail!("hex value must have an even number of characters");
    }
    value
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair)?;
            u8::from_str_radix(text, 16).context("invalid hexadecimal digit")
        })
        .collect()
}

fn trusted_origin(origin: &str) -> bool {
    origin.starts_with("https://") || origin.starts_with("local://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn verifies_an_artifact_hash_without_enabling_execution() {
        let root = std::env::temp_dir().join(format!(
            "sapiens-plugin-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("mkdir");
        let artifact = root.join("plugin.wasm");
        fs::write(&artifact, b"wasm-placeholder").expect("artifact");
        let hash = hex_digest(b"wasm-placeholder");
        let manifest = root.join("plugin.toml");
        fs::write(
            &manifest,
            format!(
                "name = \"demo\"\nversion = \"1.0.0\"\norigin = \"local-test\"\nentrypoint = \"plugin.wasm\"\npermissions = [\"read\"]\nsha256 = \"{hash}\"\n"
            ),
        )
        .expect("manifest");
        let verification = verify(&manifest, &Policy::supervised(root.clone())).expect("verify");
        assert!(verification.hash_matches);
        assert!(!verification.executable);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_hash_is_rejected() {
        let root = std::env::temp_dir().join(format!("sapiens-plugin-bad-{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        let artifact = root.join("plugin.wasm");
        fs::write(&artifact, b"wrong").expect("artifact");
        let manifest = root.join("plugin.toml");
        fs::write(
            &manifest,
            "name = \"demo\"\nversion = \"1.0.0\"\norigin = \"test\"\nentrypoint = \"plugin.wasm\"\npermissions = []\nsha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"\n",
        )
        .expect("manifest");
        assert!(verify(&manifest, &Policy::supervised(root.clone())).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verifies_hmac_signature_without_mutating_process_environment() {
        let key = "test-signing-key";
        let artifact = b"signed-artifact";
        let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("mac");
        mac.update(artifact);
        let signature = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert!(verify_signature(&signature, artifact, key).expect("signature"));
        assert!(verify_signature(&signature, artifact, "wrong-key").is_err());
        assert!(trusted_origin("https://example.com/plugin"));
        assert!(!trusted_origin("unknown-origin"));
    }

    fn write_manifest(root: &Path, version: &str, artifact: &[u8]) -> PathBuf {
        fs::create_dir_all(root).expect("mkdir");
        let artifact_path = root.join("plugin.wasm");
        fs::write(&artifact_path, artifact).expect("artifact");
        let manifest = root.join(format!("plugin-{version}.toml"));
        fs::write(
            &manifest,
            format!(
                "name = \"demo\"\nversion = \"{version}\"\norigin = \"local://test\"\nentrypoint = \"plugin.wasm\"\npermissions = [\"read\"]\nsha256 = \"{}\"\n",
                hex_digest(artifact)
            ),
        )
        .expect("manifest");
        manifest
    }

    #[test]
    fn installs_disabled_plugin_and_rejects_duplicate_without_update() {
        let root = std::env::temp_dir().join(format!(
            "sapiens-plugin-install-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let source = root.join("source");
        let destination = root.join("workspace").join("plugins");
        let manifest = write_manifest(&source, "1.0.0", b"v1");
        let policy = Policy::supervised(root.clone());
        let installation = install(&manifest, &destination, &policy, false).expect("install");
        assert_eq!(installation.path, destination.join("demo"));
        assert!(!installation.enabled);
        assert!(installation.path.join("DISABLED").is_file());
        assert!(install(&manifest, &destination, &policy, false).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn updates_plugin_atomically_and_keeps_previous_backup() {
        let root = std::env::temp_dir().join(format!(
            "sapiens-plugin-update-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let source = root.join("source");
        let destination = root.join("workspace").join("plugins");
        let first = write_manifest(&source, "1.0.0", b"v1");
        let policy = Policy::supervised(root.clone());
        install(&first, &destination, &policy, false).expect("first install");
        let second = write_manifest(&source, "2.0.0", b"v2");
        let installation = install(&second, &destination, &policy, true).expect("update");
        assert!(installation.updated);
        assert!(installation.rollback_backup.is_some());
        assert_eq!(
            fs::read(destination.join("demo").join("plugin.wasm")).expect("new artifact"),
            b"v2"
        );
        let backup = installation.rollback_backup.expect("backup");
        assert_eq!(
            fs::read(backup.join("plugin.wasm")).expect("old artifact"),
            b"v1"
        );
        let _ = fs::remove_dir_all(root);
    }
}
