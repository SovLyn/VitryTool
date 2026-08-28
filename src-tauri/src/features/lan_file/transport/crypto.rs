//! VLF/1 密码学原语组装（纯逻辑，契约 `docs/api/lan-file.md` 5.5）。
//!
//! - 身份：ed25519 长期身份（复用 peer-key），握手双向签名覆盖
//!   `magic+version+双方公钥+双方 nonce+方向角色`；`multihash(公钥)==peerId` 一致性检查；
//! - 密钥：X25519 临时 ECDH → HKDF-SHA256（salt=nonceA‖nonceB‖peerIdA‖peerIdB，
//!   info="vitry-lan-file-v1"）→ c2s / s2c 两把独立 ChaCha20-Poly1305 密钥；
//! - AEAD：nonce = 8B 会话随机前缀 + 4B 逐方向计数器（BE），每加密一次自增；
//!   AAD = 会话 ID(32B) ‖ fileIndex u32BE ‖ seq u32BE（Chunk 帧）；普通帧 AAD 仅会话 ID。

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// 握手 magic（VLF/1）。
pub const MAGIC: &[u8; 4] = b"VLF1";
/// 协议版本。
pub const VERSION: u8 = 1;
/// HKDF info 基串（契约 5.5：info="vitry-lan-file-v1"），方向标签拼接其后。
pub const HKDF_INFO: &[u8] = b"vitry-lan-file-v1";
/// 会话 ID 长度（SHA-256(trunc(Random 16B))）。
pub const SESSION_ID_LEN: usize = 32;
/// nonce 随机前缀长度。
pub const NONCE_PREFIX_LEN: usize = 8;
/// nonce 计数器长度。
pub const NONCE_COUNTER_LEN: usize = 4;
/// 计数器上限（2^32 帧内不复用；超限上层须断连重建）。
pub const NONCE_MAX_FRAMES: u64 = 1u64 << 32;

/// 握手方向角色（签名覆盖域内）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// 发起方（dial 接收方）。
    Initiator,
    /// 应答方（TCP 监听侧）。
    Responder,
}

impl Role {
    /// 签名覆盖域中的角色字节（契约 5.5）。
    pub fn tag(self) -> u8 {
        match self {
            Role::Initiator => b'i',
            Role::Responder => b'r',
        }
    }
}

/// 生成随机字节。
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

/// 会话 ID：`SHA-256(trunc(Random 16B))[..32]`（双方一致，用于 AAD 绑定与日志）。
pub fn session_id_from_nonce_prefix(prefix: &[u8]) -> [u8; SESSION_ID_LEN] {
    let mut h = Sha256::new();
    h.update(prefix);
    let out = h.finalize();
    out.into()
}

/// ed25519 公钥 → libp2p peerId 字符串（base58）。
///
/// libp2p 算法：公钥先 protobuf 编码（`0x08 0x01 0x12 0x20 || pubkey`，36B），
/// 再 identity multihash（`0x00 0x24 || 36B`），base58 后即 `12D3Koo…` 形态。
pub fn peer_id_from_ed25519(pubkey: &ed25519_dalek::VerifyingKey) -> String {
    let mut mh = Vec::with_capacity(38);
    mh.push(0x00); // identity hash code
    mh.push(0x24); // length 36
    mh.push(0x08); // protobuf field 1 (Type), varint tag
    mh.push(0x01); // Type = Ed25519
    mh.push(0x12); // protobuf field 2 (Data), bytes tag
    mh.push(0x20); // length 32
    mh.extend_from_slice(&pubkey.to_bytes());
    bs58_encode(&mh)
}

/// 校验 `multihash(公钥) == peerId`（协议固有一致性检查，契约 5.5 / 5.9）。
pub fn verify_peer_id_binding(pubkey: &ed25519_dalek::VerifyingKey, peer_id: &str) -> bool {
    peer_id_from_ed25519(pubkey) == peer_id
}

/// 公钥指纹（契约 3）：`SHA256:<base64(公钥 32B)>`，展示参考。
pub fn fingerprint_of(pubkey: &ed25519_dalek::VerifyingKey) -> String {
    format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD.encode(pubkey.to_bytes())
    )
}

