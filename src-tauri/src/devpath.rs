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

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// 一次性数据目录迁移（BUG-2026-1014-001）
// ---------------------------------------------------------------------------

/// 历史标识符（曾含作者真实姓名，会随安装落到每台用户机的 %APPDATA% 下）
const LEGACY_APP_DIRS: &[&str] = &["com.haoyuan.photo-management-sys"];

/// 把旧标识符目录下的数据整体迁到新目录，返回给人看的日志片段。
///
/// # 为什么必须做
///
/// `app_data_dir()` 由标识符推导。改标识符后，Windows 上数据目录从
/// `%APPDATA%\com.haoyuan.photo-management-sys` 变成 `%APPDATA%\com.photomanagementsys.app`，
/// 老用户的 `photos.db` / `thumbs/` / `avatars/` / `vcr-data/`（人物库）/
/// **`app.key`（敏感字段加密密钥）** 会被"原地消失"——应用照常启动、相册却是空的，
/// 且密钥缺失会让历史加密字段无法解密。这是最容易被当成"数据丢了"的坑。
///
/// # 迁移策略（安全优先）
///
/// 1. **新目录已存在且非空** → 不动。老数据留在原处（用户可能回退过版本），
///    只提示位置，绝不覆盖或删除任何东西。
/// 2. **新目录不存在/为空** → 先 `rename`（同卷，瞬间完成）；
/// 3. rename 失败（跨卷、或文件被占用）→ 退化为**递归复制**；
/// 4. **只有复制成功才删旧目录**，复制失败则原样保留旧数据 —— 任何一步
///    都不允许出现"数据既没迁走又被删掉"的状态。
///
/// 调用时机：必须在 `logger::init` 与任何 `app_data_dir()` 消费之前
/// （见 lib.rs::setup 顶部），否则日志会写进新目录、而数据还在旧目录。
pub fn migrate_legacy_data_dir(app_data_root: &Path, new_dir: &Path) -> String {
    // 新目录已有内容 → 认为已迁移（或用户回退过版本），保持现状不动手。
    if let Ok(rd) = std::fs::read_dir(new_dir) {
        if rd.count() > 0 {
            let legacy_hit = LEGACY_APP_DIRS
                .iter()
                .map(|n| app_data_root.join(n))
                .find(|p| p.is_dir());
            return match legacy_hit {
                Some(old) => format!(
                    "数据目录已就绪；检测到旧目录仍存在（{}），未改动（确认不再回退旧版本后可自行删除）",
                    old.display()
                ),
                None => "数据目录已就绪，无需迁移".to_string(),
            };
        }
    }

    let old_dir = LEGACY_APP_DIRS
        .iter()
        .map(|n| app_data_root.join(n))
        .find(|p| p.is_dir());
    let Some(old) = old_dir else {
        return "无旧数据目录，首次启动".to_string();
    };

    // 1) 优先 rename：同一 %APPDATA% 卷内是原子的、瞬间完成
    if let Some(parent) = new_dir.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return format!("创建数据目录失败（{e}），旧数据保持原位：{}", old.display());
        }
    }
    match std::fs::rename(&old, new_dir) {
        Ok(()) => {
            return format!(
                "已迁移数据目录 {} → {}（重命名完成）",
                old.display(),
                new_dir.display()
            )
        }
        Err(rename_err) => {
            // 2) 退化递归复制（跨卷 / 文件占用）
            if let Err(copy_err) = copy_dir_recursive(&old, new_dir) {
                return format!(
                    "数据迁移失败：重命名({rename_err})、复制({copy_err}) 均未成功。\
                     旧数据完好保留于 {}，请手动把该目录内容复制到 {}",
                    old.display(),
                    new_dir.display()
                );
            }
            // 3) 复制成功才删旧目录；删不掉只是留个冗余副本，不影响使用
            let removed = std::fs::remove_dir_all(&old).is_ok();
            format!(
                "已迁移数据目录 {} → {}（复制完成{}）",
                old.display(),
                new_dir.display(),
                if removed { "，旧目录已清理" } else { "，旧目录残留可手动删除" }
            )
        }
    }
}

/// 递归复制目录（文件 → 覆盖写，子目录 → 建好再递归）
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("创建目录失败: {e}"))?;
    let rd = std::fs::read_dir(src).map_err(|e| format!("读取源目录失败: {e}"))?;
    for entry in rd {
        let entry = entry.map_err(|e| format!("读取目录项失败: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("读取文件类型失败: {e}"))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| format!("复制 {:?} 失败: {e}", from.file_name()))?;
        }
    }
    Ok(())
}

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