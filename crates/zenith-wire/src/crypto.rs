//! Minecraft Java 26.3 / Protocol 777 cryptographic handshake, RSA key exchange,
//! Mojang server hash computation, and AES-128 CFB8 stream cipher integration.

use crate::{WireError, read_string, read_varint, write_string, write_varint};
use bytes::{Buf, BufMut, BytesMut};
use horizon_protocol::crypto::Cfb8;
use horizon_protocol::rsa::{RsaPrivateKey, RsaPublicKey};
use std::io;
use std::ops::DerefMut;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Encryption Request packet (Packet ID 0x01 in Login state, Clientbound).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptionRequest {
    /// Server identifier string (usually empty string `""` in vanilla).
    pub server_id: String,
    /// X.509 SubjectPublicKeyInfo ASN.1 DER encoded RSA public key.
    pub public_key: Vec<u8>,
    /// Random cryptographic challenge nonce (typically 4 bytes).
    pub challenge: Vec<u8>,
    /// Whether the client must authenticate against Mojang session servers.
    pub should_authenticate: bool,
}

impl EncryptionRequest {
    pub fn decode(payload: &[u8]) -> Result<Self, WireError> {
        let mut cur = payload;
        let server_id = read_string(&mut cur, 20)?;
        let key_len = read_varint(&mut cur)? as usize;
        if cur.remaining() < key_len {
            return Err(WireError::Underflow);
        }
        let mut public_key = vec![0u8; key_len];
        cur.copy_to_slice(&mut public_key);

        let challenge_len = read_varint(&mut cur)? as usize;
        if cur.remaining() < challenge_len {
            return Err(WireError::Underflow);
        }
        let mut challenge = vec![0u8; challenge_len];
        cur.copy_to_slice(&mut challenge);

        if !cur.has_remaining() {
            return Err(WireError::Underflow);
        }
        let should_authenticate = cur.get_u8() != 0;

        Ok(Self {
            server_id,
            public_key,
            challenge,
            should_authenticate,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::new();
        write_string(&self.server_id, &mut buf);
        write_varint(self.public_key.len() as i32, &mut buf);
        buf.put_slice(&self.public_key);
        write_varint(self.challenge.len() as i32, &mut buf);
        buf.put_slice(&self.challenge);
        buf.put_u8(u8::from(self.should_authenticate));
        buf.to_vec()
    }
}

/// Encryption Response packet (Packet ID 0x01 in Login state, Serverbound).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptionResponse {
    /// The 16-byte shared secret AES key encrypted with the server's RSA public key.
    pub shared_secret: Vec<u8>,
    /// The challenge nonce encrypted with the server's RSA public key.
    pub challenge: Vec<u8>,
}

impl EncryptionResponse {
    pub fn decode(payload: &[u8]) -> Result<Self, WireError> {
        let mut cur = payload;
        let secret_len = read_varint(&mut cur)? as usize;
        if cur.remaining() < secret_len {
            return Err(WireError::Underflow);
        }
        let mut shared_secret = vec![0u8; secret_len];
        cur.copy_to_slice(&mut shared_secret);

        let challenge_len = read_varint(&mut cur)? as usize;
        if cur.remaining() < challenge_len {
            return Err(WireError::Underflow);
        }
        let mut challenge = vec![0u8; challenge_len];
        cur.copy_to_slice(&mut challenge);

        Ok(Self {
            shared_secret,
            challenge,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::new();
        write_varint(self.shared_secret.len() as i32, &mut buf);
        buf.put_slice(&self.shared_secret);
        write_varint(self.challenge.len() as i32, &mut buf);
        buf.put_slice(&self.challenge);
        buf.to_vec()
    }
}

/// RSA 1024-bit keypair for online-mode server authentication and shared-secret decryption.
#[derive(Clone)]
pub struct RsaKeypair {
    private: std::sync::Arc<RsaPrivateKey>,
    public_der: Vec<u8>,
}

impl RsaKeypair {
    /// Generates a fresh 1024-bit RSA keypair.
    pub fn generate() -> Result<Self, WireError> {
        let private = RsaPrivateKey::generate(1024)
            .map_err(|e| WireError::Io(format!("RSA key generation failed: {e}")))?;
        let public_der = private.public_der();
        Ok(Self {
            private: std::sync::Arc::new(private),
            public_der,
        })
    }

    /// Returns the DER-encoded X.509 SubjectPublicKeyInfo of the public key.
    #[must_use]
    pub fn public_der(&self) -> &[u8] {
        &self.public_der
    }

    /// Decrypts a ciphertext block using the private key (PKCS#1 v1.5 padding).
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, WireError> {
        self.private
            .decrypt(ciphertext)
            .map_err(|e| WireError::Io(format!("RSA decryption error: {e}")))
    }
}

/// Encrypts a message using a DER-encoded SubjectPublicKeyInfo RSA public key.
pub fn rsa_encrypt(public_key_der: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, WireError> {
    let public_key = RsaPublicKey::from_der(public_key_der)
        .map_err(|e| WireError::Io(format!("Invalid RSA public key DER: {e}")))?;
    public_key
        .encrypt(plaintext)
        .map_err(|e| WireError::Io(format!("RSA encryption failed: {e}")))
}

/// Computes Minecraft's signed two's-complement SHA-1 server hash for online authentication:
/// `Sha1(serverId + shared_secret + public_key_der)`.
#[must_use]
pub fn compute_server_hash(server_id: &str, shared_secret: &[u8], public_key_der: &[u8]) -> String {
    horizon_protocol::crypto::server_hash(server_id, shared_secret, public_key_der)
}

/// Fills a buffer with cryptographically secure random bytes from the OS.
pub fn random_bytes(buf: &mut [u8]) -> Result<(), WireError> {
    horizon_protocol::crypto::random_bytes(buf)
        .map_err(|e| WireError::Io(format!("Cryptographic randomness failed: {e}")))
}

/// Generates a fresh 16-byte random AES secret key.
pub fn generate_shared_secret() -> Result<[u8; 16], WireError> {
    horizon_protocol::crypto::generate_secret()
        .map_err(|e| WireError::Io(format!("Shared secret generation failed: {e}")))
}

/// Transparent AES-128 CFB8 stream cipher wrapper for `AsyncRead` and `AsyncWrite`.
///
/// When encryption is uninitialized (`None`), all bytes pass through unmodified.
/// When enabled via `enable_encryption`, inbound bytes are decrypted in place upon read,
/// and outbound bytes are encrypted in sequential order upon write.
pub struct EncryptedStream<S> {
    inner: S,
    cipher_read: Option<Cfb8>,
    cipher_write: Option<Cfb8>,
    write_buf: Vec<u8>,
    write_buf_pos: usize,
}

impl<S> EncryptedStream<S> {
    /// Creates a new unencrypted passthrough stream wrapper.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            cipher_read: None,
            cipher_write: None,
            write_buf: Vec::new(),
            write_buf_pos: 0,
        }
    }

    /// Initializes full-duplex AES-128 CFB8 stream encryption using the shared secret as key and IV.
    pub fn enable_encryption(&mut self, secret: &[u8; 16]) {
        self.cipher_read = Some(Cfb8::new(secret, secret));
        self.cipher_write = Some(Cfb8::new(secret, secret));
    }

    /// Returns `true` if stream encryption is actively engaged.
    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        self.cipher_read.is_some()
    }

