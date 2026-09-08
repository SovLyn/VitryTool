//! lan-file 开发者测试（dt）。
//!
//! 覆盖（契约 `docs/api/lan-file.md`「测试要点」）：
//! - 会话层回环集成（tokio 回环双实例端到端）：握手 + 加密帧往返 + AAD 篡改拒绝；
//! - sidecar 读写往返 / 墓碑语义 / 窗口拦截；
//! - 状态机快照序列化形状。
//!
//! 纯逻辑测试分布在 `store.rs` / `service.rs` / `transport/*` 内嵌模块（32 + 17 用例）。

use super::service::{LanFileOffer, OfferFileInfo, TransferState};
use super::store::{
    load_sidecar, remove_sidecar, save_sidecar, LanFileSettings, SidecarFile, SidecarMeta,
};
use std::fs;
use std::time::Duration;

// ---------------------------------------------------------------------------
// sidecar 往返与墓碑（契约 5.5）
// ---------------------------------------------------------------------------

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("vitry-lanfile-dt-{tag}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample_sidecar(id: &str) -> SidecarMeta {
    SidecarMeta {
        transfer_id: id.into(),
        direction: "receive".into(),
        peer_id: "peerA".into(),
        files: vec![
            SidecarFile {
                name: "a.bin".into(),
                tmp_name: ".t-a.bin.tmp".into(),
                received_bytes: 4096,
            },
            SidecarFile {
                name: "b.bin".into(),
                tmp_name: ".t-b.bin.tmp".into(),
                received_bytes: 1024 * 1024,
            },
        ],
        cancelled: None,
    }
}

