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

// ---------------------------------------------------------------------------
// 旧标识符绝对路径修复（BUG-2026-1021-005）
// ---------------------------------------------------------------------------

/// 把 `photos.db` 里仍指向**旧标识符目录**的绝对路径改写到新目录，
/// 并顺手修 `batch_calibration.json` 里记的标定路径。
///
/// # 为什么光迁目录不够
///
/// `migrate_legacy_data_dir` 只搬运文件。但库里存的是**绝对路径**：
/// `albums.cover_path`、`photo_thumb_cache.thumb_path`（实测 130 + 24924 行）。
/// 目录改名之后这些字符串原地变成死链 —— 文件就在新目录里好好的，
/// 前端 `convertFileSrc()` 却拿到一个不存在的路径，结果就是**所有相册封面全部碎图**
/// （alt=「封面」+ 碎图标），而相册名/张数/容量一切正常，很容易被当成“缩略图功能坏了”。
///
/// # 为什么必须在每次启动都跑（而不是只在迁移分支里跑）
///
/// 已经装过旧版的用户，“目录已迁完”那一刻就再也不会进迁移分支了，
/// 而库里那些绝对路径还是旧的。所以本函数：
/// 1. 先用两张已知路径列做**便宜探针**（两行 SQL，常见情况微秒级返回）；
/// 2. 命中才做全库扫描（跳过 virtual table）。
/// 结果是幂等的：修过一次之后再探就查不到旧前缀，只剩下那次探针开销。
///
/// 必须在应用自己打开 `photos.db` 之前调用（同 `migrate_legacy_data_dir` 的位置）。
pub fn repair_legacy_data_paths(app_data_root: &Path, new_dir: &Path) -> String {
    let db_path = new_dir.join("photos.db");
    if !db_path.is_file() {
        return "无 photos.db，跳过旧路径修复".to_string();
    }
    let legacy_roots: Vec<PathBuf> = LEGACY_APP_DIRS
        .iter()
        .map(|name| app_data_root.join(name))
        .filter(|p| p != new_dir)
        .collect();
    if legacy_roots.is_empty() {
        return "无需修复旧路径".to_string();
    }

    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return format!("旧路径修复跳过：无法打开 photos.db（{e}）"),
    };

    // 1) 便宜探针：只有这两列存绝对路径。连表/列不存在也是可能的（全新库），全部容错。
    let mut hit = false;
    'probe: for (table, column) in [("albums", "cover_path"), ("photo_thumb_cache", "thumb_path")] {
        for old in &legacy_roots {
            let sql = format!("SELECT 1 FROM \"{table}\" WHERE \"{column}\" LIKE ?1 LIMIT 1");
            if let Ok(mut stmt) = conn.prepare(&sql) {
                let pattern = format!("%{}%", old.display());
                if stmt.exists(rusqlite::params![pattern]).unwrap_or(false) {
                    hit = true;
                    break 'probe;
                }
            }
        }
    }
    if !hit {
        // 标定文件独立于库，单独处理（不存在/不可写都静默跳过）
        let cal = repair_calibration_path(new_dir, &legacy_roots);
        return match cal {
            Some(n) => n,
            None => "旧数据目录路径已全部对齐（无需修复）".to_string(),
        };
    }

    // 2) 全库扫描：对每张表的每个文本列做一次前缀替换。
    //    不用手写表名列清单 —— 将来新增一张带路径的表也不需要回来改这里。
    let new_root = new_dir.display().to_string();
    let mut total = 0usize;
    let mut detail: Vec<String> = Vec::new();

    let mut tables: Vec<(String, String)> = Vec::new();
    if let Ok(mut stmt) =
        conn.prepare("SELECT name, COALESCE(sql, '') FROM sqlite_master WHERE type = 'table'")
    {
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
            tables.extend(rows.flatten());
        }
    }

    for (table, sql) in tables {
        // FTS 等虚表不能直接 UPDATE，且它们存的是文本副本、不含路径
        if sql.to_uppercase().contains("VIRTUAL TABLE") {
            continue;
        }
        let mut columns: Vec<String> = Vec::new();
        if let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info(\"{table}\")")) {
            if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) {
                columns.extend(rows.flatten());
            }
        }
        for column in columns {
            for old in &legacy_roots {
                let old_root = old.display().to_string();
                let sql = format!(
                    "UPDATE \"{table}\" SET \"{column}\" = replace(\"{column}\", ?1, ?2) \
                     WHERE \"{column}\" LIKE ?3"
                );
                let Ok(mut stmt) = conn.prepare(&sql) else {
                    continue;
                };
                let pattern = format!("%{old_root}%");
                match stmt.execute(rusqlite::params![old_root, new_root, pattern]) {
                    Ok(n) if n > 0 => {
                        total += n;
                        detail.push(format!("{table}.{column}:{n}"));
                    }
                    _ => {}
                }
            }
        }
    }

    let mut note = if total == 0 {
        "旧数据目录路径已全部对齐（无需修复）".to_string()
    } else {
        format!("已修复 {total} 行指向旧数据目录的绝对路径（{}）", detail.join("、"))
    };
    if let Some(cal) = repair_calibration_path(new_dir, &legacy_roots) {
        note.push_str("；");
        note.push_str(&cal);
    }
    note
}

