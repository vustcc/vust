//! 证书工具：CA 证书与客户端证书签发。

use rcgen::{
    Certificate, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use std::str;

/// 内置的 Agent CA 证书 PEM 内容。
pub const AGENT_CA_CERT_PEM: &[u8] = include_bytes!("../assets/vust-ca.crt");
/// 内置的 Agent CA 私钥 PEM 内容。
pub const AGENT_CA_KEY_PEM: &[u8] = include_bytes!("../assets/vust-ca.key");

/// 签发结果，包含证书、私钥与证书链。
pub struct IssuedCert {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    pub chain_pem: Vec<u8>,
}

fn ca_cert_and_key() -> Result<(Certificate, KeyPair), rcgen::Error> {
    let ca_key = KeyPair::from_pem(
        str::from_utf8(AGENT_CA_KEY_PEM).map_err(|_| rcgen::Error::CouldNotParseKeyPair)?,
    )?;
    let ca_pem =
        str::from_utf8(AGENT_CA_CERT_PEM).map_err(|_| rcgen::Error::CouldNotParseCertificate)?;
    let ca_params = CertificateParams::from_ca_cert_pem(ca_pem)?;
    let ca_cert = ca_params.self_signed(&ca_key)?;
    Ok((ca_cert, ca_key))
}

/// 使用内置 CA 签发客户端证书。
pub fn issue_client_cert(common_name: &str) -> Result<IssuedCert, rcgen::Error> {
    issue_leaf_cert(common_name, &[], true, false)
}

/// 使用内置 CA 签发服务端证书，并支持 SAN 列表。
pub fn issue_server_cert(common_name: &str, sans: &[String]) -> Result<IssuedCert, rcgen::Error> {
    issue_leaf_cert(common_name, sans, false, true)
}

fn issue_leaf_cert(
    common_name: &str,
    sans: &[String],
    client_auth: bool,
    server_auth: bool,
) -> Result<IssuedCert, rcgen::Error> {
    let (ca_cert, ca_key) = ca_cert_and_key()?;
    let mut params = CertificateParams::new(sans.to_vec())?;

    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    params.distinguished_name = dn;
    params.is_ca = IsCa::NoCa;
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    if client_auth {
        params
            .extended_key_usages
            .push(ExtendedKeyUsagePurpose::ClientAuth);
    }
    if server_auth {
        params
            .extended_key_usages
            .push(ExtendedKeyUsagePurpose::ServerAuth);
    }

    let leaf_key = KeyPair::generate()?;
    let cert = params.signed_by(&leaf_key, &ca_cert, &ca_key)?;
    let cert_pem = cert.pem().into_bytes();
    let key_pem = leaf_key.serialize_pem().into_bytes();
    let mut chain_pem = Vec::new();
    chain_pem.extend_from_slice(&cert_pem);
    chain_pem.extend_from_slice(AGENT_CA_CERT_PEM);

    Ok(IssuedCert {
        cert_pem,
        key_pem,
        chain_pem,
    })
}