#[test]
fn sidecar_roundtrip_and_missing() {
    let dir = tmp_dir("sidecar");
    // 不存在 → None
    assert!(load_sidecar(&dir, "t1").unwrap().is_none());
    // 写读往返
    let meta = sample_sidecar("t1");
    save_sidecar(&dir, &meta).unwrap();
    let loaded = load_sidecar(&dir, "t1").unwrap().unwrap();
    assert_eq!(loaded, meta);
    assert!(!loaded.is_cancelled());
    // 文件名形如 .t1.meta.json
    assert!(dir.join(".t1.meta.json").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn sidecar_cancelled_tombstone_blocks_resume() {
    let dir = tmp_dir("tombstone");
    let mut meta = sample_sidecar("t2");
    save_sidecar(&dir, &meta).unwrap();
    // 用户显式取消 → 写墓碑 + 删 tmp（此处只验证墓碑语义）
    meta.cancelled = Some(true);
    save_sidecar(&dir, &meta).unwrap();
    let loaded = load_sidecar(&dir, "t2").unwrap().unwrap();
    assert!(loaded.is_cancelled(), "墓碑侧必须拒绝续传");
    // 契约：墓碑保留至窗口自然过期，期间对端重连一律 Reject{cancelled}
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn sidecar_remove_cleans() {
    let dir = tmp_dir("remove");
    let meta = sample_sidecar("t3");
    save_sidecar(&dir, &meta).unwrap();
    remove_sidecar(&dir, "t3").unwrap();
    assert!(load_sidecar(&dir, "t3").unwrap().is_none());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_window_gates_sidecar() {
    // sidecar 存在但超窗 → 视同失效（state 层清理）；窗口内 → Accept{resume}
    let last = 1_000u64;
    assert!(super::service::resume_window_active(last, last + 119_999));
    assert!(!super::service::resume_window_active(last, last + 120_000));
}

// ---------------------------------------------------------------------------
// 会话层回环集成（tokio 回环双实例端到端）
// ---------------------------------------------------------------------------

/// 回环：一对 TCP 连接两端跑完整握手，返回双方会话。
async fn handshake_loopback() -> (
    super::transport::session::SecureSession,
    super::transport::session::PeerIdentity,
    super::transport::session::SecureSession,
    super::transport::session::PeerIdentity,
) {
    use super::transport::session::SecureSession;
    use ed25519_dalek::SigningKey;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let ikey = SigningKey::from_bytes(&[1u8; 32]);
    let rkey = SigningKey::from_bytes(&[2u8; 32]);
    let ipub = super::transport::crypto::peer_id_from_ed25519(&ikey.verifying_key());
    let rpub = super::transport::crypto::peer_id_from_ed25519(&rkey.verifying_key());

    let ikey_c = ikey;
    let rkey_c = rkey;
    let (i_res, r_res) = tokio::join!(
        async move { SecureSession::connect(&addr, &ikey_c, &ipub).await },
        async move {
            let (stream, _) = listener.accept().await.unwrap();
            SecureSession::accept(stream, &rkey_c, &rpub).await
        },
    );
    let (i_sess, i_peer) = i_res.expect("initiator handshake");
    let (r_sess, r_peer) = r_res.expect("responder handshake");
    (i_sess, i_peer, r_sess, r_peer)
}

#[tokio::test]
async fn session_handshake_binds_identity_both_ways() {
    let (i, i_peer, r, r_peer) = handshake_loopback().await;
    // 双方看到的对端身份正确
    assert_eq!(
        i_peer.peer_id,
        super::transport::crypto::peer_id_from_ed25519(
            &ed25519_dalek::SigningKey::from_bytes(&[2u8; 32]).verifying_key()
        )
    );
    assert_eq!(
        r_peer.peer_id,
        super::transport::crypto::peer_id_from_ed25519(
            &ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]).verifying_key()
        )
    );
    assert!(i_peer.fingerprint.starts_with("SHA256:"));
    assert!(!r.is_initiator());
    assert!(i.is_initiator());
}

// ---------------------------------------------------------------------------
// 帧读取的取消安全（真机实测 bug：200ms 分片轮询取消 read_exact → 帧流失步）
// ---------------------------------------------------------------------------

/// 复现：读侧在帧只到达一部分时被 `timeout` 取消；补齐后必须仍能读到完整帧。
///
/// 旧实现（`read_exact`）会在此丢字节：长度头被消费、载荷前半丢失，下一帧长度字段
/// 读到载荷中间的随机字节 → `frame too large`（真机实测值 2626586369），传输必然失败。
#[tokio::test]
async fn buffered_frame_read_survives_cancellation() {
    use super::transport::proto::MAX_FRAME_LEN;
    use super::transport::session::read_frame_buffered;
    use tokio::io::AsyncWriteExt;
    use tokio::net::{TcpListener, TcpStream};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (writer, (mut reader, _)) = tokio::join!(
        async move { TcpStream::connect(addr).await.unwrap() },
        async move { listener.accept().await.unwrap() },
    );
    let mut writer = writer;

    let payload = vec![0xABu8; 4096];
    // 先写长度头 + 前 1KiB 载荷（模拟 TCP 分片：帧在途但未完整）
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .unwrap();
    writer.write_all(&payload[..1024]).await.unwrap();
    writer.flush().await.unwrap();

    // 读侧分片等待被取消（已消费的字节必须留在缓冲里）
    let mut buf = Vec::new();
    let cancelled = tokio::time::timeout(
        Duration::from_millis(150),
        read_frame_buffered(&mut reader, &mut buf),
    )
    .await;
    assert!(cancelled.is_err(), "分片等待应超时取消");
    assert!(!buf.is_empty(), "已读字节必须保留在缓冲");

    // 补齐剩余载荷 → 再读必须拿到完整帧
    writer.write_all(&payload[1024..]).await.unwrap();
    writer.flush().await.unwrap();
    let frame = read_frame_buffered(&mut reader, &mut buf).await.unwrap();
    assert_eq!(frame, payload, "取消后仍须完整读出同一帧");

    // 超长长度仍被拒绝（先校验再分配）
    writer
        .write_all(&(MAX_FRAME_LEN + 1).to_be_bytes())
        .await
        .unwrap();
    writer.flush().await.unwrap();
    let err = read_frame_buffered(&mut reader, &mut buf).await;
    assert!(err.is_err());
}

// ---------------------------------------------------------------------------
// 对端取消帧识别（真机实测 bug：Cancel 帧被当成断线 → 不写墓碑、留 .tmp、转 resuming）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn peer_cancel_frame_detected_in_json_read() {
    use super::transport::session::SessionError;
    let (mut i, _ip, mut r, _rp) = handshake_loopback().await;
    // 发起方发 Cancel（独立 cancel_aad 域）
    i.send_cancel("user").await;
    // 应答方按普通 JSON 域读：必须识别为「对端取消」，而不是 IO 错误
    let err = r
        .recv_json::<super::transport::proto::ReplyFrame>()
        .await
        .unwrap_err();
    assert!(
        matches!(err, SessionError::Cancelled(_)),
        "Cancel 帧应识别为对端取消，实际 {err:?}"
    );
}

#[tokio::test]
async fn peer_cancel_frame_detected_in_chunk_read() {
    use super::transport::session::SessionError;
    let (mut i, _ip, mut r, _rp) = handshake_loopback().await;
    i.send_cancel("user").await;
    // 接收方正在等 Chunk（chunk AAD 域）→ 同样必须识别为对端取消
    let err = r.recv_chunk(0, 0).await.unwrap_err();
    assert!(matches!(err, SessionError::Cancelled(_)));
}

#[tokio::test]
async fn session_json_frames_roundtrip() {
    let (mut i, _ip, mut r, _rp) = handshake_loopback().await;
    // Initial Offer 往返
    let offer = super::transport::proto::InitialFrame::Offer {
        transfer_id: "t-1".into(),
        files: vec![super::transport::proto::OfferFile {
            name: "a.bin".into(),
            size: 5,
        }],
        total_bytes: 5,
    };
    i.send_json(&offer).await.unwrap();
    let got: super::transport::proto::InitialFrame = r.recv_json().await.unwrap();
    assert_eq!(got, offer);

    // Reply Accept 往返
    let reply = super::transport::proto::ReplyFrame::Accept {
        fresh: true,
        per_file_received_bytes: None,
    };
    r.send_json(&reply).await.unwrap();
    let got: super::transport::proto::ReplyFrame = i.recv_json().await.unwrap();
    assert_eq!(got, reply);

    // End/Ack/Cancel 往返
    let end = super::transport::proto::EndFrame {
        sha256: vec!["ab".into()],
    };
    i.send_json(&end).await.unwrap();
    let got: super::transport::proto::EndFrame = r.recv_json().await.unwrap();
    assert_eq!(got, end);
    let ack = super::transport::proto::AckFrame {
        ok: true,
        code: None,
    };
    r.send_json(&ack).await.unwrap();
    let got: super::transport::proto::AckFrame = i.recv_json().await.unwrap();
    assert!(got.ok);
    let cancel = super::transport::proto::CancelFrame {
        reason: "user".into(),
    };
    i.send_json(&cancel).await.unwrap();
    let got: super::transport::proto::CancelFrame = r.recv_json().await.unwrap();
    assert_eq!(got.reason, "user");
}

#[tokio::test]
async fn session_chunks_and_tampered_aad_rejected() {
    let (mut i, _ip, mut r, _rp) = handshake_loopback().await;
    // 正常 Chunk：发起方 → 应答方（c2s 方向）
    let payload: Vec<u8> = (0..CHUNK_TEST_LEN).map(|i| (i % 251) as u8).collect();
    i.send_chunk(0, 0, &payload).await.unwrap();
    let got = r.recv_chunk(0, 0).await.unwrap();
    assert_eq!(got, payload);

    // AAD 篡改（fileIndex 不同）→ AEAD open 失败 → 连接报错
    i.send_chunk(0, 1, &payload).await.unwrap();
    let bad = r.recv_chunk(1, 1).await;
    assert!(bad.is_err(), "AAD 绑定 fileIndex+seq，错位必须解不开");
    // 连接已被对端判废：后续帧不可靠，直接收尾
    let _ = i.shutdown().await;
    let _ = r.shutdown().await;
}
const CHUNK_TEST_LEN: usize = 1024 * 512 + 7; // >1 chunk 大小的一半，接近但不超帧上限

#[tokio::test]
async fn session_multiple_chunks_keep_counter() {
    let (mut i, _ip, mut r, _rp) = handshake_loopback().await;
    // 多帧连续：counter 逐帧递增，乱序/重放不可解
    for seq in 0..3u32 {
        let data = vec![seq as u8; 100];
        i.send_chunk(2, seq, &data).await.unwrap();
    }
    for seq in 0..3u32 {
        let got = r.recv_chunk(2, seq).await.unwrap();
        assert_eq!(got, vec![seq as u8; 100]);
    }
}

#[tokio::test]
async fn session_rejects_wrong_peer_identity() {
    // 假冒者：宣称 peerId 与其公钥不绑定 → 握手失败
    use super::transport::session::SecureSession;
    use ed25519_dalek::SigningKey;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let fake = SigningKey::from_bytes(&[9u8; 32]);
    let real = SigningKey::from_bytes(&[2u8; 32]);
    let real_peer = super::transport::crypto::peer_id_from_ed25519(&real.verifying_key());
    let real_peer_for_r = real_peer.clone();
    // 假冒者顶 real 的 peerId，但用自己的密钥 → 绑定校验失败
    let (i_res, r_res) = tokio::join!(
        async move { SecureSession::connect(&addr, &fake, &real_peer).await },
        async move {
            let (stream, _) = listener.accept().await.unwrap();
            SecureSession::accept(stream, &real, &real_peer_for_r).await
        },
    );
    assert!(
        i_res.is_err() || r_res.is_err(),
        "至少一侧必须拒绝不绑定身份"
    );
}

#[tokio::test]
async fn session_end_to_end_file_transfer_loopback() {
    // 端到端：多文件串行 + 逐文件哈希对账（在会话层模拟 Offer→Accept→Chunk→End→Ack）
    let (mut i, _ip, mut r, _rp) = handshake_loopback().await;
    let files = vec![
        super::transport::proto::OfferFile {
            name: "a.bin".into(),
            size: CHUNK_TEST_LEN as u64,
        },
        super::transport::proto::OfferFile {
            name: "b.txt".into(),
            size: 11,
        },
    ];
    let total: u64 = files.iter().map(|f| f.size).sum();
    let offer = super::transport::proto::InitialFrame::Offer {
        transfer_id: "t-e2e".into(),
        files: files.clone(),
        total_bytes: total,
    };
    i.send_json(&offer).await.unwrap();
    let got: super::transport::proto::InitialFrame = r.recv_json().await.unwrap();
    assert_eq!(got, offer);

    // 接收方接受
    r.send_json(&super::transport::proto::ReplyFrame::Accept {
        fresh: true,
        per_file_received_bytes: None,
    })
    .await
    .unwrap();
    let accept: super::transport::proto::ReplyFrame = i.recv_json().await.unwrap();
    assert!(matches!(
        accept,
        super::transport::proto::ReplyFrame::Accept { fresh: true, .. }
    ));

    // 逐文件：分帧发送 → 接收侧重哈希
    for (idx, f) in files.iter().enumerate() {
        let data = vec![(idx + 1) as u8; f.size as usize];
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let hex = hex_encode(&hasher.finalize());
        let mut sent = 0u64;
        let mut seq = 0u32;
        while sent < f.size {
            let end = ((sent as usize) + CHUNK_SIZE_TEST).min(data.len());
            let slice = &data[sent as usize..end];
            i.send_chunk(idx as u32, seq, slice).await.unwrap();
            let got = r.recv_chunk(idx as u32, seq).await.unwrap();
            assert_eq!(got, slice);
            sent = end as u64;
            seq += 1;
        }
        i.send_json(&super::transport::proto::EndFrame { sha256: vec![hex] })
            .await
            .unwrap();
        let end_frame: super::transport::proto::EndFrame = r.recv_json().await.unwrap();
        // 接收方对账（收到的数据即发送数据，重哈希一致）
        let expect = vec![(idx + 1) as u8; f.size as usize];
        let mut h2 = Sha256::new();
        h2.update(&expect);
        assert_eq!(end_frame.sha256[0], hex_encode(&h2.finalize()));
        r.send_json(&super::transport::proto::AckFrame {
            ok: true,
            code: None,
        })
        .await
        .unwrap();
        let ack: super::transport::proto::AckFrame = i.recv_json().await.unwrap();
        assert!(ack.ok);
    }
}
const CHUNK_SIZE_TEST: usize = 256 * 1024;

// ---------------------------------------------------------------------------
// Offer 快照形状（LanFileOffer 序列化）
// ---------------------------------------------------------------------------

#[test]
fn offer_snapshot_serializes_name_clash() {
    let offer = LanFileOffer {
        transfer_id: "t1".into(),
        peer_id: "peerX".into(),
        terminal_name: "SOVLYN".into(),
        fingerprint: "SHA256:abc".into(),
        name_clash: true,
        files: vec![OfferFileInfo {
            name: "a.txt".into(),
            size: 3,
        }],
        total_bytes: 3,
        known_from_minutes: 42,
    };
    let json = serde_json::to_value(&offer).unwrap();
    assert_eq!(json["nameClash"], true);
    assert_eq!(json["knownFromMinutes"], 42);
    assert_eq!(json["files"][0]["name"], "a.txt");
}

#[test]
fn transfer_state_serializes_seven_states() {
    let cases = [
        (TransferState::Offering, "\"offering\""),
        (TransferState::Transferring, "\"transferring\""),
        (TransferState::Resuming, "\"resuming\""),
        (TransferState::Done, "\"done\""),
        (TransferState::Failed, "\"failed\""),
        (TransferState::Cancelled, "\"cancelled\""),
        (TransferState::Rejected, "\"rejected\""),
    ];
    for (state, expect) in cases {
        assert_eq!(serde_json::to_string(&state).unwrap(), expect);
    }
}

#[test]
fn settings_store_roundtrip_file_shape() {
    // 模拟 lan-file.json 的 settings 键值形状（tauri-plugin-store 内 {settings: {...}}）
    let mut s = LanFileSettings::default();
    s.trust("peerA", "SOVLYN", "2026-08-20T00:00:00Z".into());
    let wrapped = serde_json::json!({ "settings": s });
    let text = serde_json::to_string(&wrapped).unwrap();
    let back: serde_json::Value = serde_json::from_str(&text).unwrap();
    let s2: LanFileSettings = serde_json::from_value(back["settings"].clone()).unwrap();
    assert!(s2.is_trusted("peerA"));
    assert_eq!(s2.trusted_peers[0].terminal_name, "SOVLYN");
}

#[tokio::test]
async fn transfer_resume_offsets_computation() {
    // 续传：接收方报 per_file_received_bytes，发送方 seek 跳过
    let sidecar = sample_sidecar("t9");
    let offsets: Vec<u64> = sidecar.files.iter().map(|f| f.received_bytes).collect();
    assert_eq!(offsets, vec![4096, 1024 * 1024]);
    // Accept{resume} 帧携带 offsets
    let accept = super::transport::proto::ReplyFrame::Accept {
        fresh: false,
        per_file_received_bytes: Some(offsets),
    };
    let json = serde_json::to_value(&accept).unwrap();
    assert_eq!(json["fresh"], false);
    assert_eq!(json["perFileReceivedBytes"][0], 4096);
}

#[tokio::test]
async fn backoff_retry_within_window() {
    // 120s 窗口内退避重试累计时长不超过窗口（1+2+4+8+16+32 = 63s < 120s）
    let total: u64 = super::service::RETRY_BACKOFF_SECS.iter().sum();
    assert_eq!(total, 63);
    assert!(Duration::from_secs(total) < super::service::RESUME_WINDOW);
}

// ---------------------------------------------------------------------------
// multiaddr → IP 解析（数据面 dial 地址来源；曾因提前 return 空串导致整条链路失效）
// ---------------------------------------------------------------------------

#[test]
fn multiaddr_ip_extraction() {
    // 标准 libp2p multiaddr：tcp / udp-quic 两种形态（本机实测 mdns discovered 输出）
    assert_eq!(
        super::state::ip_from_multiaddr("/ip4/192.168.31.203/tcp/38583/p2p/12D3KooW"),
        "192.168.31.203"
    );
    assert_eq!(
        super::state::ip_from_multiaddr("/ip4/192.168.31.203/udp/52981/quic-v1/p2p/12D3KooW"),
        "192.168.31.203"
    );
    // 无协议前缀 / 空 / 非 ip4/ip6 → 空串
    assert_eq!(super::state::ip_from_multiaddr(""), "");
    assert_eq!(super::state::ip_from_multiaddr("/tcp/12345"), "");
    assert_eq!(super::state::ip_from_multiaddr("12D3KooW"), "");
    // ip6
    assert_eq!(super::state::ip_from_multiaddr("/ip6/::1/tcp/12345"), "::1");
}

// ---------------------------------------------------------------------------
// 续传对齐（真机实测 bug：`.tmp` 比 sidecar 记录更长 → append 错位 → 哈希对账失败）
// ---------------------------------------------------------------------------

#[test]
fn resume_truncate_aligns_tmp_to_sidecar_offset() {
    use std::io::Write;
    let chunk = super::transport::proto::CHUNK_SIZE;
    // 源文件 = 3 块可辨识内容
    let src: Vec<u8> = (0..chunk * 3).map(|i| (i % 251) as u8).collect();
    let dir = tmp_dir("resume-align");
    let tmp = dir.join(".t.0.tmp");
    // 模拟异常中断：.tmp 已写到 1.5 块，而 sidecar 只记录到 1 块
    fs::write(&tmp, &src[..chunk + chunk / 2]).unwrap();
    let recorded = chunk as u64;

    // 续传对齐：截断到记录偏移
    let f = fs::OpenOptions::new().write(true).open(&tmp).unwrap();
    f.set_len(recorded).unwrap();
    drop(f);

    // 发送方从 recorded 继续 → 接收方 append
    let mut out = fs::OpenOptions::new().append(true).open(&tmp).unwrap();
    out.write_all(&src[recorded as usize..]).unwrap();
    out.flush().unwrap();
    drop(out);

    // 结果必须与源文件逐字节一致（对齐成功）
    assert_eq!(fs::read(&tmp).unwrap(), src);
    let _ = fs::remove_dir_all(dir);
}

use super::store::hex_encode;
use sha2::{Digest as _, Sha256};

// ---------------------------------------------------------------------------
// 待决提议表（accept/reject 命令的送达通道；曾因从不注册导致「接受」必报 peer_not_found）
// ---------------------------------------------------------------------------

#[test]
fn pending_offer_registry_delivers_decision() {
    use super::state::{
        pending_offer_peer, register_pending_offer, resolve_offer, unregister_pending_offer,
        TaskCommand,
    };
    let (tx, rx) = std::sync::mpsc::channel::<TaskCommand>();
    register_pending_offer("t-accept", "peerA", "SOVLYN", tx);
    assert_eq!(
        pending_offer_peer("t-accept"),
        Some(("peerA".to_string(), "SOVLYN".to_string()))
    );
    // 命令层查表 → 转发决定 → 任务侧收到 Accept
    resolve_offer("t-accept", TaskCommand::Accept);
    assert!(matches!(rx.recv().unwrap(), TaskCommand::Accept));
    // 已消费：表项移除，重复 resolve 不 panic 也不重复投递
    assert!(pending_offer_peer("t-accept").is_none());
    resolve_offer("t-accept", TaskCommand::Reject);
    assert!(rx.try_recv().is_err());

    // Reject / Cancel 同样送达
    let (tx2, rx2) = std::sync::mpsc::channel::<TaskCommand>();
    register_pending_offer("t-reject", "peerB", "OTHER", tx2);
    resolve_offer("t-reject", TaskCommand::Reject);
    assert!(matches!(rx2.recv().unwrap(), TaskCommand::Reject));
    let (tx3, rx3) = std::sync::mpsc::channel::<TaskCommand>();
    register_pending_offer("t-cancel", "peerC", "THIRD", tx3);
    resolve_offer("t-cancel", TaskCommand::Cancel);
    assert!(matches!(rx3.recv().unwrap(), TaskCommand::Cancel));

    // 兜底清理（超时 / 任务结束）
    let (tx4, _rx4) = std::sync::mpsc::channel::<TaskCommand>();
    register_pending_offer("t-stale", "peerD", "STALE", tx4);
    unregister_pending_offer("t-stale");
    assert!(pending_offer_peer("t-stale").is_none());
    // 未知 transferId 查表为空（命令层据此返回 lan_file.peer_not_found）
    assert!(pending_offer_peer("nope").is_none());
}

// ---------------------------------------------------------------------------
// 剪贴板新鲜观察表（移动端图片通道门控第二层证据，契约 5.9）
// ---------------------------------------------------------------------------

#[test]
fn clipboard_peer_freshness_tracking() {
    use super::state::{clipboard_peer_fresh, note_clipboard_peer};
    assert!(!clipboard_peer_fresh("peer-never-seen"));
    note_clipboard_peer("peer-seen");
    assert!(clipboard_peer_fresh("peer-seen"));
    assert!(!clipboard_peer_fresh("peer-other"));
}