/// 修 `batch_calibration.json` 里记的标定样本目录（纯文本前缀替换）。
///
/// 该文件只记录“本次标定用的缩略图目录”，路径失效不影响启动；
/// 但留着旧路径会让人误以为标定还对着一个新跑过的目录，所以一并抹干净。
fn repair_calibration_path(new_dir: &Path, legacy_roots: &[PathBuf]) -> Option<String> {
    let path = new_dir.join("batch_calibration.json");
    let text = std::fs::read_to_string(&path).ok()?;
    let mut fixed = text.clone();
    for old in legacy_roots {
        // JSON 里的反斜杠是转义的，两种写法都替一遍
        let old_raw = old.display().to_string();
        fixed = fixed.replace(&old_raw, &new_dir.display().to_string());
        let old_json = old_raw.replace('\\', "\\\\");
        if old_json != old_raw {
            fixed = fixed.replace(&old_json, &new_dir.display().to_string().replace('\\', "\\\\"));
        }
    }
    if fixed == text {
        return None;
    }
    std::fs::write(&path, fixed).ok()?;
    Some("batch_calibration.json 的标定路径已改写".to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧标识符目录改名后，库里的绝对路径要跟着改到新目录，而且改完必须真能打开。
    #[test]
    fn repair_legacy_data_paths_rewrites_db_and_calibration() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("pm-devpath-{}-{stamp}", std::process::id()));
        let new_dir = root.join("com.photomanagementsys.app");
        let legacy_dir = root.join("com.haoyuan.photo-management-sys");
        std::fs::create_dir_all(new_dir.join("thumbs")).unwrap();
        std::fs::create_dir_all(&legacy_dir).unwrap();

        // 文件已经在**新**目录，库里却还写着**旧**绝对路径
        let thumb = new_dir.join("thumbs").join("a.webp");
        std::fs::write(&thumb, b"x").unwrap();
        let old_thumb = legacy_dir.join("thumbs").join("a.webp").display().to_string();

        let db = new_dir.join("photos.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE albums(id INTEGER PRIMARY KEY, cover_path TEXT);
                 CREATE TABLE photo_thumb_cache(thumb_path TEXT);
                 CREATE TABLE unrelated(txt TEXT);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO albums(cover_path) VALUES (?1)",
                rusqlite::params![old_thumb],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO photo_thumb_cache(thumb_path) VALUES (?1)",
                rusqlite::params![old_thumb],
            )
            .unwrap();
            conn.execute("INSERT INTO unrelated(txt) VALUES ('hello')", [])
                .unwrap();
        }
        std::fs::write(
            new_dir.join("batch_calibration.json"),
            format!("{{\"path\": {old_thumb:?}}}"),
        )
        .unwrap();

        let note = repair_legacy_data_paths(&root, &new_dir);
        assert!(note.contains("albums.cover_path:1"), "note = {note}");
        assert!(
            note.contains("photo_thumb_cache.thumb_path:1"),
            "note = {note}"
        );

        let conn = rusqlite::Connection::open(&db).unwrap();
        let p: String = conn
            .query_row("SELECT cover_path FROM albums", [], |r| r.get(0))
            .unwrap();
        assert_eq!(p, thumb.display().to_string());
        assert!(
            std::path::Path::new(&p).exists(),
            "改写后必须指向真实存在的文件: {p}"
        );
        let hit: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM photo_thumb_cache WHERE thumb_path LIKE '%com.haoyuan%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 0, "缩略图缓存表也要改干净");
        drop(conn);

        let cal = std::fs::read_to_string(new_dir.join("batch_calibration.json")).unwrap();
        assert!(
            cal.contains("com.photomanagementsys.app"),
            "标定文件路径未改写: {cal}"
        );

        // 幂等：再跑一次不应再命中任何行
        let note2 = repair_legacy_data_paths(&root, &new_dir);
        assert!(note2.contains("无需修复"), "note2 = {note2}");

        std::fs::remove_dir_all(&root).ok();
    }
}