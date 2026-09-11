//! 在线升级制品命名规范。

use semver::Version;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::target_triple::{TargetTripleError, normalize_target_triple};

pub const COMPONENT_CONTROLLER: &str = "controller";
pub const COMPONENT_AGENT: &str = "agent";

/// 制品命名错误。
#[derive(Debug, Error)]
pub enum ArtifactNameError {
    #[error("invalid SemVer version: {0}")]
    InvalidVersion(semver::Error),
    #[error(transparent)]
    InvalidTargetTriple(#[from] TargetTripleError),
    #[error("component must be controller or agent")]
    InvalidComponent,
    #[error("artifact name does not match expected pattern")]
    Mismatch,
}

/// 完整版本包三件套名称。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleasePackageNames {
    pub version: String,
    pub target_triple: String,
    pub package: String,
    pub checksum: String,
    pub signature: String,
}

/// 包内组件制品三件套名称。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentArtifactNames {
    pub component: String,
    pub target_triple: String,
    pub artifact: String,
    pub checksum: String,
    pub signature: String,
}

pub fn normalize_version(value: &str) -> Result<String, ArtifactNameError> {
    let normalized = value.trim().trim_start_matches('v');
    Version::parse(normalized).map_err(ArtifactNameError::InvalidVersion)?;
    Ok(normalized.to_string())
}

pub fn release_package_names(
    version: &str,
    target_triple: &str,
) -> Result<ReleasePackageNames, ArtifactNameError> {
    let version = normalize_version(version)?;
    let target_triple = normalize_target_triple(target_triple)?;
    let package = format!("vust-{version}-{target_triple}.tar.gz");
    Ok(ReleasePackageNames {
        version,
        target_triple,
        checksum: format!("{package}.sha256"),
        signature: format!("{package}.sig"),
        package,
    })
}

pub fn component_artifact_names(
    component: &str,
    target_triple: &str,
) -> Result<ComponentArtifactNames, ArtifactNameError> {
    if !matches!(component, COMPONENT_CONTROLLER | COMPONENT_AGENT) {
        return Err(ArtifactNameError::InvalidComponent);
    }
    let target_triple = normalize_target_triple(target_triple)?;
    let artifact_prefix = match component {
        COMPONENT_CONTROLLER => "vust",
        COMPONENT_AGENT => "vust-agent",
        _ => return Err(ArtifactNameError::InvalidComponent),
    };
    let artifact = format!("{artifact_prefix}-{target_triple}.tar.gz");
    Ok(ComponentArtifactNames {
        component: component.to_string(),
        target_triple,
        checksum: format!("{artifact}.sha256"),
        signature: format!("{artifact}.sig"),
        artifact,
    })
}

pub fn parse_release_package_name(name: &str) -> Result<ReleasePackageNames, ArtifactNameError> {
    let Some(stem) = name
        .strip_prefix("vust-")
        .and_then(|v| v.strip_suffix(".tar.gz"))
    else {
        return Err(ArtifactNameError::Mismatch);
    };
    for target in ["linux-x86_64", "linux-aarch64"] {
        let suffix = format!("-{target}");
        if let Some(version) = stem.strip_suffix(&suffix) {
            return release_package_names(version, target);
        }
    }
    Err(ArtifactNameError::Mismatch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_names_match_spec() {
        let names = release_package_names("v0.1.0-alpha.1", "linux-x86_64").unwrap();
        assert_eq!(names.package, "vust-0.1.0-alpha.1-linux-x86_64.tar.gz");
        assert_eq!(
            names.checksum,
            "vust-0.1.0-alpha.1-linux-x86_64.tar.gz.sha256"
        );
    }

    #[test]
    fn component_names_match_spec() {
        let names = component_artifact_names("controller", "linux-x86_64").unwrap();
        assert_eq!(names.artifact, "vust-linux-x86_64.tar.gz");

        let names = component_artifact_names("agent", "linux-x86_64").unwrap();
        assert_eq!(names.artifact, "vust-agent-linux-x86_64.tar.gz");
    }
}
