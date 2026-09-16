//! 开发者视角（数据与路径副窗口）后端 —— 只读诊断
//!
//! 提供三件事（全部只读）：
//!   1. [`paths`]  运行态路径清单：相册主库 / 人物库 / 各类缓存目录 / 模型目录 / 日志，含体量与说明
//!   2. [`tables`] 库表清单：表名 + 行数（photos.db / persons.db）
//!   3. [`rows`]   表数据预览：前 N 行（默认 50，上限 200）
//!
//! 由来：排障时反复出现「数据到底写哪去了」的猜测成本——
//! 相册主库在 `%APPDATA%`、人物库一度由编译期常量指向项目目录（BUG-2026-0916-005）、
//! 模型目录只在只读时才会转成 app_data 下的硬链接副本（BUG-2026-0916-004）。
//! 这些口径必须能在界面上**直接看到**，而不是靠翻代码。
//!
//! 纪律（分层：本模块是服务层，命令壳与建窗在 lib.rs）：
//!   - 一律 `SQLITE_OPEN_READ_ONLY` 打开，**不提供任何写入口**（开发辅助不能变成数据破坏源）
//!   - 库名走白名单、表名走 sqlite_master 校验，标识符按 SQLite 规则转义（不做字符串拼接式注入面）
//!   - 行数上限 + 单元格截断 + BLOB 只报字节数，避免长文本/向量把 IPC 撑爆
//!   - 敏感列（password_hash / token / secret / salt …）默认打码
//!   - 路径口径与运行态严格一致：photos.db / thumbs / avatars / logs 取 app_data_dir；
//!     persons.db 取 `VCR_DATA_DIR`（lib.rs::setup 写入）；模型目录取 `VCR_MODEL_DIR`

use std::path::{Path, PathBuf};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::{logger, vision};

/// 单次预览最大行数（IPC 与渲染保护）
const MAX_LIMIT: i64 = 200;
/// 单个单元格最大字符数（长文本/JSON 截断）
const CELL_MAX: usize = 200;
/// 目录占用统计的文件数上限（防止超大目录把统计拖成分钟级）
const MAX_WALK_FILES: u64 = 200_000;

// ---------------------------------------------------------------------------
// 1. 路径清单
// ---------------------------------------------------------------------------

/// 一条运行态路径（文件或目录）
#[derive(Debug, Clone, Serialize)]
pub struct PathEntry {
    /// 稳定标识（前端用它请求「在资源管理器中定位」）
    pub key: String,
    /// 分组：数据库 / 缓存 / 模型 / 运行态
    pub group: String,
    pub label: String,
    pub path: String,
    pub exists: bool,
    pub is_dir: bool,
    /// 文件大小；目录为递归总占用
    pub bytes: u64,
    /// 目录内文件数（文件为 1，不存在为 0）
    pub files: u64,
    /// 口径说明（为什么在这里 / 有什么坑）
    pub note: String,
}

fn dir_stats(dir: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(entry.path());
            } else {
                files += 1;
                if let Ok(m) = entry.metadata() {
                    bytes += m.len();
                }
                if files >= MAX_WALK_FILES {
                    return (bytes, files);
                }
            }
        }
    }
    (bytes, files)
}

fn file_entry(key: &str, group: &str, label: &str, path: PathBuf, note: &str) -> PathEntry {
    let exists = path.is_file();
    let mut bytes = 0;
    if exists {
        bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        // WAL 体积一并计入（SQLite 未 checkpoint 时数据主要在 -wal 里，容易误判“库是空的”）
        for suffix in ["-wal", "-shm"] {
            let side = PathBuf::from(format!("{}{}", path.display(), suffix));
            if let Ok(m) = std::fs::metadata(&side) {
                bytes += m.len();
            }
        }
    }
    PathEntry {
        key: key.into(),
        group: group.into(),
        label: label.into(),
        path: path.display().to_string(),
        exists,
        is_dir: false,
        bytes,
        files: if exists { 1 } else { 0 },
        note: note.into(),
    }
}

