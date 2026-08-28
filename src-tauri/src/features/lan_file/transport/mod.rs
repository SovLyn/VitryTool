//! lan-file 传输协议原语（纯逻辑 + TCP 会话层）。
//!
//! 契约：`docs/api/lan-file.md` 5.5（VLF/1）。拆分：
//! - `crypto.rs`：身份签名校验 + X25519 → HKDF-SHA256 双向密钥 + ChaCha20-Poly1305 分帧 AEAD；
//! - `proto.rs`：握手帧序列化/反序列化 + 应用帧编解码（帧长先校验再分配，上限 1 MiB）；
//! - `session.rs`：单连接握手 + 加密收发（任务级语义归 state.rs 任务管理器）。
//!
//! 注：crypto/proto 由 dt 直接消费；session 由 state.rs 消费，
//! 阶段性未消费告警以模块级 allow 抑制（crypto/proto 已有 dt，接线后移除 session 的 allow）。

#![allow(dead_code)]

pub mod crypto;
pub mod proto;
pub mod session;