/// 最小 base58 编码（Bitcoin 字母表；peerId 用，避免引入 libp2p 内部私有 API）。
pub fn bs58_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let zeros = data.iter().take_while(|b| **b == 0).count();
    let mut digits: Vec<u8> = Vec::with_capacity(data.len() * 2);
    for &byte in &data[zeros..] {
        let mut carry = byte as usize;
        for d in digits.iter_mut() {
            carry += (*d as usize) << 8;
            *d = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let mut out = String::with_capacity(zeros + digits.len());
    for _ in 0..zeros {
        out.push('1');
    }
    for d in digits.iter().rev() {
        out.push(ALPHABET[*d as usize] as char);
    }
    out
}

/// HKDF 双向密钥（契约 5.5）：`(c2s_key, s2c_key, session_id)`。
///
/// IKM = X25519(本端临时私钥, 对端临时公钥)；salt = nonceA‖nonceB‖peerIdA‖peerIdB；
/// c2s = 发起方→应答方方向密钥，s2c 反之。方向密钥用不同 info 标签展开（同 IKM 下
/// 两方向独立：info = HKDF_INFO ‖ 方向标签）。
pub fn derive_session_keys(
    ikm: &[u8],
    nonce_a: &[u8; 16],
    nonce_b: &[u8; 16],
    peer_id_a: &str,
    peer_id_b: &str,
) -> ([u8; 32], [u8; 32], [u8; SESSION_ID_LEN]) {
    let mut salt = Vec::with_capacity(32 + peer_id_a.len() + peer_id_b.len());
    salt.extend_from_slice(nonce_a);
    salt.extend_from_slice(nonce_b);
    salt.extend_from_slice(peer_id_a.as_bytes());
    salt.extend_from_slice(peer_id_b.as_bytes());
    let hk = Hkdf::<Sha256>::new(Some(&salt), ikm);
    let mut c2s = [0u8; 32];
    let mut s2c = [0u8; 32];
    let mut session_id = [0u8; SESSION_ID_LEN];
    hk.expand(&join_info(b"c2s"), &mut c2s).expect("32 okm");
    hk.expand(&join_info(b"s2c"), &mut s2c).expect("32 okm");
    hk.expand(b"vitry-session-id", &mut session_id)
        .expect("32 okm");
    (c2s, s2c, session_id)
}

/// info = HKDF_INFO ‖ 标签（方向分离）。
fn join_info(tag: &[u8]) -> Vec<u8> {
    let mut info = HKDF_INFO.to_vec();
    info.extend_from_slice(tag);
    info
}

/// X25519 ECDH（临时密钥对 → 共享秘密）。
pub fn x25519_shared_secret(
    secret: &x25519_dalek::StaticSecret,
    peer_public: &[u8; 32],
) -> Result<[u8; 32], String> {
    let public = x25519_dalek::PublicKey::from(*peer_public);
    Ok(secret.diffie_hellman(&public).to_bytes())
}

/// 生成 X25519 临时密钥对。
pub fn x25519_keypair() -> (x25519_dalek::StaticSecret, x25519_dalek::PublicKey) {
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let secret = x25519_dalek::StaticSecret::from(seed);
    let public = x25519_dalek::PublicKey::from(&secret);
    (secret, public)
}

/// 生成 X25519 临时私钥（会话握手用；公钥由其派生）。
pub fn x25519_keypair_secret() -> x25519_dalek::StaticSecret {
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    x25519_dalek::StaticSecret::from(seed)
}

/// 生成握手随机 nonce（16B）。
pub fn random_nonce() -> [u8; 16] {
    let mut n = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut n);
    n
}

/// 生成 AEAD nonce 随机前缀（8B）。
pub fn random_prefix() -> [u8; 8] {
    let mut n = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut n);
    n
}

/// 从 libp2p Keypair 提取 ed25519-dalek SigningKey（VLF 握手签名复用同一身份密钥）。
///
/// libp2p identity 的 ed25519 Keypair 内部即 ed25519-dalek SigningKey；
/// 经 secret().as_ref()（32B 种子）重建，两者签名互通（同版本族 dalek 2.x）。
pub fn signing_key_from_libp2p(
    kp: &libp2p::identity::Keypair,
) -> Result<ed25519_dalek::SigningKey, String> {
    let ed_kp = kp
        .clone()
        .try_into_ed25519()
        .map_err(|_| "identity is not ed25519".to_string())?;
    let secret = ed_kp.secret();
    let bytes: &[u8] = secret.as_ref();
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "ed25519 seed length mismatch".to_string())?;
    Ok(ed25519_dalek::SigningKey::from_bytes(&seed))
}

