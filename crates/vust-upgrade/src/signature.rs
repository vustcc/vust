//! Minisign 兼容 Ed25519 detached signature 验签。

use base64::Engine;
use ring::signature;
use thiserror::Error;

use crate::signing_key::VUST_RELEASE_PUBLIC_KEY;

/// 签名校验错误。
#[derive(Debug, Error)]
pub enum SignatureError {
    #[error("public key payload is invalid")]
    InvalidPublicKey,
    #[error("signature payload is invalid")]
    InvalidSignature,
    #[error("signature verification failed")]
    VerificationFailed,
}

/// 使用内置发布公钥验签。
pub fn verify_release_signature(
    message: &[u8],
    signature_text: &str,
) -> Result<(), SignatureError> {
    verify_detached_signature(VUST_RELEASE_PUBLIC_KEY, message, signature_text)
}

/// 验证 minisign detached signature；也兼容原始 64 字节 base64 签名。
pub fn verify_detached_signature(
    public_key_text: &str,
    message: &[u8],
    signature_text: &str,
) -> Result<(), SignatureError> {
    let public_key = parse_minisign_public_key(public_key_text)?;
    let (signature, is_prehashed) = parse_detached_signature(signature_text)?;
    let verifier = signature::UnparsedPublicKey::new(&signature::ED25519, public_key);

    let verified_message = if is_prehashed {
        let hash = blake2b_simd::Params::new().hash_length(64).hash(message);
        hash.as_bytes().to_vec()
    } else {
        message.to_vec()
    };

    verifier
        .verify(&verified_message, &signature)
        .map_err(|_| SignatureError::VerificationFailed)
}

fn parse_minisign_public_key(text: &str) -> Result<Vec<u8>, SignatureError> {
    let payload = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("untrusted comment:"))
        .ok_or(SignatureError::InvalidPublicKey)?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|_| SignatureError::InvalidPublicKey)?;
    if let Ok(decoded_text) = std::str::from_utf8(&decoded)
        && decoded_text.contains("minisign public key")
    {
        return parse_minisign_public_key(decoded_text);
    }
    if decoded.len() == 32 {
        return Ok(decoded);
    }
    if decoded.len() == 42 && (decoded.starts_with(b"Ed") || decoded.starts_with(b"ED")) {
        return Ok(decoded[10..].to_vec());
    }
    Err(SignatureError::InvalidPublicKey)
}

fn parse_detached_signature(text: &str) -> Result<(Vec<u8>, bool), SignatureError> {
    let payload = text
        .lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !line.starts_with("untrusted comment:")
                && !line.starts_with("trusted comment:")
        })
        .ok_or(SignatureError::InvalidSignature)?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|_| SignatureError::InvalidSignature)?;
    if decoded.len() == 64 {
        return Ok((decoded, false));
    }
    if decoded.len() >= 74 {
        if decoded.starts_with(b"Ed") {
            return Ok((decoded[10..74].to_vec(), false));
        }
        if decoded.starts_with(b"ED") {
            return Ok((decoded[10..74].to_vec(), true));
        }
    }
    Err(SignatureError::InvalidSignature)
}

#[cfg(test)]
mod tests {
    use super::{parse_detached_signature, parse_minisign_public_key};
    use crate::signing_key::VUST_RELEASE_PUBLIC_KEY;

    #[test]
    fn parses_embedded_public_key() {
        let key = parse_minisign_public_key(VUST_RELEASE_PUBLIC_KEY).unwrap();
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn parses_detached_signature_standard() {
        let sig_text = r#"untrusted comment: signature from minisign secret key
RUQbTVkt/M5I+7kj+WfWnNmy74ShnSV7f4ZJusny83dYaSkRx6UD4JudC5a5DyV/81ZRAgH0moPYTPrU8r2dZpf6szyVAG/Dygo=
trusted comment: timestamp:1781250890	file:vust-agent-linux-x86_64.tar.gz	hashed
t8v89DZt1BCrkApJHKFoIoz82zWLNIJlQtnY1rGrL2rp/4ana8L6Dzwk50kJN86ZsntG+oq46DAu//0tFkBBCg=="#;
        let (sig, is_prehashed) = parse_detached_signature(sig_text).unwrap();
        assert_eq!(sig.len(), 64);
        assert!(is_prehashed);
    }

    #[test]
    fn parses_detached_signature_raw_64() {
        let sig_raw = "t8v89DZt1BCrkApJHKFoIoz82zWLNIJlQtnY1rGrL2rp/4ana8L6Dzwk50kJN86ZsntG+oq46DAu//0tFkBBCg==";
        let (sig, is_prehashed) = parse_detached_signature(sig_raw).unwrap();
        assert_eq!(sig.len(), 64);
        assert!(!is_prehashed);
    }
}