fn dir_entry(key: &str, group: &str, label: &str, path: PathBuf, note: &str) -> PathEntry {
    let exists = path.is_dir();
    let (bytes, files) = if exists { dir_stats(&path) } else { (0, 0) };
    PathEntry {
        key: key.into(),
        group: group.into(),
        label: label.into(),
        path: path.display().to_string(),
        exists,
        is_dir: true,
        bytes,
        files,
        note: note.into(),
    }
}

/// 相册主库路径（app_data_dir/photos.db）——与 lib.rs::setup 完全一致
pub fn photos_db_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {e}"))?
        .join("photos.db"))
}

/// 人物库路径（VCR_DATA_DIR/persons.db）——与 persons.rs / python config.py 同源
pub fn persons_db_path(_app: &AppHandle) -> PathBuf {
    let dir = match std::env::var("VCR_DATA_DIR") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::new(),
    };
    if dir.as_os_str().is_empty() {
        // setup 未跑（理论不可达）：回落项目目录，语义与旧口径一致
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let project = manifest.parent().unwrap_or(manifest);
        return project.join("python").join("data").join("persons.db");
    }
    dir.join("persons.db")
}

/// 库标识 → 路径（**白名单**：前端只能传 "photos" / "persons"）
fn db_path_by_key(app: &AppHandle, key: &str) -> Result<PathBuf, String> {
    match key {
        "photos" => photos_db_path(app),
        "persons" => Ok(persons_db_path(app)),
        other => Err(format!("未知数据库: {other}（只支持 photos / persons）")),
    }
}

/// 运行态路径清单（开发辅助主视图）
pub fn paths(app: &AppHandle) -> Result<Vec<PathEntry>, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {e}"))?;
    let mut out = Vec::new();

    // --- 数据库 ---
    let photos = data_dir.join("photos.db");
    out.push(file_entry(
        "photos_db",
        "数据库",
        "相册主库 photos.db",
        photos.clone(),
        "相册/照片/标签/评分/扫描结果/语义向量/分类定义（含 -wal/-shm 体积）。重装系统清 C: 即丢失",
    ));
    let persons = persons_db_path(app);
    out.push(file_entry(
        "persons_db",
        "数据库",
        "人物库 persons.db",
        persons.clone(),
        "人脸特征与人物命名/合并（VCR 写、人物页直读，同一份）。目录取自 VCR_DATA_DIR，统一在 app_data 下",
    ));
    // 旧口径遗留（只读提示，方便解释“为什么换机后看不到人物”）
    let legacy = {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let project = manifest.parent().unwrap_or(manifest);
        project.join("python").join("data").join("persons.db")
    };
    if legacy.is_file() && legacy != persons {
        out.push(file_entry(
            "persons_db_legacy",
            "数据库",
            "人物库（旧口径遗留）",
            legacy,
            "v0.3.0 及更早的编译期路径产物：安装版曾经读它而服务另写 VCR_DATA_DIR，形成读写分裂（BUG-2026-0916-005）。现已被忽略，确认无用后可删除以释放空间",
        ));
    }

    // --- 缓存 ---
    out.push(dir_entry(
        "thumbs",
        "缓存",
        "缩略图缓存 thumbs/",
        data_dir.join("thumbs"),
        "浏览时按需生成并永久缓存；丢失只影响首屏速度（会重新生成）",
    ));
    out.push(dir_entry(
        "avatars",
        "缓存",
        "人物头像缓存 avatars/",
        data_dir.join("avatars"),
        "人物代表脸裁剪（96×96 JPEG，按人物 id + 脸指纹命名）",
    ));
    out.push(dir_entry(
        "vcr_data",
        "缓存",
        "识别服务数据目录 vcr-data/",
        data_dir.join("vcr-data"),
        "Python 微服务的 VCR_DATA_DIR（persons.db 所在目录）",
    ));

    // --- 模型 ---
    let model_dir = vision::resolve_model_dir(app);
    let bundled = app
        .path()
        .resource_dir()
        .ok()
        .map(|r| r.join("vcr").join("models"));
    let model_note = match &bundled {
        Some(b) if *b != model_dir => {
            "安装包内置目录只读（MSI 装到 Program Files）→ 已启用 app_data 下的可写硬链接副本，语义拆图/档位/下载都写这里（BUG-2026-0916-004）"
        }
        Some(_) => "安装包内置目录且可写（NSIS 按用户安装），直接使用",
        None => "开发态：源码目录 python/models",
    };
    out.push(dir_entry(
        "model_dir",
        "模型",
        "VCR 模型目录",
        model_dir,
        model_note,
    ));
    if let Some(b) = bundled {
        if b.is_dir() {
            out.push(dir_entry(
                "model_dir_bundled",
                "模型",
                "模型目录（安装包内置）",
                b,
                "随安装包分发，只读；缺失的档位由「性能设置 → 模型下载」按需获取",
            ));
        }
    }

    // --- 运行态 ---
    out.push(dir_entry(
        "app_data",
        "运行态",
        "应用数据目录",
        data_dir.clone(),
        "photos.db / thumbs / avatars / logs / session.token / app.key 都在这里（Windows: %APPDATA%/<identifier>）",
    ));
    out.push(dir_entry(
        "logs",
        "运行态",
        "日志目录 logs/",
        data_dir.join("logs"),
        "app.log（开发者视角 · 实时日志窗口读它）；保留 3 天",
    ));
    let exe = std::env::current_exe().unwrap_or_default();
    let info = app.package_info();
    out.push(file_entry(
        "exe",
        "运行态",
        "程序主程序 exe",
        exe,
        &format!(
            "v{} · dev={} · 前端 dist 与后端的构建指纹（跑错包第一现场）",
            info.version,
            tauri::is_dev()
        ),
    ));
    Ok(out)
}

