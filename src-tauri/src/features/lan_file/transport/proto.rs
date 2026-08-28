//! VLF/1 帧编解码（纯逻辑，契约 `docs/api/lan-file.md` 5.5）。
//!
//! 传输帧统一 `[u32 BE 帧长][密文载荷]`；**帧长先校验再分配**（上限 1 MiB，
//! 超限立即报错断连）。握手为明文（身份签名自证），应用帧为 AEAD 密文。
//!
//! 握手序列（双向，10s 总超时由 IO 层控制）：
//! ```text
//! initiator → HELLO { magic, version, role=initiator, peerId, pubEd, pubX, nonce(16B) }
//! responder → HELLO { magic, version, role=responder, peerId, pubEd, pubX, nonce(16B) }
//! initiator → AUTH { sig_i }（覆盖 magic+ver+双方公钥+双方 nonce+role_i）
//! responder → AUTH { sig_r }（覆盖同域 role_r）
//! 然后 IO 层用双方 nonce/公钥派生密钥，并交换 AEAD 前缀（AUTH 帧内附带 8B 前缀）
//! ```
//!
//! 应用帧（加密后载荷 JSON 或文件字节，AAD 绑定会话 ID）：
//! Offer / ResumeHint 在 Offer 内 / Accept / Chunk / End / Ack / Cancel。

use super::crypto::{handshake_signing_bytes, MAGIC, NONCE_PREFIX_LEN, SESSION_ID_LEN, VERSION};

/// 帧长上限（1 MiB，契约 5.5：先校验再分配）。
pub const MAX_FRAME_LEN: u32 = 1024 * 1024;
/// 握手总超时（契约 5.5）。
pub const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// TCP 连接建立超时。
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// 提议响应超时（60s，契约 5.4）。
pub const OFFER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// 断线续传宽限窗口（120s，契约 5.5）。
pub const RESUME_WINDOW: std::time::Duration = std::time::Duration::from_secs(120);
/// 正文分帧大小（≤1 MiB）。
pub const CHUNK_SIZE: usize = 256 * 1024;

// ---------------------------------------------------------------------------
// 握手帧（明文）
// ---------------------------------------------------------------------------

/// HELLO 帧（明文）：双方互发。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloFrame {
    /// 发送方 peerId（base58 multihash）。
    pub peer_id: String,
    /// ed25519 长期公钥（32B）。
    pub identity_pub: [u8; 32],
    /// X25519 临时公钥（32B）。
    pub x25519_pub: [u8; 32],
    /// 会话 nonce（16B；发起方先生成、应答方拼接）。
    pub nonce: [u8; 16],
}

impl HelloFrame {
    /// 序列化：magic(4) + version(1) + role(1) + peerLen(u16) + peer + pubEd(32) + pubX(32) + nonce(16)。
    pub fn encode(&self, role_tag: u8) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4 + 1 + 1 + 2 + self.peer_id.len() + 32 + 32 + 16);
        buf.extend_from_slice(MAGIC);
        buf.push(VERSION);
        buf.push(role_tag);
        buf.extend_from_slice(&(self.peer_id.len() as u16).to_be_bytes());
        buf.extend_from_slice(self.peer_id.as_bytes());
        buf.extend_from_slice(&self.identity_pub);
        buf.extend_from_slice(&self.x25519_pub);
        buf.extend_from_slice(&self.nonce);
        buf
    }

    /// 反序列化（role_tag 由调用方按期望方向校验）。
    pub fn decode(data: &[u8]) -> Result<(u8, Self), String> {
        let mut cur = Cursor::new(data);
        let magic: [u8; 4] = cur.read_array()?;
        if &magic != MAGIC {
            return Err("bad magic".into());
        }
        let version = cur.read_u8()?;
        if version != VERSION {
            return Err(format!("unsupported version {version}"));
        }
        let role = cur.read_u8()?;
        let peer_len = cur.read_u16()? as usize;
        if peer_len > 128 {
            return Err("peerId too long".into());
        }
        let peer_bytes = cur.read_bytes(peer_len)?;
        let peer_id = String::from_utf8(peer_bytes.to_vec()).map_err(|_| "peerId not utf8")?;
        let identity_pub: [u8; 32] = cur.read_array()?;
        let x25519_pub: [u8; 32] = cur.read_array()?;
        let nonce: [u8; 16] = cur.read_array()?;
        Ok((
            role,
            HelloFrame {
                peer_id,
                identity_pub,
                x25519_pub,
                nonce,
            },
        ))
    }
}

