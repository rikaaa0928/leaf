#[cfg(feature = "outbound-rog-tcp")]
use std::io;

#[cfg(feature = "outbound-rog-tcp")]
use chacha20poly1305::aead::rand_core::RngCore;
#[cfg(feature = "outbound-rog-tcp")]
use chacha20poly1305::aead::{Aead, OsRng};
#[cfg(feature = "outbound-rog-tcp")]
use chacha20poly1305::{AeadCore, ChaCha20Poly1305, KeyInit};
#[cfg(feature = "outbound-rog-tcp")]
use base64::engine::general_purpose::STANDARD as BASE64;
#[cfg(feature = "outbound-rog-tcp")]
use base64::Engine;
#[cfg(feature = "outbound-rog-tcp")]
use prost::Message;
#[cfg(feature = "outbound-rog-tcp")]
use sha2::{Digest, Sha256};
#[cfg(feature = "outbound-rog-tcp")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[cfg(feature = "outbound-rog-tcp")]
pub const CONN_TYPE_STREAM: u8 = 0x01;
#[cfg(feature = "outbound-rog-tcp")]
pub const CONN_TYPE_UDP: u8 = 0x02;

#[cfg(feature = "outbound-rog-tcp")]
fn derive_key(password: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hasher.finalize().into()
}

#[cfg(feature = "outbound-rog-tcp")]
pub fn encrypt_bytes(plaintext: &[u8], password: &str) -> io::Result<Vec<u8>> {
    let key = derive_key(password);
    let cipher = ChaCha20Poly1305::new(&key.into());
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| io::Error::other(format!("encrypt error: {}", e)))?;
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(combined)
}

#[cfg(feature = "outbound-rog-tcp")]
pub fn decrypt_bytes(combined: &[u8], password: &str) -> io::Result<Vec<u8>> {
    let key = derive_key(password);
    let cipher = ChaCha20Poly1305::new(&key.into());
    if combined.len() < 12 {
        return Err(io::Error::other("ciphertext too short"));
    }
    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = chacha20poly1305::Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| io::Error::other(format!("decrypt error: {}", e)))?;
    Ok(plaintext)
}

#[cfg(feature = "outbound-rog-tcp")]
pub fn encrypt_field(plaintext: &str, password: &str) -> io::Result<String> {
    let combined = encrypt_bytes(plaintext.as_bytes(), password)?;
    Ok(BASE64.encode(&combined))
}

#[cfg(feature = "outbound-rog-tcp")]
pub fn decrypt_field(ciphertext_b64: &str, password: &str) -> io::Result<String> {
    let combined = BASE64
        .decode(ciphertext_b64)
        .map_err(|e| io::Error::other(format!("base64 decode error: {}", e)))?;
    let plaintext = decrypt_bytes(&combined, password)?;
    String::from_utf8(plaintext).map_err(|e| io::Error::other(format!("utf8 error: {}", e)))
}

#[cfg(feature = "outbound-rog-tcp")]
pub async fn write_frame<W: AsyncWriteExt + Unpin, M: Message>(
    writer: &mut W,
    msg: &M,
) -> io::Result<()> {
    let data = msg.encode_to_vec();
    let len = data.len() as u64;
    let r = OsRng.next_u64();
    let second = len.wrapping_sub(r);
    writer.write_all(&r.to_be_bytes()).await?;
    writer.write_all(&second.to_be_bytes()).await?;
    writer.write_all(&data).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(feature = "outbound-rog-tcp")]
pub async fn read_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut buf16 = [0u8; 16];
    reader.read_exact(&mut buf16).await?;
    let r = u64::from_be_bytes(buf16[..8].try_into().unwrap());
    let second = u64::from_be_bytes(buf16[8..].try_into().unwrap());
    let len = r.wrapping_add(second) as usize;
    if len > 16 * 1024 * 1024 {
        return Err(io::Error::other("frame too large"));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

#[cfg(feature = "outbound-rog-tcp")]
pub async fn read_msg<R: AsyncReadExt + Unpin, M: Message + Default>(
    reader: &mut R,
) -> io::Result<M> {
    let data = read_frame(reader).await?;
    M::decode(data.as_slice()).map_err(|e| io::Error::other(format!("protobuf decode: {}", e)))
}

#[cfg(feature = "outbound-rog-tcp")]
pub async fn write_conn_type<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    conn_type: u8,
) -> io::Result<()> {
    writer.write_all(&[conn_type]).await?;
    writer.flush().await?;
    Ok(())
}