// ---------------------------------------------------------------------------
// 2. 库表清单
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct TableInfo {
    pub name: String,
    pub rows: i64,
    /// FTS 影子表（photo_content_fts_*）：实现细节，界面上弱化显示
    pub shadow: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DbInfo {
    pub key: String,
    pub label: String,
    pub path: String,
    pub exists: bool,
    pub bytes: u64,
    pub tables: Vec<TableInfo>,
    /// 不存在 / 打不开时的说明
    pub error: String,
}

/// SQLite 只读打开（**唯一**打开的入口，杜绝误写）
fn open_ro(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("只读打开失败: {e}"))
}

fn is_shadow_table(name: &str) -> bool {
    const SUFFIX: [&str; 6] = ["_data", "_idx", "_content", "_docsize", "_config", "_segments"];
    name.contains("_fts") && SUFFIX.iter().any(|s| name.ends_with(s))
}

/// 列出两个库的表与行数
pub fn tables(app: &AppHandle) -> Result<Vec<DbInfo>, String> {
    let specs = [
        ("photos", "相册主库 photos.db"),
        ("persons", "人物库 persons.db"),
    ];
    let mut out = Vec::with_capacity(specs.len());
    for (key, label) in specs {
        let path = db_path_by_key(app, key)?;
        let exists = path.is_file();
        let bytes = if exists {
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
        let mut info = DbInfo {
            key: key.into(),
            label: label.into(),
            path: path.display().to_string(),
            exists,
            bytes,
            tables: Vec::new(),
            error: String::new(),
        };
        if !exists {
            info.error = "库文件不存在（该功能还没产生数据，或数据目录被清空）".into();
            out.push(info);
            continue;
        }
        match open_ro(&path).and_then(|conn| list_tables(&conn)) {
            Ok(t) => info.tables = t,
            Err(e) => info.error = e,
        }
        out.push(info);
    }
    Ok(out)
}

fn list_tables(conn: &Connection) -> Result<Vec<TableInfo>, String> {
    let names: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .map_err(|e| format!("读取表清单失败: {e}"))?;
        let it = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| format!("读取表清单失败: {e}"))?;
        it.flatten().collect()
    };
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        // 计数失败（视图/损坏）不阻断列表，记 -1
        let rows = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{}\"", escape_ident(&name)), [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap_or(-1);
        out.push(TableInfo {
            shadow: is_shadow_table(&name),
            name,
            rows,
        });
    }
    Ok(out)
}

