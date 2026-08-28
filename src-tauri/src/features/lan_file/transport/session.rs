//! VLF/1 TCP 会话层（数据面，契约 `docs/api/lan-file.md` 5.5）。
//!
//! 一条 TCP 连接 = 一个任务生命周期实例（断线重连开新连接）。
//! 帧：`[u32 BE 帧长][载荷]`，帧长先校验再分配（上限 1 MiB）。
//! 握手：明文 HELLO×2 + AUTH×2（ed25519 签名 + multihash 绑定校验）→ HKDF 双向密钥。
//! 应用帧（AEAD 密文）：由 state.rs 任务管理器经 `send_json`/`send_chunk` 编排。
#![allow(dead_code)]

use super::crypto::{self, handshake_signing_bytes, Role, SESSION_ID_LEN};
use super::proto::{
    cancel_aad, chunk_aad, plain_aad, AuthFrame, HelloFrame, CONNECT_TIMEOUT, HANDSHAKE_TIMEOUT,
    MAX_FRAME_LEN,
};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use ed25519_dalek::Signer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// 会话级错误（上层映射为稳定错误码）。
#[derive(Debug)]
pub enum SessionError {
    /// 对端拒绝（含稳定错误码）。
    Rejected(String),
    /// 对端取消。
    Cancelled(String),
    /// IO / 协议错误（`lan_file.transfer_failed`）。
    Io(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Rejected(c) => write!(f, "rejected: {c}"),
            SessionError::Cancelled(r) => write!(f, "cancelled: {r}"),
            SessionError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl From<std::io::Error> for SessionError {
    fn from(e: std::io::Error) -> Self {
        SessionError::Io(e.to_string())
    }
}

/// 读取一帧（`u32 BE 长度 + 载荷`；先校验长度再分配）。
pub async fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>, SessionError> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(SessionError::Io(format!("frame too large: {len}")));
    }
    let mut buf = vec![0u8; len as usize];
    if len > 0 {
        stream.read_exact(&mut buf).await?;
    }
    Ok(buf)
}

/// 写一帧。
pub async fn write_frame(stream: &mut TcpStream, data: &[u8]) -> Result<(), SessionError> {
    stream.write_all(&(data.len() as u32).to_be_bytes()).await?;
    stream.write_all(data).await?;
    stream.flush().await?;
    Ok(())
}

/// 加密通道（一方向）。
struct Channel {
    cipher: ChaCha20Poly1305,
    prefix: [u8; 8],
    counter: u32,
}

impl Channel {
    fn new(key: [u8; 32], prefix: [u8; 8]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(&key)),
            prefix,
            counter: 0,
        }
    }

    fn nonce(&self, counter: u32) -> Nonce {
        let mut n = [0u8; 12];
        n[..8].copy_from_slice(&self.prefix);
        n[8..].copy_from_slice(&counter.to_be_bytes());
        Nonce::from(n)
    }
}

/// 已建立的加密会话（连接级）。
pub struct SecureSession {
    stream: TcpStream,
    /// 本端发送通道（c2s 若本端为发起方，否则 s2c）。
    tx: Channel,
    /// 对端发送通道（解密用）。
    rx: Channel,
    /// 本端是否发起方（Chunk/End 由发起方发送，方向密钥选择用）。
    is_initiator: bool,
    pub session_id: [u8; SESSION_ID_LEN],
}

/// 已认证的对端身份（握手产物）。
#[derive(Debug, Clone)]
pub struct PeerIdentity {
    pub peer_id: String,
    pub fingerprint: String,
}

impl SecureSession {
    /// 发起方握手：dial 后调用（role=initiator）。
    pub async fn connect(
        addr: &str,
        signing: &ed25519_dalek::SigningKey,
        self_peer_id: &str,
    ) -> Result<(Self, PeerIdentity), SessionError> {
        let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
            .await
            .map_err(|_| SessionError::Io("connect timeout".into()))?
            .map_err(SessionError::from)?;
        let _ = stream.set_nodelay(true);
        Self::handshake(stream, signing, self_peer_id, true).await
    }

    /// 应答方握手：accept 后调用（role=responder）。
    pub async fn accept(
        stream: TcpStream,
        signing: &ed25519_dalek::SigningKey,
        self_peer_id: &str,
    ) -> Result<(Self, PeerIdentity), SessionError> {
        let _ = stream.set_nodelay(true);
        Self::handshake(stream, signing, self_peer_id, false).await
    }