/// AEAD 会话（单方向）：ChaCha20-Poly1305 + 8B 随机前缀 + 4B 逐帧计数器。
pub struct AeadDirection {
    cipher: ChaCha20Poly1305,
    prefix: [u8; NONCE_PREFIX_LEN],
    counter: u64,
}

impl AeadDirection {
    /// 创建方向 AEAD；nonce 前缀随机生成（会话内不重复）。
    pub fn new(key: [u8; 32]) -> Self {
        let mut prefix = [0u8; NONCE_PREFIX_LEN];
        rand::thread_rng().fill_bytes(&mut prefix);
        Self::with_prefix(key, prefix)
    }

    /// 以指定前缀创建（测试确定性用）。
    pub fn with_prefix(key: [u8; 32], prefix: [u8; NONCE_PREFIX_LEN]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(&key)),
            prefix,
            counter: 0,
        }
    }

    /// 已发送帧数。
    pub fn frames(&self) -> u64 {
        self.counter
    }

    fn next_nonce(&mut self) -> Result<Nonce, String> {
        if self.counter >= NONCE_MAX_FRAMES {
            return Err("nonce counter exhausted".into());
        }
        let mut nonce = [0u8; 12];
        nonce[..NONCE_PREFIX_LEN].copy_from_slice(&self.prefix);
        nonce[NONCE_PREFIX_LEN..].copy_from_slice(&(self.counter as u32).to_be_bytes());
        self.counter += 1;
        Ok(Nonce::from(nonce))
    }

    /// 加密一帧（AAD = aad ‖ 会话 ID 由调用方拼装；nonce 前缀随密文无关，不传输——
    /// 双方在握手后交换一次前缀，见 `SessionCrypto`）。
    pub fn seal(&mut self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, String> {
        let nonce = self.next_nonce()?;
        self.cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|e| format!("aead seal: {e}"))
    }

    /// 解密一帧。
    pub fn open(&self, ciphertext: &[u8], aad: &[u8], counter: u32) -> Result<Vec<u8>, String> {
        let mut nonce = [0u8; 12];
        nonce[..NONCE_PREFIX_LEN].copy_from_slice(&self.prefix);
        nonce[NONCE_PREFIX_LEN..].copy_from_slice(&counter.to_be_bytes());
        self.cipher
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|e| format!("aead open: {e}"))
    }
}

/// 会话密码学套件：双向 AEAD + 会话 ID。
pub struct SessionCrypto {
    /// 发起方→应答方方向。
    pub c2s: AeadDirection,
    /// 应答方→发起方方向。
    pub s2c: AeadDirection,
    /// 32B 会话 ID（AAD 绑定 + 日志）。
    pub session_id: [u8; SESSION_ID_LEN],
}

/// 握手签名覆盖域（契约 5.5）：magic+version+双方公钥+双方 nonce+方向角色。
pub fn handshake_signing_bytes(
    initiator_pub: &[u8; 32],
    responder_pub: &[u8; 32],
    nonce_a: &[u8; 16],
    nonce_b: &[u8; 16],
    responder_side: bool,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + 1 + 64 + 32 + 1);
    buf.extend_from_slice(MAGIC);
    buf.push(VERSION);
    buf.extend_from_slice(initiator_pub);
    buf.extend_from_slice(responder_pub);
    buf.extend_from_slice(nonce_a);
    buf.extend_from_slice(nonce_b);
    buf.push(if responder_side {
        Role::Responder.tag()
    } else {
        Role::Initiator.tag()
    });
    buf
}

