//! 开发态源码路径 —— 全仓**唯一**允许出现 `env!("CARGO_MANIFEST_DIR")` 的地方
//!
//! # 为什么必须收口（BUG-2026-1013-001）
//!
//! `env!("CARGO_MANIFEST_DIR")` 是**编译期宏**：它在 `cargo build` 时就被展开成
//! 构建机的绝对路径并写进可执行文件。因此只要 release 构建里有任何一处引用它
//! （哪怕藏在永不执行的分支里），构建机的用户名与目录结构就会永久留在二进制中。
//!
//! 实测症状：把安装包发给同事，对方打开「⚙ 开发者视角 → 数据与路径」，看到
//! `VCR 模型目录 = D:\YUAN HAO\Documents\PersonalIdeaProject\PhotoMangementSys\python\models`
//! —— 直接暴露了作者本机用户名与目录；安装不完整时还会以错误信息形式弹出
//! `创意工坊服务脚本不存在: D:\YUAN HAO\...`。
//!
//! # 纪律
//!
//! 1. 生产代码一律**不得**直接写 `env!("CARGO_MANIFEST_DIR")`，只能调 [`dev_project_dir`]。
//! 2. `env!` 必须待在 `#[cfg(debug_assertions)]` **属性**块里 —— 属性在宏展开**之前**
//!    剥离代码，release 构建根本不会展开它，字符串也就不会进二进制。
//! 3. ⚠️ 不可写成 `if cfg!(debug_assertions) { ... env!(...) ... }`：`cfg!` 只是运行时
//!    布尔常量，`env!` 照样在编译期展开并把路径写进去，等于没做。
//! 4. release 下 [`dev_project_dir`] 返回**空** `PathBuf`，调用方据此给出不含路径的
//!    通用报错，而不是回落到构建机路径。
//!
//! 测试代码不受此限：`#[cfg(test)]` 同样在 release 前剥离。

use std::path::PathBuf;

/// 源码项目根目录（`src-tauri` 的上一级）。
///
/// - **debug 构建**：构建机上的源码根，正是开发期需要的位置；
/// - **release 构建**：空路径。调用方必须自己判断并给出不含任何本机路径的报错。
///
/// 空路径不是「猜一个地方」，而是明确的信号：安装版本就不该依赖源码目录。
pub fn dev_project_dir() -> PathBuf {
    #[cfg(debug_assertions)]
    {
        // 只有 debug 构建才会展开下面这行宏；release 下整个块被 cfg 剥离。
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        manifest
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| manifest.to_path_buf())
    }
    #[cfg(not(debug_assertions))]
    {
        PathBuf::new()
    }
}

/// 是否处于「源码开发布局」（debug 构建且能定位到项目根）。
///
/// release 恒为 `false`。
pub fn has_dev_layout() -> bool {
    !dev_project_dir().as_os_str().is_empty()
}

/// 开发版组件缺失时的报错文案。
///
/// release 下**绝不**拼接任何路径 —— 缺失就是缺失，说清楚「重新安装」即可；
/// 早期版本在这里回落到编译期路径，等于把构建机用户名发给每一位用户
/// （BUG-2026-1013-001）。
pub fn dev_component_missing_err(component: &str, install_subpath: &str) -> String {
    if has_dev_layout() {
        let p = dev_project_dir().join(component);
        format!("{component} 不存在: {}", p.display())
    } else {
        format!("未找到内置组件（{install_subpath}），请重新安装完整安装包")
    }
}