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
    if !product_version(&version) {
        return Err(
            io::Error::other("VERSION must be a canonical X.Y.Z or X.Y.Z-rc.N version").into(),
        );
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

fn product_version(version: &str) -> bool {
    let Ok(parsed) = semver::Version::parse(version) else {
        return false;
    };
    if !parsed.build.is_empty() {
        return false;
    }
    let prerelease = parsed.pre.as_str();
    prerelease.is_empty()
        || prerelease.strip_prefix("rc.").is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::product_version;

    #[test]
    fn accepts_canonical_stable_and_rc_product_versions() {
        for value in [
            "1.0.0",
            "1.0.129",
            "2.10.123",
            "1.0.0-rc.21686",
            "1.0.0-rc.0",
        ] {
            assert!(product_version(value), "{value}");
        }
        for value in [
            "",
            "v1.0.0",
            "01.0.0",
            "1.0",
            "1.0.0.0",
            "1.0.0-rc.01",
            "1.0.0-rc.x",
            "1.0.0-rc.1.extra",
            "1.0.0-beta.1",
            "1.0.0+build",
            "1.0.0\n",
        ] {
            assert!(!product_version(value), "{value}");
        }
    }
}
