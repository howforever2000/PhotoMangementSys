//! Tauri API 的最小桩（详见同级 Cargo.toml 的说明）。
//!
//! 只需要「类型约束正确」，不需要行为正确 —— 目的是让
//! `src-tauri/src/edit/commands.rs` 作为普通 Rust 代码被完整类型检查。

use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;

pub use tauri_stub_macros::command;

/// 与真 Tauri 同构的 managed-state 句柄。
#[derive(Debug)]
pub struct State<'a, T: Send + Sync + 'static>(&'a T);

impl<'a, T: Send + Sync + 'static> State<'a, T> {
    #[allow(dead_code)]
    pub fn new(v: &'a T) -> Self {
        Self(v)
    }
}

impl<'a, T: Send + Sync + 'static> Deref for State<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0
    }
}

/// 裸二进制 IPC 返回体（真 Tauri 同样用裸字节而非 base64）。
///
/// ⚠️ 本桩**不**模拟真 Tauri 的 `IpcResponse` trait 约束（真 Tauri 里
/// `Response` 与所有 `Serialize` 类型都满足它，但**元组不满足** —— v1 的
/// `impl_tuple!` 在 v2 已移除）。因此本桩对返回类型一律放行，
/// 「返回 `(Response, Meta)`」这类错误在验证壳里查不出来，
/// 只会在真 crate 编译时暴露为 E0599 `async_kind`。
/// 这是桩与真 API 的已知差异，改桩需一并处理 `stub-macros` 的恒等宏。
#[derive(Debug)]
pub struct Response {
    #[allow(dead_code)]
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(body: Vec<u8>) -> Self {
        Self { body }
    }
}

pub mod ipc {
    pub use super::Response;
}

/// 任务被取消（真 tokio 的 `JoinError`）。
#[derive(Debug)]
pub struct JoinError(pub String);

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for JoinError {}

/// 异步运行时（真 Tauri 里包的是 tokio）。
///
/// ⚠️ `spawn_blocking` 必须返回 `Result<R, JoinError>`（与 tokio 的
/// `JoinHandle` 一致），否则调用点少写一个 `?` 也能通过编译，
/// 而在真 crate 里会报错 —— 桩与真 API 的签名一旦不同，
/// 整个「编译检查」就失去意义。
pub mod async_runtime {
    use super::{Future, JoinError, Pin};

    pub fn spawn_blocking<F, R>(f: F) -> Pin<Box<dyn Future<Output = Result<R, JoinError>> + Send>>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        Box::pin(async move {
            // 模拟 tokio：把 panic 收敛成 JoinError，而不是让它穿透 future
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                Ok(v) => Ok(v),
                Err(_) => Err(JoinError("任务 panic".into())),
            }
        })
    }
}

#[allow(dead_code)]
fn _assert_send() {
    fn is_send<T: Send>() {}
    is_send::<Pin<Box<dyn Future<Output = ()> + Send>>>();
    // spawn_blocking 的返回 future 必须可跨线程（tokio 要求）
    is_send::<Pin<Box<dyn Future<Output = Result<(), JoinError>> + Send>>>();
}