/// AUTH 帧（明文）：签名 + AEAD nonce 前缀交换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthFrame {
    /// 本方 ed25519 签名（覆盖握手域）。
    pub signature: [u8; 64],
    /// 本方 AEAD nonce 随机前缀（8B，供对端解密本方向密文）。
    pub nonce_prefix: [u8; NONCE_PREFIX_LEN],
}

impl AuthFrame {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64 + NONCE_PREFIX_LEN);
        buf.extend_from_slice(&self.signature);
        buf.extend_from_slice(&self.nonce_prefix);
        buf
    }

    pub fn decode(data: &[u8]) -> Result<Self, String> {
        if data.len() != 64 + NONCE_PREFIX_LEN {
            return Err("auth frame size".into());
        }
        let mut sig = [0u8; 64];
        sig.copy_from_slice(&data[..64]);
        let mut prefix = [0u8; NONCE_PREFIX_LEN];
        prefix.copy_from_slice(&data[64..]);
        Ok(AuthFrame {
            signature: sig,
            nonce_prefix: prefix,
        })
    }
}

/// 构造本方握手签名覆盖域并返回待签字节。
pub fn signing_bytes_for(
    initiator_pub: &[u8; 32],
    responder_pub: &[u8; 32],
    nonce_a: &[u8; 16],
    nonce_b: &[u8; 16],
    as_initiator: bool,
) -> Vec<u8> {
    handshake_signing_bytes(
        initiator_pub,
        responder_pub,
        nonce_a,
        nonce_b,
        !as_initiator,
    )
}

// ---------------------------------------------------------------------------
// 应用帧语义（加密载荷为 JSON / 文件字节，由会话层编解码）
// ---------------------------------------------------------------------------

/// Initial 帧：发起方 → 接收方（JSON）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "t", rename_all_fields = "camelCase")]
pub enum InitialFrame {
    /// 新任务提议。
    Offer {
        transfer_id: String,
        files: Vec<OfferFile>,
        total_bytes: u64,
    },
    /// 续传重连（同 transferId；接收方查 sidecar 决定 Accept{resume} 或 Reject）。
    ResumeOffer {
        transfer_id: String,
        files: Vec<OfferFile>,
        total_bytes: u64,
    },
    /// 自动图片通道（契约 5.7-6：同 5.5 数据面，Initial 帧 variant kind:"image"；
    /// 接收端免确认落盘，静默语义，不占交互会话槽）。
    ImageOffer {
        transfer_id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        size: u64,
        /// 图片字节 SHA-256 hex（关联键）。
        hash: String,
    },
}

/// 提议中的单文件。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OfferFile {
    pub name: String,
    pub size: u64,
}

/// 应答帧：接收方 → 发起方（JSON）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "t", rename_all_fields = "camelCase")]
pub enum ReplyFrame {
    /// 接受（fresh = 从头收；resume = 从各文件已收偏移续收）。
    Accept {
        fresh: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        per_file_received_bytes: Option<Vec<u64>>,
    },
    /// 拒绝（code = 稳定错误码，如 `lan_file.busy` / `lan_file.disk_full` / `lan_file.cancelled`）。
    Reject { code: String },
}

/// End 帧：每文件 SHA-256（hex），文件序（JSON）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EndFrame {
    pub sha256: Vec<String>,
}