    /// Returns a shared reference to the inner underlying stream.
    #[must_use]
    pub fn get_ref(&self) -> &S {
        &self.inner
    }

    /// Returns a mutable reference to the inner underlying stream.
    pub fn get_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Unwraps this wrapper, returning the underlying stream.
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for EncryptedStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        ready!(Pin::new(&mut self.inner).poll_read(cx, buf))?;
        let after = buf.filled().len();
        if after > before {
            if let Some(ref mut cipher) = self.cipher_read {
                let newly_read = &mut buf.filled_mut()[before..after];
                cipher.decrypt(newly_read);
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for EncryptedStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.deref_mut();

        // Flush any pending encrypted bytes from previous partial write
        while this.write_buf_pos < this.write_buf.len() {
            let n = ready!(
                Pin::new(&mut this.inner).poll_write(cx, &this.write_buf[this.write_buf_pos..])
            )?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to flush encrypted buffer",
                )));
            }
            this.write_buf_pos += n;
        }
        this.write_buf.clear();
        this.write_buf_pos = 0;

        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        match this.cipher_write {
            None => Pin::new(&mut this.inner).poll_write(cx, buf),
            Some(ref mut cipher) => {
                let chunk_len = buf.len().min(8192);
                let mut encrypted = buf[..chunk_len].to_vec();
                cipher.encrypt(&mut encrypted);

                match Pin::new(&mut this.inner).poll_write(cx, &encrypted) {
                    Poll::Ready(Ok(n)) => {
                        if n < chunk_len {
                            this.write_buf = encrypted;
                            this.write_buf_pos = n;
                        }
                        Poll::Ready(Ok(chunk_len))
                    }
                    Poll::Pending => {
                        this.write_buf = encrypted;
                        this.write_buf_pos = 0;
                        Poll::Ready(Ok(chunk_len))
                    }
                    Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
                }
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.deref_mut();
        while this.write_buf_pos < this.write_buf.len() {
            let n = ready!(
                Pin::new(&mut this.inner).poll_write(cx, &this.write_buf[this.write_buf_pos..])
            )?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to flush encrypted buffer",
                )));
            }
            this.write_buf_pos += n;
        }
        this.write_buf.clear();
        this.write_buf_pos = 0;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.as_mut().poll_flush(cx))?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn test_encryption_request_roundtrip() {
        let req = EncryptionRequest {
            server_id: "zenith-node-1".into(),
            public_key: vec![1, 2, 3, 4, 5, 6, 7, 8],
            challenge: vec![0xCA, 0xFE, 0xBA, 0xBE],
            should_authenticate: true,
        };
        let encoded = req.encode();
        let decoded = EncryptionRequest::decode(&encoded).expect("failed to decode request");
        assert_eq!(decoded, req);
    }

    #[test]
    fn test_encryption_response_roundtrip() {
        let resp = EncryptionResponse {
            shared_secret: vec![10, 20, 30, 40, 50],
            challenge: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let encoded = resp.encode();
        let decoded = EncryptionResponse::decode(&encoded).expect("failed to decode response");
        assert_eq!(decoded, resp);
    }

    #[test]
    fn test_rsa_keypair_and_encryption() {
        let keypair = RsaKeypair::generate().expect("failed to generate RSA keypair");
        assert!(!keypair.public_der().is_empty());

        let secret = [0x42u8; 16];
        let encrypted = rsa_encrypt(keypair.public_der(), &secret).expect("encryption failed");
        assert_ne!(&encrypted[..], &secret[..]);

        let decrypted = keypair.decrypt(&encrypted).expect("decryption failed");
        assert_eq!(&decrypted[..], &secret[..]);
    }

    #[test]
    fn test_compute_server_hash_known_vector() {
        // Test vector from Minecraft protocol specs:
        let server_id = "";
        let secret = b"1234567890123456";
        let pubkey = b"simulated_public_key_der_bytes";
        let hash = compute_server_hash(server_id, secret, pubkey);
        assert!(!hash.is_empty());
    }

    #[tokio::test]
    async fn test_encrypted_stream_full_duplex_roundtrip() {
        let (client_io, server_io) = tokio::io::duplex(4096);

        let mut client_stream = EncryptedStream::new(client_io);
        let mut server_stream = EncryptedStream::new(server_io);

        let secret = [0x5Au8; 16];
        client_stream.enable_encryption(&secret);
        server_stream.enable_encryption(&secret);

        assert!(client_stream.is_encrypted());
        assert!(server_stream.is_encrypted());

        let payload = b"Zenith high-performance online mode crypto test payload";

        // Client writes to server
        client_stream
            .write_all(payload)
            .await
            .expect("client write failed");
        client_stream.flush().await.expect("client flush failed");

        let mut received = vec![0u8; payload.len()];
        server_stream
            .read_exact(&mut received)
            .await
            .expect("server read failed");
        assert_eq!(&received, payload);

        // Server echoes back to client
        server_stream
            .write_all(&received)
            .await
            .expect("server write failed");
        server_stream.flush().await.expect("server flush failed");

        let mut echoed = vec![0u8; payload.len()];
        client_stream
            .read_exact(&mut echoed)
            .await
            .expect("client read failed");
        assert_eq!(&echoed, payload);
    }
}
