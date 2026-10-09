//! 配方的持久化与导入导出
//! ====================================================================
//! ## 存哪：同目录 sidecar，而不是数据库
//!
//! 选择 sidecar（`<原文件名>.pmrecipe.json`）而非入库：
//!
//! - **可移植**。用户拷走照片目录，调色结果一起走。存进 `photos.db` 的配方
//!   在换机器、拷目录、或者用户只是想把这个文件夹发给朋友时全部丢失。
//! - **不侵入主库**。`photos.db` 的 schema 与迁移链已经很重
//!   （见 `db/mod.rs`），为编辑配方再加一张表会让每次迁移的风险面变大。
//! - **冲突天然可见**。文件在资源管理器里，看得见摸得着。
//!
//! 代价是「同一张图在不同相册里引用同一份配方」会共享 —— 这正是想要的语义。
//!
//! ## 写入必须原子
//!
//! sidecar 是**用户资产**。写到一半断电/崩溃会留下一个半截 JSON，
//! 之后这张图就再也打不开了。因此一律「写临时文件 → fsync → rename」：
//! `rename` 在同一文件系统内是原子操作，读者要么看到旧文件要么看到新文件。
//! 直接 `fs::write` 覆盖是最常见的配方损坏原因。

use std::path::{Path, PathBuf};

use super::recipe::AdjustmentRecipe;

/// sidecar 扩展名。
pub const RECIPE_EXT: &str = "pmrecipe.json";

/// 由原图路径推出 sidecar 路径。
///
/// `D:\Pics\a.jpg` → `D:\Pics\a.jpg.pmrecipe.json`
///
/// 用「原名 + 扩展名」而不是替换扩展名，是为了**不与真实的同名文件冲突**：
/// `a.jpg` 与 `a.png` 在同一目录时，两个配方必须能共存。
pub fn sidecar_path(source: &Path) -> PathBuf {
    let mut name = source
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(".");
    name.push(RECIPE_EXT);
    source.with_file_name(name)
}

/// 同目录的临时文件名（原子写的中转站）。
///
/// 必须是 `Path::with_extension` 之外的字符串拼接：`with_extension` 会替换
/// 最后一个点之后的内容，用在这里会把 `a.jpg.pmrecipe.json` 变成
/// `a.jpg.pmrecipe.pmtmprecipe.json` —— 每次保存都在文件名里再叠一层。
fn temp_path_for(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    target.with_file_name(name)
}

/// 配方存盘错误。
#[derive(Debug)]
pub enum StoreError {
    Io(String),
    Format(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "写入配方文件失败：{e}"),
            StoreError::Format(e) => write!(f, "配方内容损坏：{e}"),
        }
    }
}

/// sidecar 是否存在。
pub fn exists(source: &Path) -> bool {
    sidecar_path(source).is_file()
}

/// 读取 sidecar。不存在返回 `Ok(None)`（= 用户还没调过）。
pub fn load(source: &Path) -> Result<Option<AdjustmentRecipe>, StoreError> {
    let p = sidecar_path(source);
    if !p.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&p).map_err(|e| StoreError::Io(e.to_string()))?;
    // 文件存在但内容坏了：返回错误而不是当成「没调过」。
    // 静默回落到空配方会让用户以为自己调的东西丢了 —— 这是很糟的体验。
    AdjustmentRecipe::from_json(&text)
        .map(Some)
        .map_err(StoreError::Format)
}

/// 原子写入 sidecar。
pub fn save(source: &Path, recipe: &AdjustmentRecipe) -> Result<PathBuf, StoreError> {
    let target = sidecar_path(source);
    let json = recipe.to_json().map_err(StoreError::Format)?;

    // 同目录的临时文件：必须与目标同文件系统，rename 才是原子的。
    let tmp = temp_path_for(&target);

    // 写入 + fsync：确保数据真正落盘后才 rename。
    // 少了 fsync 的话，rename 可能先于数据落盘，断电后仍会得到空文件。
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).map_err(|e| StoreError::Io(e.to_string()))?;
        f.write_all(json.as_bytes())
            .map_err(|e| StoreError::Io(e.to_string()))?;
        f.sync_all().map_err(|e| StoreError::Io(e.to_string()))?;
    }

    std::fs::rename(&tmp, &target).map_err(|e| {
        // 失败时清掉临时文件，否则目录里会留一堆 .tmp 垃圾
        let _ = std::fs::remove_file(&tmp);
        StoreError::Io(e.to_string())
    })?;
    Ok(target)
}

/// 删除 sidecar（「重置为原图」= 删掉配方文件）。
///
/// 找不到文件不算错误：重置操作是幂等的。
pub fn remove(source: &Path) -> Result<(), StoreError> {
    let p = sidecar_path(source);
    if !p.exists() {
        return Ok(());
    }
    std::fs::remove_file(&p).map_err(|e| StoreError::Io(e.to_string()))
}

/// 从 JSON 文本导入（粘贴导入用）。
pub fn import_text(text: &str) -> Result<AdjustmentRecipe, StoreError> {
    AdjustmentRecipe::from_json(text).map_err(StoreError::Format)
}

