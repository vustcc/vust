//! 加密工具：密钥与敏感信息的安全处理。

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};

const MASTER_KEY: [u8; 32] = [
    0x2e, 0xf7, 0x31, 0xb1, 0x2a, 0x9b, 0x4f, 0x73, 0x88, 0x42, 0x9a, 0x13, 0x7c, 0x55, 0x1d, 0x6b,
    0xb3, 0x0c, 0xe2, 0x9d, 0x7f, 0x6a, 0x0f, 0x91, 0x4b, 0x8a, 0x2c, 0x44, 0x6e, 0x1f, 0x5a, 0xc9,
];

/// 加解密过程中可能出现的错误类型。
#[derive(Debug)]
pub enum CryptoError {
    InvalidInput,
    EncryptFailed,
    DecryptFailed,
    RandomFailed,
}

/// 使用对称密钥加密字节数据并返回十六进制字符串。
pub fn encrypt_bytes(data: &[u8]) -> Result<String, CryptoError> {
    let key = UnboundKey::new(&AES_256_GCM, &MASTER_KEY).map_err(|_| CryptoError::EncryptFailed)?;
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
    let key = UnboundKey::new(&AES_256_GCM, &MASTER_KEY).map_err(|_| CryptoError::DecryptFailed)?;
    let key = LessSafeKey::new(key);
    let mut in_out = ciphertext.to_vec();
    let plain = key
        .open_in_place(nonce, Aad::empty(), &mut in_out)
        .map_err(|_| CryptoError::DecryptFailed)?;
    Ok(plain.to_vec())
}

#[cfg(test)]
mod tests {
    use super::{CryptoError, decrypt_bytes, encrypt_bytes};

    #[test]
    fn encrypt_roundtrip() {
        let input = b"vust-agent-test";
        let encrypted = encrypt_bytes(input).expect("encrypt should succeed");
        let decrypted = decrypt_bytes(&encrypted).expect("decrypt should succeed");
        assert_eq!(decrypted, input);
    }

    #[test]
    fn decrypt_rejects_invalid_input() {
        let err = decrypt_bytes("not-hex").expect_err("invalid input should fail");
        assert!(matches!(
            err,
            CryptoError::InvalidInput | CryptoError::DecryptFailed
        ));
    }
}