/// 标识符转义（SQLite 双引号包裹，内部双引号加倍）
fn escape_ident(name: &str) -> String {
    name.replace('"', "\"\"")
}

// ---------------------------------------------------------------------------
// 3. 表数据预览
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct RowsResult {
    pub db: String,
    pub table: String,
    pub columns: Vec<String>,
    /// 已打码的列名（密码/令牌类）
    pub masked_columns: Vec<String>,
    /// 行数据（字符串化；BLOB 只报字节数，长文本截断）
    pub rows: Vec<Vec<String>>,
    /// 表总行数
    pub total: i64,
    /// 是否因 limit/上限被截断
    pub truncated: bool,
}

/// 敏感列判定：口令 / 令牌 / 密钥 / 盐值
fn is_sensitive_column(col: &str) -> bool {
    let c = col.to_ascii_lowercase();
    const PAT: [&str; 8] = [
        "password", "passwd", "pwd", "token", "secret", "salt", "api_key", "private_key",
    ];
    PAT.iter().any(|p| c.contains(p))
}

/// 单元格 → 字符串（BLOB 只报字节数；长文本截断）
fn cell_to_string(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => format!("{f}"),
        ValueRef::Text(t) => {
            let s = String::from_utf8_lossy(t);
            if s.chars().count() > CELL_MAX {
                let head: String = s.chars().take(CELL_MAX).collect();
                format!("{head}…（共 {} 字符）", s.chars().count())
            } else {
                s.into_owned()
            }
        }
        ValueRef::Blob(b) => format!("<BLOB {} bytes>", b.len()),
    }
}

/// 预览某表前 `limit` 行（只读；敏感列打码）
pub fn rows(app: &AppHandle, db: &str, table: &str, limit: i64) -> Result<RowsResult, String> {
    let path = db_path_by_key(app, db)?;
    if !path.is_file() {
        return Err(format!("库文件不存在: {}", path.display()));
    }
    let limit = limit.clamp(1, MAX_LIMIT);
    let conn = open_ro(&path)?;

    // 表名必须存在于该库（白名单校验，顺带拿到真实名字，杜绝任意 SQL 面）
    let real: String = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE name = ?1 AND type IN ('table','view')",
            [table],
            |r| r.get(0),
        )
        .map_err(|_| format!("库中不存在表/视图: {table}"))?;

    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM \"{}\"", escape_ident(&real)),
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);

    let sql = format!(
        "SELECT * FROM \"{}\" LIMIT {}",
        escape_ident(&real),
        limit
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("准备查询失败: {e}"))?;
    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let masked_columns: Vec<String> = columns
        .iter()
        .filter(|c| is_sensitive_column(c))
        .cloned()
        .collect();

    let mut rows_out: Vec<Vec<String>> = Vec::new();
    let mut q = stmt.query([]).map_err(|e| format!("查询失败: {e}"))?;
    while let Some(row) = q.next().map_err(|e| format!("读取行失败: {e}"))? {
        let mut cells = Vec::with_capacity(columns.len());
        for (idx, col) in columns.iter().enumerate() {
            if is_sensitive_column(col) {
                cells.push("••••••（已打码）".to_string());
                continue;
            }
            let v = row.get_ref(idx).map_err(|e| format!("读取列失败: {e}"))?;
            cells.push(cell_to_string(v));
        }
        rows_out.push(cells);
    }

    logger::log_info(&format!(
        "devdata::rows db={db} table={real} limit={limit} 返回 {} 行（总 {total}）",
        rows_out.len()
    ));

    Ok(RowsResult {
        db: db.into(),
        table: real,
        columns,
        masked_columns,
        truncated: total > rows_out.len() as i64,
        rows: rows_out,
        total,
    })
}
