//! Tauri 的最小 proc-macro 桩（仅供 `edit-verify` 编译检查使用）。
//!
//! `#[command]` 是**恒等**属性：保留函数体与签名不变，只是剥掉标记。
//! 这让 `commands.rs` 能作为普通 `async fn` 被类型检查 ——
//! 而类型检查恰恰是这一层最需要的（字段名、借用、`Send` 边界）。

use proc_macro::TokenStream;

/// `#[tauri::command]` 的恒等桩。
#[proc_macro_attribute]
pub fn command(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// `generate_handler![...]` 桩：展开为空（真 crate 里生成 handler 表，
/// 桩里不关心），但**参数列表会被检查语法**。
#[proc_macro]
pub fn generate_handler(input: TokenStream) -> TokenStream {
    let mut out = TokenStream::new();
    out.extend(std::iter::once(proc_macro::TokenTree::Group(
        proc_macro::Group::new(proc_macro::Delimiter::None, input),
    )));
    out
}