/// Ack 帧：接收方对 End 的对账结论（JSON）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AckFrame {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// 取消帧（JSON，独立加密帧类型；seq 取 u32::MAX 语义位，AAD 含专门标记避免与数据帧混淆）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CancelFrame {
    pub reason: String,
}

/// Chunk AAD：`session_id ‖ fileIndex u32BE ‖ seq u32BE`。
pub fn chunk_aad(session_id: &[u8; SESSION_ID_LEN], file_index: u32, seq: u32) -> Vec<u8> {
    let mut aad = Vec::with_capacity(SESSION_ID_LEN + 8);
    aad.extend_from_slice(session_id);
    aad.extend_from_slice(&file_index.to_be_bytes());
    aad.extend_from_slice(&seq.to_be_bytes());
    aad
}

/// 控制帧 AAD（Cancel 专用）：`session_id ‖ 0xFFFFFFFF ‖ 0xFFFFFFFF`——
/// 与任何真实 Chunk(fileIndex<2^32-1) 的 AAD 域不重叠，防取消帧被误当数据块。
pub fn cancel_aad(session_id: &[u8; SESSION_ID_LEN]) -> Vec<u8> {
    chunk_aad(session_id, u32::MAX, u32::MAX)
}

/// 普通帧 AAD：仅会话 ID。
pub fn plain_aad(session_id: &[u8; SESSION_ID_LEN]) -> Vec<u8> {
    session_id.to_vec()
}

