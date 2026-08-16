//! 节点身份：ed25519 keypair 的持久化加载/创建。
//!
//! peerId 是终端在局域网中的稳定身份（**不依赖 IP**，局域网 IP 可变）。
//! 首次启动生成并落盘；文件缺失/损坏时重新生成（记录日志）。
//! 契约：`docs/api/lan-sync.md` 第 5.1 节。

use libp2p::identity::Keypair;
use std::fs;
use std::path::Path;

/// 从磁盘加载身份；不存在或损坏时生成新身份并落盘。
///
/// 返回 `(keypair, 是否新建)`。落盘失败不阻断启动（仅日志），
/// 身份仅在本次运行有效（下次重新生成）。
pub fn load_or_create(path: &Path) -> (Keypair, bool) {
    if let Ok(bytes) = fs::read(path) {
        match Keypair::from_protobuf_encoding(&bytes) {
            Ok(kp) => {
                log::debug!("identity: loaded existing keypair from {}", path.display());
                return (kp, false);
            }
            Err(e) => {
                log::warn!(
                    "identity: corrupt keypair at {}, regenerating: {e}",
                    path.display()
                );
            }
        }
    }
    let kp = Keypair::generate_ed25519();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    match kp.to_protobuf_encoding() {
        Ok(bytes) => match fs::write(path, bytes) {
            Ok(()) => log::debug!("identity: generated new keypair at {}", path.display()),
            Err(e) => log::error!("identity: failed to persist keypair: {e}"),
        },
        Err(e) => log::error!("identity: failed to encode keypair: {e}"),
    }
    (kp, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_or_create_creates_then_loads_same() {
        let dir = std::env::temp_dir().join(format!("vitrytool-id-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("peer-key.json");
        let (kp1, created) = load_or_create(&path);
        assert!(created);
        let (kp2, created2) = load_or_create(&path);
        assert!(!created2);
        assert_eq!(kp1.public().to_peer_id(), kp2.public().to_peer_id());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_file_regenerates() {
        let dir = std::env::temp_dir().join(format!("vitrytool-id-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("peer-key.json");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, b"not a valid keypair").unwrap();
        let (kp, created) = load_or_create(&path);
        assert!(created);
        assert_eq!(kp.public().to_peer_id().to_base58().len(), 52); // 12D3Koo... 52 字符
        let _ = fs::remove_dir_all(dir);
    }
}
