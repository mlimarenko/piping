use std::env;
use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::process::Command;

const PUBLIC_KEY_ENV: &str = "PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM";

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed={PUBLIC_KEY_ENV}");
    println!("cargo:rerun-if-env-changed=VERSION");
    let is_release = env::var("PROFILE").is_ok_and(|profile| profile == "release");
    let version = match env::var("VERSION") {
        Ok(value) => value,
        Err(env::VarError::NotPresent) if !is_release => env::var("CARGO_PKG_VERSION")?,
        _ => return Err(io::Error::other("VERSION is required for a release build").into()),
    };
    if !stable_version(&version) {
        return Err(io::Error::other("VERSION must be a canonical stable X.Y.Z version").into());
    }
    println!("cargo:rustc-env=PIPING_PRODUCT_VERSION={version}");

    let manifest_directory = env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("CARGO_MANIFEST_DIR must be set by Cargo"))?;
    let validator = manifest_directory
        .join("../..")
        .join("scripts/validate-license-public-key.sh");
    println!("cargo:rerun-if-changed={}", validator.display());

    let has_public_key = env::var_os(PUBLIC_KEY_ENV).is_some_and(|value| !value.is_empty());
    if !has_public_key && !is_release {
        return Ok(());
    }
    if !has_public_key {
        return Err(
            io::Error::other(format!("{PUBLIC_KEY_ENV} is required for a release build")).into(),
        );
    }

    let status = Command::new("bash")
        .arg(&validator)
        .status()
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to run {}: {error}", validator.display()),
            )
        })?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "{} rejected the release public key with {status}",
            validator.display()
        ))
        .into());
    }

    Ok(())
}

fn stable_version(version: &str) -> bool {
    let components: Vec<_> = version.split('.').collect();
    components.len() == 3
        && components.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}

#[cfg(test)]
mod tests {
    use super::stable_version;

    #[test]
    fn accepts_only_canonical_stable_product_versions() {
        for value in ["1.0.0", "1.0.129", "2.10.123"] {
            assert!(stable_version(value), "{value}");
        }
        for value in [
            "",
            "v1.0.0",
            "01.0.0",
            "1.0",
            "1.0.0.0",
            "1.0.0-rc.1",
            "1.0.0+build",
            "1.0.0\n",
        ] {
            assert!(!stable_version(value), "{value}");
        }
    }
}