// ---------------------------------------------------------------------------
// 内部游标
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err("frame truncated".into());
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, String> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let b = self.read_bytes(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(b);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_roundtrip_both_roles() {
        let hello = HelloFrame {
            peer_id: "12D3KooWTestPeerId".into(),
            identity_pub: [1; 32],
            x25519_pub: [2; 32],
            nonce: [3; 16],
        };
        for role in [0u8, 1u8] {
            let bytes = hello.encode(role);
            let (decoded_role, decoded) = HelloFrame::decode(&bytes).unwrap();
            assert_eq!(decoded_role, role);
            assert_eq!(decoded, hello);
        }
    }

    #[test]
    fn hello_rejects_bad_magic_version_and_truncation() {
        let hello = HelloFrame {
            peer_id: "p".into(),
            identity_pub: [0; 32],
            x25519_pub: [0; 32],
            nonce: [0; 16],
        };
        let mut bytes = hello.encode(0);
        bytes[0] = b'X';
        assert!(HelloFrame::decode(&bytes).is_err(), "bad magic");

        let mut bytes = hello.encode(0);
        bytes[4] = 99; // version
        assert!(HelloFrame::decode(&bytes).is_err(), "bad version");

        let bytes = hello.encode(0);
        assert!(
            HelloFrame::decode(&bytes[..bytes.len() - 1]).is_err(),
            "truncated"
        );
        // peerId 超长
        let long = HelloFrame {
            peer_id: "x".repeat(200),
            identity_pub: [0; 32],
            x25519_pub: [0; 32],
            nonce: [0; 16],
        };
        assert!(HelloFrame::decode(&long.encode(0)).is_err());
    }

    #[test]
    fn auth_roundtrip_and_size_check() {
        let auth = AuthFrame {
            signature: [7; 64],
            nonce_prefix: [9; NONCE_PREFIX_LEN],
        };
        let decoded = AuthFrame::decode(&auth.encode()).unwrap();
        assert_eq!(decoded, auth);
        assert!(AuthFrame::decode(&[0u8; 10]).is_err());
    }

    #[test]
    fn signing_domain_differs_by_role() {
        let b_i = signing_bytes_for(&[1; 32], &[2; 32], &[3; 16], &[4; 16], true);
        let b_r = signing_bytes_for(&[1; 32], &[2; 32], &[3; 16], &[4; 16], false);
        assert_ne!(b_i, b_r);
    }

    #[test]
    fn chunk_aad_binds_file_and_seq() {
        let sid = [5u8; SESSION_ID_LEN];
        let a1 = chunk_aad(&sid, 0, 0);
        let a2 = chunk_aad(&sid, 0, 1);
        let a3 = chunk_aad(&sid, 1, 0);
        assert_ne!(a1, a2);
        assert_ne!(a1, a3);
        assert_eq!(a1.len(), SESSION_ID_LEN + 8);
        assert_eq!(plain_aad(&sid).len(), SESSION_ID_LEN);
    }

    #[test]
    fn json_frames_roundtrip() {
        /// 序列化为字节（测试辅助）。
        fn json_bytes<T: serde::Serialize>(v: &T) -> Vec<u8> {
            serde_json::to_vec(v).unwrap()
        }
        let initial = InitialFrame::Offer {
            transfer_id: "t-1".into(),
            files: vec![OfferFile {
                name: "a.txt".into(),
                size: 3,
            }],
            total_bytes: 3,
        };
        let json = serde_json::to_vec(&initial).unwrap();
        let back: InitialFrame = serde_json::from_slice(&json).unwrap();
        assert_eq!(back, initial);

        let resume = InitialFrame::ResumeOffer {
            transfer_id: "t-1".into(),
            files: vec![],
            total_bytes: 0,
        };
        let json = serde_json::to_vec(&resume).unwrap();
        assert!(matches!(
            serde_json::from_slice::<InitialFrame>(&json).unwrap(),
            InitialFrame::ResumeOffer { .. }
        ));

        // ImageOffer（契约 5.7-6）：字段 camelCase
        let img = InitialFrame::ImageOffer {
            transfer_id: "img-1".into(),
            name: "shot.png".into(),
            width: Some(1920),
            height: Some(1080),
            size: 2048,
            hash: "e3b0c4".into(),
        };
        let jsonv = serde_json::to_value(&img).unwrap();
        assert_eq!(jsonv["t"], "ImageOffer");
        assert_eq!(jsonv["name"], "shot.png");
        assert_eq!(jsonv["hash"], "e3b0c4");
        let jsonb = json_bytes(&img);
        assert_eq!(serde_json::from_slice::<InitialFrame>(&jsonb).unwrap(), img);

        let accept = ReplyFrame::Accept {
            fresh: false,
            per_file_received_bytes: Some(vec![1, 2]),
        };
        let json = serde_json::to_vec(&accept).unwrap();
        assert_eq!(serde_json::from_slice::<ReplyFrame>(&json).unwrap(), accept);

        let reject = ReplyFrame::Reject {
            code: "lan_file.busy".into(),
        };
        let json = serde_json::to_vec(&reject).unwrap();
        assert_eq!(serde_json::from_slice::<ReplyFrame>(&json).unwrap(), reject);

        let end = EndFrame {
            sha256: vec!["ab".into()],
        };
        let json = serde_json::to_vec(&end).unwrap();
        assert_eq!(serde_json::from_slice::<EndFrame>(&json).unwrap(), end);

        let ack = AckFrame {
            ok: false,
            code: Some("lan_file.integrity_mismatch".into()),
        };
        let json = serde_json::to_vec(&ack).unwrap();
        assert_eq!(serde_json::from_slice::<AckFrame>(&json).unwrap(), ack);

        let cancel = CancelFrame {
            reason: "user".into(),
        };
        let json = serde_json::to_vec(&cancel).unwrap();
        assert_eq!(
            serde_json::from_slice::<CancelFrame>(&json).unwrap(),
            cancel
        );
    }

    #[test]
    fn frame_len_constant_matches_contract() {
        assert_eq!(MAX_FRAME_LEN, 1024 * 1024);
        assert_eq!(HANDSHAKE_TIMEOUT.as_secs(), 10);
        assert_eq!(CONNECT_TIMEOUT.as_secs(), 5);
        assert_eq!(OFFER_TIMEOUT.as_secs(), 60);
        assert_eq!(RESUME_WINDOW.as_secs(), 120);
        assert!(CHUNK_SIZE <= MAX_FRAME_LEN as usize);
    }
}
