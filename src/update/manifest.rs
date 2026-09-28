//! Public release metadata. HTTPS provides transport security; SHA-256 detects corruption.
use anyhow::{Result, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;

pub const TARGET: &str = "x86_64-pc-windows-msvc";
pub const ASSET: &str = "tshell-windows-x86_64.exe";
pub const FEED: &str = "update-windows-x86_64.json";
pub const MAX_BINARY: u64 = 512 * 1024 * 1024;
pub const MAX_MANIFEST: u64 = 16 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub version: String,
    pub target: String,
    pub asset: String,
    pub size: u64,
    pub sha256: String,
}

pub fn parse(bytes: &[u8]) -> Result<Manifest> {
    ensure!(
        bytes.len() as u64 <= MAX_MANIFEST,
        "Update manifest is too large"
    );
    let manifest: Manifest = serde_json::from_slice(bytes)?;
    manifest.validate()?;
    Ok(manifest)
}

impl Manifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "Unsupported update schema");
        ensure!(
            self.target == TARGET && self.asset == ASSET,
            "Wrong update target"
        );
        let version = Version::parse(&self.version)?;
        ensure!(
            version.pre.is_empty() && version.build.is_empty(),
            "Only stable releases are supported"
        );
        ensure!(version.to_string() == self.version, "Noncanonical version");
        ensure!(
            self.size > 0 && self.size <= MAX_BINARY,
            "Invalid update size"
        );
        ensure!(
            self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "Invalid SHA-256"
        );
        Ok(())
    }

    pub fn newer_than(&self, current: &str) -> Result<bool> {
        self.validate()?;
        Ok(Version::parse(&self.version)? > Version::parse(current)?)
    }

    pub fn verify_binary(&self, reader: impl Read) -> Result<()> {
        let (size, hash) = fingerprint(reader)?;
        ensure!(
            size == self.size && hash == self.sha256,
            "Update binary does not match its manifest"
        );
        Ok(())
    }
}

pub fn fingerprint(reader: impl Read) -> Result<(u64, String)> {
    let mut reader = reader.take(MAX_BINARY + 1);
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut size = 0;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size += count as u64;
    }
    ensure!(size <= MAX_BINARY, "Update binary is too large");
    Ok((size, hex::encode(hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(version: &str, binary: &[u8]) -> Vec<u8> {
        let (size, sha256) = fingerprint(binary).unwrap();
        serde_json::to_vec(&Manifest {
            schema: 1,
            version: version.into(),
            target: TARGET.into(),
            asset: ASSET.into(),
            size,
            sha256,
        })
        .unwrap()
    }

    #[test]
    fn update_checks_version_and_binary_integrity() {
        let bytes = metadata("0.2.0", b"release");
        let manifest = parse(&bytes).unwrap();
        assert!(manifest.newer_than("0.1.0").unwrap());
        assert!(!manifest.newer_than("0.2.0").unwrap());
        assert!(!manifest.newer_than("0.3.0").unwrap());
        manifest.verify_binary(b"release".as_slice()).unwrap();
        assert!(manifest.verify_binary(b"corrupt".as_slice()).is_err());
        assert!(manifest.verify_binary(b"release extra".as_slice()).is_err());
        assert!(parse(b"{}").is_err());
        assert!(parse(&vec![b' '; MAX_MANIFEST as usize + 1]).is_err());
    }

    #[test]
    fn update_rejects_prereleases_and_wrong_targets() {
        assert!(parse(&metadata("0.2.0-beta.1", b"release")).is_err());
        let mut manifest = parse(&metadata("0.2.0", b"release")).unwrap();
        manifest.target = "aarch64-apple-darwin".into();
        assert!(manifest.validate().is_err());
        manifest.target = TARGET.into();
        manifest.asset = "../other.exe".into();
        assert!(manifest.validate().is_err());
    }
}
