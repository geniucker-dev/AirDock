// SPDX-License-Identifier: MPL-2.0
use aes::Aes128;
use anyhow::{Result, bail};
use ctr::cipher::{KeyIvInit, StreamCipher};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use sha2::{Digest, Sha512};
use std::path::Path;
use x25519_dalek::{PublicKey, StaticSecret};

#[path = "playfair.rs"]
mod playfair;

pub type AesCtr = ctr::Ctr128BE<Aes128>;

pub fn derive16(salt: &[u8], secret: &[u8]) -> [u8; 16] {
    let mut h = Sha512::new();
    h.update(salt);
    h.update(secret);
    h.finalize()[..16].try_into().unwrap()
}

pub fn load_identity(path: &Path) -> Result<SigningKey> {
    if path.exists() {
        let bytes = std::fs::read(path)?;
        let seed: [u8; 32] = bytes.try_into().map_err(|_| {
            anyhow::anyhow!(
                "Identity must contain a 32-byte seed; refusing to change receiver identity"
            )
        })?;
        return Ok(SigningKey::from_bytes(&seed));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let key = SigningKey::generate(&mut OsRng);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(path)?;
    file.write_all(&key.to_bytes())?;
    file.sync_all()?;
    Ok(key)
}

pub struct PairVerify {
    cipher: AesCtr,
    client_ed: VerifyingKey,
    client_x: [u8; 32],
    server_x: [u8; 32],
    pub secret: [u8; 32],
    pub verified: bool,
}

impl PairVerify {
    pub fn start(identity: &SigningKey, input: &[u8]) -> Result<(Self, Vec<u8>)> {
        if input.len() != 68 || input[..4] != [1, 0, 0, 0] {
            bail!("Invalid pair-verify round one")
        }
        let client_x: [u8; 32] = input[4..36].try_into()?;
        let client_ed = VerifyingKey::from_bytes(input[36..68].try_into()?)?;
        let ephemeral = StaticSecret::random_from_rng(OsRng);
        let server_x = PublicKey::from(&ephemeral).to_bytes();
        let shared = ephemeral.diffie_hellman(&PublicKey::from(client_x));
        if !shared.was_contributory() {
            bail!("Non-contributory X25519 public key")
        }
        let secret = shared.to_bytes();
        let key = derive16(b"Pair-Verify-AES-Key", &secret);
        let iv = derive16(b"Pair-Verify-AES-IV", &secret);
        let mut cipher = AesCtr::new(&key.into(), &iv.into());
        let mut signed = [0; 64];
        signed[..32].copy_from_slice(&server_x);
        signed[32..].copy_from_slice(&client_x);
        let mut signature = identity.sign(&signed).to_bytes();
        cipher.apply_keystream(&mut signature);
        let mut reply = Vec::with_capacity(96);
        reply.extend_from_slice(&server_x);
        reply.extend_from_slice(&signature);
        Ok((
            Self {
                cipher,
                client_ed,
                client_x,
                server_x,
                secret,
                verified: false,
            },
            reply,
        ))
    }
    pub fn finish(&mut self, input: &[u8]) -> Result<()> {
        if self.verified || input.len() != 68 || input[..4] != [0; 4] {
            bail!("Invalid pair-verify round two")
        }
        let mut signature: [u8; 64] = input[4..68].try_into()?;
        self.cipher.apply_keystream(&mut signature);
        let mut signed = [0; 64];
        signed[..32].copy_from_slice(&self.client_x);
        signed[32..].copy_from_slice(&self.server_x);
        self.client_ed
            .verify_strict(&signed, &Signature::from_bytes(&signature))?;
        self.verified = true;
        Ok(())
    }
}

#[derive(Default)]
pub struct FairPlay {
    message3: Option<[u8; 164]>,
    started: bool,
}
impl FairPlay {
    pub fn process(&mut self, input: &[u8]) -> Result<Vec<u8>> {
        if input.len() < 5 || &input[..4] != b"FPLY" || input[4] != 3 {
            bail!("Invalid FairPlay framing")
        }
        match input.len() {
            16 if input[14] < 4 => {
                self.started = true;
                self.message3 = None;
                Ok(include_bytes!("fairplay-replies.bin")
                    [input[14] as usize * 142..(input[14] as usize + 1) * 142]
                    .to_vec())
            }
            164 if self.started => {
                self.message3 = Some(input.try_into()?);
                let mut out = vec![0x46, 0x50, 0x4c, 0x59, 3, 1, 4, 0, 0, 0, 0, 0x14];
                out.extend_from_slice(&input[144..164]);
                Ok(out)
            }
            _ => bail!("Invalid FairPlay length or handshake state"),
        }
    }
    pub fn decrypt(&self, encrypted: &[u8]) -> Result<[u8; 16]> {
        let m3 = self
            .message3
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("FairPlay setup incomplete"))?;
        if encrypted.len() != 72 || m3[12] > 3 {
            bail!("Invalid encrypted stream key")
        }
        stream_key(m3, encrypted)
    }
}
pub fn stream_key(message3: &[u8], encrypted: &[u8]) -> Result<[u8; 16]> {
    if message3.len() != 164 || message3[12] > 3 || encrypted.len() != 72 {
        bail!("Invalid FairPlay key derivation input")
    }
    Ok(playfair::playfair_decrypt(message3, encrypted))
}

pub fn mirror_cipher(key: &[u8; 16], connection: u64) -> AesCtr {
    let k = derive16(format!("AirPlayStreamKey{connection}").as_bytes(), key);
    let iv = derive16(format!("AirPlayStreamIV{connection}").as_bytes(), key);
    AesCtr::new(&k.into(), &iv.into())
}

pub fn decrypt_audio(key: &[u8; 16], iv: &[u8; 16], payload: &mut [u8]) {
    use cbc::cipher::{BlockDecryptMut, KeyIvInit};
    let mut cipher = cbc::Decryptor::<Aes128>::new(key.into(), iv.into());
    for block in payload.as_chunks_mut::<16>().0 {
        cipher.decrypt_block_mut(block.into());
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn promoted_shift_matches_known_answer() {
        let (message, key) = include_str!("../tests/fixtures/fairplay-regression.txt")
            .trim()
            .split_once(' ')
            .unwrap();
        assert_eq!(
            hex::encode(
                stream_key(&hex::decode(message).unwrap(), &hex::decode(key).unwrap()).unwrap()
            ),
            "2a7ef6c632626859b485d44cfe6bc3a6"
        );
    }
}
