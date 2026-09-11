//! 加密工具：对敏感信息进行加解密与安全处理。

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};

/// 加解密过程中可能出现的错误类型。
#[derive(Debug)]
pub enum CryptoError {
    InvalidInput,
    EncryptFailed,
    DecryptFailed,
    RandomFailed,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            CryptoError::InvalidInput => "Invalid input",
            CryptoError::EncryptFailed => "Encrypt failed",
            CryptoError::DecryptFailed => "Decrypt failed",
            CryptoError::RandomFailed => "Random generation failed",
        };
        write!(f, "{}", message)
    }
}

impl std::error::Error for CryptoError {}

const DEFAULT_MASTER_SECRET: &str = "vust_dev_ssh_secret";

fn derive_master_key() -> [u8; 32] {
    let digest = digest::digest(&digest::SHA256, DEFAULT_MASTER_SECRET.as_bytes());
    let mut key = [0_u8; 32];
    key.copy_from_slice(digest.as_ref());
    key
}

/// 使用对称密钥加密字节数据并返回十六进制字符串。
pub fn encrypt_bytes(data: &[u8]) -> Result<String, CryptoError> {
    let master_key = derive_master_key();
    let key = UnboundKey::new(&AES_256_GCM, &master_key).map_err(|_| CryptoError::EncryptFailed)?;
    let key = LessSafeKey::new(key);
    let rng = SystemRandom::new();
    let mut nonce_bytes = [0_u8; 12];
    rng.fill(&mut nonce_bytes)
        .map_err(|_| CryptoError::RandomFailed)?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);

    let mut in_out = data.to_vec();
    key.seal_in_place_append_tag(nonce, Aad::empty(), &mut in_out)
        .map_err(|_| CryptoError::EncryptFailed)?;

    let mut blob = Vec::with_capacity(12 + in_out.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&in_out);
    Ok(hex::encode(blob))
}

/// 解密十六进制编码的密文并返回原始字节。
pub fn decrypt_bytes(data: &str) -> Result<Vec<u8>, CryptoError> {
    let raw = hex::decode(data).map_err(|_| CryptoError::InvalidInput)?;
    if raw.len() <= 12 {
        return Err(CryptoError::InvalidInput);
    }
    let (nonce_bytes, ciphertext) = raw.split_at(12);
    let nonce =
        Nonce::try_assume_unique_for_key(nonce_bytes).map_err(|_| CryptoError::InvalidInput)?;
    let master_key = derive_master_key();
    let key = UnboundKey::new(&AES_256_GCM, &master_key).map_err(|_| CryptoError::DecryptFailed)?;
    let key = LessSafeKey::new(key);
    let mut in_out = ciphertext.to_vec();
    let plain = key
        .open_in_place(nonce, Aad::empty(), &mut in_out)
        .map_err(|_| CryptoError::DecryptFailed)?;
    Ok(plain.to_vec())
}

/// 对可选字符串进行加密，保留空值与空字符串语义。
pub fn encrypt_optional(value: Option<String>) -> Result<Option<String>, CryptoError> {
    match value {
        Some(raw) if !raw.is_empty() => Ok(Some(encrypt_bytes(raw.as_bytes())?)),
        Some(_) => Ok(Some(String::new())),
        None => Ok(None),
    }
}

/// 解密可选字符串，并兼容已是明文的情况。
pub fn decrypt_optional(value: Option<String>) -> Result<Option<String>, CryptoError> {
    match value {
        Some(raw) if raw.is_empty() => Ok(Some(String::new())),
        Some(raw) => {
            let decoded = decrypt_bytes(&raw).or_else(|_| Ok(raw.into_bytes()))?;
            let text = String::from_utf8(decoded).map_err(|_| CryptoError::DecryptFailed)?;
            Ok(Some(text))
        }
        None => Ok(None),
    }
}