/// 导出为 JSON 文本（剪贴板导出用）。
pub fn export_text(recipe: &AdjustmentRecipe) -> Result<String, StoreError> {
    recipe.to_json().map_err(StoreError::Format)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pm-edit-store-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sidecar_appends_extension_without_removing_original() {
        let p = sidecar_path(Path::new("/x/a.jpg"));
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name, "a.jpg.pmrecipe.json");
        assert!(p.to_string_lossy().ends_with(RECIPE_EXT));
    }

    /// jpg 与 png 同名时两份配方必须互不覆盖 —— 这是「追加而非替换」的原因。
    #[test]
    fn sidecar_distinguishes_same_basename_different_ext() {
        let a = sidecar_path(Path::new("/x/a.jpg"));
        let b = sidecar_path(Path::new("/x/a.png"));
        assert_ne!(a, b);
    }

    #[test]
    fn save_then_load_roundtrip() {
        let d = tmpdir("rt");
        let src = d.join("pic.jpg");
        std::fs::write(&src, b"not a real jpeg").unwrap();

        let mut r = AdjustmentRecipe::default();
        r.basic.exposure = 1.75;
        r.basic.shadows = -0.3;
        r.curve_luma
            .points
            .push(super::super::recipe::CurvePoint::new(0.1, 0.2));
        r.curve_luma
            .points
            .push(super::super::recipe::CurvePoint::new(0.9, 0.8));

        let path = save(&src, &r).unwrap();
        assert!(path.is_file());
        let back = load(&src).unwrap().expect("应能读回");
        assert_eq!(back, r);

        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn missing_sidecar_loads_as_none() {
        let d = tmpdir("missing");
        let src = d.join("nope.jpg");
        assert!(!exists(&src));
        assert!(load(&src).unwrap().is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn remove_is_idempotent() {
        let d = tmpdir("rm");
        let src = d.join("a.jpg");
        std::fs::write(&src, b"x").unwrap();
        remove(&src).unwrap();
        // 第二次不该报错
        remove(&src).unwrap();
        std::fs::remove_dir_all(&d).ok();
    }

    /// 损坏的 sidecar 必须报错，不能静默变成「没调过」。
    /// 静默吞掉会让用户以为自己调的东西丢了。
    #[test]
    fn corrupt_sidecar_reports_error() {
        let d = tmpdir("corrupt");
        let src = d.join("b.jpg");
        std::fs::write(&src, b"x").unwrap();
        std::fs::write(sidecar_path(&src), b"{ this is not json ").unwrap();

        let e = load(&src).unwrap_err();
        assert!(
            matches!(e, StoreError::Format(_)),
            "应报格式错误，实得 {e:?}"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// 原子写：成功后不应留下 .tmp 残留。
    #[test]
    fn save_leaves_no_temp_file() {
        let d = tmpdir("tmpclean");
        let src = d.join("c.jpg");
        std::fs::write(&src, b"x").unwrap();
        save(&src, &AdjustmentRecipe::default()).unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件 {leftovers:?}");
        std::fs::remove_dir_all(&d).ok();
    }

    /// 旧格式 sidecar（字段缺失）必须能被新版读出来。
    #[test]
    fn legacy_sidecar_with_few_fields_loads() {
        let d = tmpdir("legacy");
        let src = d.join("d.jpg");
        std::fs::write(&src, b"x").unwrap();
        std::fs::write(
            sidecar_path(&src),
            br#"{"version":1,"basic":{"exposure":0.5}}"#,
        )
        .unwrap();

        let r = load(&src).unwrap().unwrap();
        assert_eq!(r.basic.exposure, 0.5);
        assert_eq!(r.basic.temperature, 6500.0);
        std::fs::remove_dir_all(&d).ok();
    }

    /// sidecar 里不该出现未知/多余字段（往返后仍能被新版读）。
    #[test]
    fn stored_json_is_stable_across_reloads() {
        let d = tmpdir("stable");
        let src = d.join("e.jpg");
        std::fs::write(&src, b"x").unwrap();

        let mut r = AdjustmentRecipe::default();
        r.color.saturation = 0.4;
        save(&src, &r).unwrap();
        let first = std::fs::read_to_string(sidecar_path(&src)).unwrap();

        let loaded = load(&src).unwrap().unwrap();
        save(&src, &loaded).unwrap();
        let second = std::fs::read_to_string(sidecar_path(&src)).unwrap();
        assert_eq!(first, second, "二次存储结果不同 → 不稳定");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn import_export_text_roundtrip() {
        let mut r = AdjustmentRecipe::default();
        r.basic.contrast = -0.4;
        let text = export_text(&r).unwrap();
        assert!(text.contains("contrast"));
        let back = import_text(&text).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn import_empty_text_yields_default() {
        assert!(import_text("").unwrap().is_identity());
    }

    #[test]
    fn import_invalid_text_reports_error() {
        assert!(import_text("}{").is_err());
    }
}
