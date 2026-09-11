//! 在线升级共享能力：包名、平台标识、checksum 与签名校验。

pub mod archive;
pub mod artifact_names;
pub mod checksum;
pub mod signature;
pub mod signing_key;
pub mod target_triple;

pub use archive::extract_named_file_from_tar_gz;
pub use artifact_names::{
    COMPONENT_AGENT, COMPONENT_CONTROLLER, ComponentArtifactNames, ReleasePackageNames,
    component_artifact_names, normalize_version, release_package_names,
};
pub use checksum::{compute_sha256_hex, parse_checksum_text, verify_sha256};
pub use signature::{verify_detached_signature, verify_release_signature};
pub use target_triple::{current_target_triple, normalize_target_triple};