    async fn handshake(
        mut stream: TcpStream,
        signing: &ed25519_dalek::SigningKey,
        self_peer_id: &str,
        as_initiator: bool,
    ) -> Result<(Self, PeerIdentity), SessionError> {
        let handshake = async {
            let self_pub = signing.verifying_key();
            let self_pub_b = self_pub.to_bytes();
            let x_secret = crypto::x25519_keypair_secret();
            let x_pub = x25519_dalek::PublicKey::from(&x_secret).to_bytes();
            // nonce：发起方先生成；应答方生成的会被发起方覆盖逻辑对齐（见下方）
            let self_nonce: [u8; 16] = crypto::random_nonce();

            let (hello, hello_bytes) = if as_initiator {
                let h = HelloFrame {
                    peer_id: self_peer_id.to_string(),
                    identity_pub: self_pub_b,
                    x25519_pub: x_pub,
                    nonce: self_nonce,
                };
                let bytes = h.encode(Role::Initiator.tag());
                (h, bytes)
            } else {
                let h = HelloFrame {
                    peer_id: self_peer_id.to_string(),
                    identity_pub: self_pub_b,
                    x25519_pub: x_pub,
                    nonce: self_nonce,
                };
                let bytes = h.encode(Role::Responder.tag());
                (h, bytes)
            };
            write_frame(&mut stream, &hello_bytes).await?;
            // 读对端 HELLO
            let peer_hello_raw = read_frame(&mut stream).await?;
            let (peer_role, peer_hello) =
                HelloFrame::decode(&peer_hello_raw).map_err(SessionError::Io)?;
            let expected_role = if as_initiator {
                Role::Responder.tag()
            } else {
                Role::Initiator.tag()
            };
            if peer_role != expected_role {
                return Err(SessionError::Io("handshake role mismatch".into()));
            }
            // 对端身份校验：multihash(公钥) == 宣称 peerId
            let peer_vk = ed25519_dalek::VerifyingKey::from_bytes(&peer_hello.identity_pub)
                .map_err(|e| SessionError::Io(format!("bad peer pubkey: {e}")))?;
            if !crypto::verify_peer_id_binding(&peer_vk, &peer_hello.peer_id) {
                return Err(SessionError::Io("peerId binding mismatch".into()));
            }

            // 方向参数：nonceA/peerIdA 恒为发起方
            let (nonce_a, nonce_b, ipub, rpub) = if as_initiator {
                (
                    hello.nonce,
                    peer_hello.nonce,
                    hello.identity_pub,
                    peer_hello.identity_pub,
                )
            } else {
                (
                    peer_hello.nonce,
                    hello.nonce,
                    peer_hello.identity_pub,
                    hello.identity_pub,
                )
            };

            // 签名 + AUTH（含本方向 nonce 前缀）
            let signing_bytes =
                handshake_signing_bytes(&ipub, &rpub, &nonce_a, &nonce_b, as_initiator);
            let sig = signing.sign(&signing_bytes).to_bytes();
            let self_prefix = crypto::random_prefix();
            let auth = AuthFrame {
                signature: sig,
                nonce_prefix: self_prefix,
            };
            write_frame(&mut stream, &auth.encode()).await?;

            // 读对端 AUTH
            let peer_auth_raw = read_frame(&mut stream).await?;
            let peer_auth = AuthFrame::decode(&peer_auth_raw).map_err(SessionError::Io)?;
            // 验证对端签名（覆盖同域、对端角色）
            let peer_signing_bytes =
                handshake_signing_bytes(&ipub, &rpub, &nonce_a, &nonce_b, !as_initiator);
            let peer_sig = ed25519_dalek::Signature::from_bytes(&peer_auth.signature);
            peer_vk
                .verify_strict(&peer_signing_bytes, &peer_sig)
                .map_err(|e| SessionError::Io(format!("peer signature invalid: {e}")))?;

            // 密钥派生：IKM = X25519(self_x_secret, peer_x_pub)
            let ikm = crypto::x25519_shared_secret(&x_secret, &peer_hello.x25519_pub)
                .map_err(SessionError::Io)?;
            let SessionKeys {
                c2s_key,
                s2c_key,
                session_id,
            } = derive_keys(
                &ikm,
                &nonce_a,
                &nonce_b,
                &peer_id_a_str(as_initiator, self_peer_id, &peer_hello.peer_id),
                &peer_id_b_str(as_initiator, self_peer_id, &peer_hello.peer_id),
            );
            // 本端发送密钥：发起方 → c2s；应答方 → s2c
            let (tx_key, rx_key, peer_prefix) = if as_initiator {
                (c2s_key, s2c_key, peer_auth.nonce_prefix)
            } else {
                (s2c_key, c2s_key, peer_auth.nonce_prefix)
            };
            let session = SecureSession {
                tx: Channel::new(tx_key, self_prefix),
                rx: Channel::new(rx_key, peer_prefix),
                is_initiator: as_initiator,
                stream,
                session_id,
            };
            Ok((
                session,
                PeerIdentity {
                    peer_id: peer_hello.peer_id,
                    fingerprint: crypto::fingerprint_of(&peer_vk),
                },
            ))
        };
        tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake)
            .await
            .map_err(|_| SessionError::Io("handshake timeout".into()))?
    }

    /// 发送加密应用帧（JSON 载荷，AAD=会话 ID）。
    pub async fn send_json<T: serde::Serialize>(&mut self, value: &T) -> Result<(), SessionError> {
        let plaintext = serde_json::to_vec(value).map_err(|e| SessionError::Io(e.to_string()))?;
        self.send_raw(&plaintext, &plain_aad(&self.session_id))
            .await
    }

    /// 接收加密应用帧并反序列化（JSON 载荷）。
    pub async fn recv_json<T: serde::de::DeserializeOwned>(&mut self) -> Result<T, SessionError> {
        let plaintext = self.recv_raw(&plain_aad(&self.session_id)).await?;
        serde_json::from_slice(&plaintext)
            .map_err(|e| SessionError::Io(format!("bad frame json: {e}")))
    }

    /// 发送 Chunk（AAD 绑定 fileIndex+seq；counter 递增）。
    pub async fn send_chunk(
        &mut self,
        file_index: u32,
        seq: u32,
        data: &[u8],
    ) -> Result<(), SessionError> {
        let aad = chunk_aad(&self.session_id, file_index, seq);
        self.send_raw(data, &aad).await
    }

    /// 接收 Chunk（AAD 绑定校验在 recv_raw；调用方按 fileIndex/seq 构造 AAD）。
    pub async fn recv_chunk(&mut self, file_index: u32, seq: u32) -> Result<Vec<u8>, SessionError> {
        let aad = chunk_aad(&self.session_id, file_index, seq);
        self.recv_raw(&aad).await
    }

    /// 发送 Cancel（独立加密帧，cancel_aad 域；尽力送达，失败仅日志不阻塞本地清理）。
    pub async fn send_cancel(&mut self, reason: &str) {
        let frame = super::proto::CancelFrame {
            reason: reason.to_string(),
        };
        if let Ok(plaintext) = serde_json::to_vec(&frame) {
            if let Err(e) = self
                .send_raw(&plaintext, &cancel_aad(&self.session_id))
                .await
            {
                log::debug!("lan_file: cancel frame send failed (best effort): {e}");
            }
        }
        let _ = self.stream.shutdown().await;
    }

    /// 尝试接收 Cancel（对端显式取消侦测：收到任意帧后先按 cancel 域试解）。
    ///
    /// 调用时机：任务进行中读到「非期望 Chunk」时判定对端是否发了取消。
    /// 解开即返回 Some(reason)；解开但非 Cancel 语义 / 解不开返回 None（正常数据路径按 AAD 域区分）。
    pub fn try_decode_cancel(plaintext: &[u8]) -> Option<String> {
        serde_json::from_slice::<super::proto::CancelFrame>(plaintext)
            .ok()
            .map(|c| c.reason)
    }

    async fn send_raw(&mut self, plaintext: &[u8], aad: &[u8]) -> Result<(), SessionError> {
        let counter = self.tx.counter;
        self.tx.counter = self.tx.counter.wrapping_add(1);
        let nonce = self.tx.nonce(counter);
        let ct = self
            .tx
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|e| SessionError::Io(format!("seal: {e}")))?;
        write_frame(&mut self.stream, &ct).await
    }

    async fn recv_raw(&mut self, aad: &[u8]) -> Result<Vec<u8>, SessionError> {
        // 计数器由帧序保证（TCP 有序）；失败即断连（上层重建）
        let ct = read_frame(&mut self.stream).await?;
        let counter = self.rx.counter;
        self.rx.counter = self.rx.counter.wrapping_add(1);
        let nonce = self.rx.nonce(counter);
        self.rx
            .cipher
            .decrypt(&nonce, Payload { msg: &ct, aad })
            .map_err(|e| SessionError::Io(format!("open: {e}")))
    }

    pub async fn shutdown(&mut self) {
        let _ = self.stream.shutdown().await;
    }

    pub fn is_initiator(&self) -> bool {
        self.is_initiator
    }
}

struct SessionKeys {
    c2s_key: [u8; 32],
    s2c_key: [u8; 32],
    session_id: [u8; SESSION_ID_LEN],
}

fn derive_keys(
    ikm: &[u8],
    nonce_a: &[u8; 16],
    nonce_b: &[u8; 16],
    peer_a: &str,
    peer_b: &str,
) -> SessionKeys {
    let (c2s, s2c, sid) = crypto::derive_session_keys(ikm, nonce_a, nonce_b, peer_a, peer_b);
    SessionKeys {
        c2s_key: c2s,
        s2c_key: s2c,
        session_id: sid,
    }
}

fn peer_id_a_str(as_initiator: bool, self_peer: &str, peer: &str) -> String {
    if as_initiator {
        self_peer.to_string()
    } else {
        peer.to_string()
    }
}

fn peer_id_b_str(as_initiator: bool, self_peer: &str, peer: &str) -> String {
    if as_initiator {
        peer.to_string()
    } else {
        self_peer.to_string()
    }
}