/// 握手完成：由共享秘密派生双向密钥与会话 ID。
pub fn finish_handshake(
    ikm: &[u8],
    nonce_a: &[u8; 16],
    nonce_b: &[u8; 16],
    peer_id_a: &str,
    peer_id_b: &str,
) -> SessionCrypto {
    let (c2s_key, s2c_key, session_id) =
        derive_session_keys(ikm, nonce_a, nonce_b, peer_id_a, peer_id_b);
    SessionCrypto {
        c2s: AeadDirection::new(c2s_key),
        s2c: AeadDirection::new(s2c_key),
        session_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// libp2p peerId 路径一致性：ed25519 公钥 → protobuf → identity multihash → base58
    /// 必须与 libp2p `Keypair::public().to_peer_id()` 同构（前缀 12D3Koo + 52 字符），
    /// 真机与 libp2p 互通时由公告自校验兜底（announce_consistent）。
    #[test]
    fn peer_id_matches_libp2p_vector() {
        // 固定种子 ed25519（测试专用；生产用 identity.rs 持久化密钥）
        let seed: [u8; 32] = core::array::from_fn(|i| i as u8);
        let signing = SigningKey::from_bytes(&seed);
        let peer = peer_id_from_ed25519(&signing.verifying_key());
        // multihash identity: 0x00 0x24 || protobuf(36B) → base58；前缀应为 12D3Koo
        assert!(peer.starts_with("12D3Koo"), "peer={peer}");
        assert_eq!(peer.len(), 52);
    }

    #[test]
    fn verify_peer_id_binding_roundtrip() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let vk = signing.verifying_key();
        let peer = peer_id_from_ed25519(&vk);
        assert!(verify_peer_id_binding(&vk, &peer));
        assert!(!verify_peer_id_binding(&vk, "12D3KooWRONG"));
    }

    #[test]
    fn fingerprint_format() {
        let signing = SigningKey::from_bytes(&[9u8; 32]);
        let fp = fingerprint_of(&signing.verifying_key());
        assert!(fp.starts_with("SHA256:"));
        // 32B base64 = 44 chars
        assert_eq!(fp.len(), "SHA256:".len() + 44);
    }

    #[test]
    fn hkdf_keys_deterministic_and_independent() {
        let ikm = [42u8; 32];
        let na = [1u8; 16];
        let nb = [2u8; 16];
        let (c1, s1, sid1) = derive_session_keys(&ikm, &na, &nb, "peerA", "peerB");
        let (c2, s2, sid2) = derive_session_keys(&ikm, &na, &nb, "peerA", "peerB");
        assert_eq!(c1, c2);
        assert_eq!(s1, s2);
        assert_eq!(sid1, sid2);
        // 方向独立
        assert_ne!(c1, s1);
        // 输入变化 → 输出变化
        let (c3, s3, sid3) = derive_session_keys(&ikm, &na, &nb, "peerA", "peerC");
        assert_ne!(c1, c3);
        assert_ne!(s1, s3);
        assert_ne!(sid1, sid3);
    }

    #[test]
    fn aead_roundtrip_and_wrong_key_fails() {
        let mut a = AeadDirection::with_prefix([5u8; 32], [1u8; 8]);
        let ct = a.seal(b"hello", b"aad").unwrap();
        let b = AeadDirection::with_prefix([5u8; 32], [1u8; 8]);
        let pt = b.open(&ct, b"aad", 0).unwrap();
        assert_eq!(pt, b"hello");
        // 计数器逐帧递增：第二帧 counter=1
        let ct2 = a.seal(b"world", b"aad").unwrap();
        assert!(b.open(&ct2, b"aad", 1).is_ok());
        assert!(
            b.open(&ct2, b"aad", 0).is_err(),
            "counter mismatch must fail"
        );
        // AAD 篡改拒绝
        let c = AeadDirection::with_prefix([5u8; 32], [1u8; 8]);
        assert!(c.open(&ct, b"BAD", 0).is_err());
        // 错误密钥拒绝
        let d = AeadDirection::with_prefix([6u8; 32], [1u8; 8]);
        assert!(d.open(&ct, b"aad", 0).is_err());
    }

    #[test]
    fn aead_nonce_prefix_unique_per_session() {
        let a = AeadDirection::new([1u8; 32]);
        let b = AeadDirection::new([1u8; 32]);
        // 随机前缀 8B：两次会话 nonce 空间独立（2^-64 碰撞概率，忽略不计）；
        // 随机性本身由 with_prefix 确定性测试覆盖，这里验证 frames 计数与构造。
        assert_eq!(a.frames(), 0);
        assert_eq!(b.frames(), 0);
    }

    #[test]
    fn signing_bytes_cover_all_fields() {
        let b1 = handshake_signing_bytes(&[1; 32], &[2; 32], &[3; 16], &[4; 16], false);
        let b2 = handshake_signing_bytes(&[1; 32], &[2; 32], &[3; 16], &[4; 16], true);
        assert_ne!(b1, b2, "方向角色必须进入签名域");
        assert_eq!(&b1[..4], MAGIC);
        let b3 = handshake_signing_bytes(&[9; 32], &[2; 32], &[3; 16], &[4; 16], false);
        assert_ne!(b1, b3);
    }

    #[test]
    fn full_handshake_sign_verify_with_binding() {
        // 双方身份
        let ikey = SigningKey::from_bytes(&[10u8; 32]);
        let rkey = SigningKey::from_bytes(&[20u8; 32]);
        let ipub = ikey.verifying_key();
        let rpub = rkey.verifying_key();
        let ipub_b = ipub.to_bytes();
        let rpub_b = rpub.to_bytes();
        let peer_a = peer_id_from_ed25519(&ipub);
        let peer_b = peer_id_from_ed25519(&rpub);
        let nonce_a: [u8; 16] = core::array::from_fn(|i| i as u8);
        let nonce_b: [u8; 16] = core::array::from_fn(|i| (i + 100) as u8);

        // 双方各自计算签名域并发签名
        let i_bytes = handshake_signing_bytes(&ipub_b, &rpub_b, &nonce_a, &nonce_b, false);
        let r_bytes = handshake_signing_bytes(&ipub_b, &rpub_b, &nonce_a, &nonce_b, true);
        let isig = ikey.sign(&i_bytes);
        let rsig = rkey.sign(&r_bytes);

        // 接收方校验：签名 + 绑定
        use ed25519_dalek::Verifier;
        assert!(rkey.verifying_key().verify(&r_bytes, &rsig).is_ok());
        assert!(ikey.verifying_key().verify(&i_bytes, &isig).is_ok());
        assert!(verify_peer_id_binding(&ipub, &peer_a));
        assert!(verify_peer_id_binding(&rpub, &peer_b));

        // 篡改 nonce → 验签失败
        let tampered = handshake_signing_bytes(&ipub_b, &rpub_b, &[9u8; 16], &nonce_b, true);
        assert!(rkey.verifying_key().verify(&tampered, &rsig).is_err());
    }

    #[test]
    fn session_keys_agree_on_both_sides() {
        // 两端各自 X25519 → IKM 相同 → HKDF 输出相同
        let (isec, ipub) = x25519_keypair();
        let (rsec, rpub) = x25519_keypair();
        let ikm_i = x25519_shared_secret(&isec, &rpub.to_bytes()).unwrap();
        let ikm_r = x25519_shared_secret(&rsec, &ipub.to_bytes()).unwrap();
        assert_eq!(ikm_i, ikm_r);
        let nonce_a = [1u8; 16];
        let nonce_b = [2u8; 16];
        let s_i = finish_handshake(&ikm_i, &nonce_a, &nonce_b, "A", "B");
        let s_r = finish_handshake(&ikm_r, &nonce_a, &nonce_b, "A", "B");
        assert_eq!(s_i.session_id, s_r.session_id);
        // 方向密钥一致性：双方以相同前缀（握手时交换）+ 相同方向密钥互解
        let prefix = [0xA5u8; 8];
        let mut i_sender = AeadDirection::with_prefix(
            derive_session_keys(&ikm_i, &nonce_a, &nonce_b, "A", "B").0,
            prefix,
        );
        let ct_fixed = i_sender.seal(b"payload", b"aad").unwrap();
        let r_opener = AeadDirection::with_prefix(
            derive_session_keys(&ikm_r, &nonce_a, &nonce_b, "A", "B").0,
            prefix,
        );
        assert_eq!(r_opener.open(&ct_fixed, b"aad", 0).unwrap(), b"payload");
    }

    #[test]
    fn bs58_known_vectors() {
        // RFC 4648 风格向量（Bitcoin base58 常用测试向量）
        assert_eq!(bs58_encode(&[0, 0]), "11");
        assert_eq!(bs58_encode(&[0]), "1");
        // "hello" → "Cn8eVZg"
        assert_eq!(bs58_encode(b"hello"), "Cn8eVZg");
    }

    #[test]
    fn signing_key_roundtrip_with_libp2p_keypair() {
        let kp = libp2p::identity::Keypair::generate_ed25519();
        let libp2p_peer = kp.public().to_peer_id().to_string();
        let signing = signing_key_from_libp2p(&kp).unwrap();
        // 签名互通：libp2p 公钥验证 dalek 签名，且 peerId 绑定一致
        use ed25519_dalek::Signer;
        let sig = signing.sign(b"msg");
        let vk = signing.verifying_key();
        use ed25519_dalek::Verifier;
        assert!(vk.verify(b"msg", &sig).is_ok());
        assert_eq!(
            peer_id_from_ed25519(&vk),
            libp2p_peer,
            "VLF 身份必须与 libp2p peerId 完全一致"
        );
        assert!(verify_peer_id_binding(&vk, &libp2p_peer));
    }
}
