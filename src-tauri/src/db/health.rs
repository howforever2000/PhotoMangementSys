//! 入库体检的持久层聚合（FEAT-152）
//!
//! ## 为什么口径是「目录前缀」而不是 `album_id`
//!
//! 三张表（`photo_content_scan` / `photo_thumb_cache` / `photo_embeddings`）都带
//! `album_id` 列，但那一列是**写入时的快照**：相册被拆分、迁移、父子相册共享照片时
//! 它会停留在旧值上。本项目已经为此踩过一次坑（BUG-2026-0909-001：按 `album_id`
//! 统计导致「之前入库的照片变未入库」），所以 `count_scanned_by_prefix` 改为按
//! 相册目录前缀在全库路径上做子树匹配。
//!
//! 体检沿用同一口径（否则「体检说缺 300 张、智慧相册说 0 张缺失」这种自相矛盾
//! 一定会发生）。代价是每次体检要拉三张表的路径列进内存做前缀匹配 —— 全库两万
//! 多张照片、几十个相册，实测是毫秒级，且这是**用户手动触发的只读操作**。
//!
//! ## 只进内存的另两个原因
//!
//! - 只读：不建表、不写缓存，避免「体检」这种诊断动作改变被诊断对象；
//! - 可解释：每个数字都由 `SELECT path ... WHERE user_id=?` 现算，没有二级缓存
//!   过期带来的新语义。

use std::collections::HashMap;

use rusqlite::params;

use super::{DbError, Database};

/// 抽样验证「记录指向的文件是否还在」时，每个相册最多看多少条
///
/// 全量 stat 在机械盘 / 网络盘上要几十秒，而体检的用途是「发现问题」而不是
/// 「精确点名」：50 条足够判断一册是否整体失联，且总量 = 相册数 × 50 次 stat。
pub const SAMPLE_PER_ALBUM: usize = 50;

/// 单个相册的路径级事实（全部按目录前缀统计）
#[derive(Debug, Clone, Default)]
pub struct AlbumFacts {
    /// 已入库行数（`photo_content_scan`）
    pub scanned: i64,
    /// 有缩略图缓存行的照片数（`photo_thumb_cache`）
    pub thumbs: i64,
    /// 缩略图缓存占用字节（`photo_thumb_cache.size_bytes` 之和）
    pub thumb_bytes: i64,
    /// 有图像向量的照片数（`photo_embeddings`，任意模型）
    pub embeddings: i64,
    /// 抽样用的已入库路径（最多 [`SAMPLE_PER_ALBUM`] 条）
    pub sample_paths: Vec<String>,
}

impl Database {
    /// 一次取全部相册的磁盘读数（`album_stats` 缓存表），避开逐册查询的 N+1
    ///
    /// 返回 `album_id → (photo_count, size_bytes)`。缓存表里没有行的相册不在返回值里，
    /// 调用方按 0 处理（含义：从未完成过一次目录统计）。
    pub fn album_stats_map(&self) -> Result<HashMap<i64, (i64, u64)>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT album_id, photo_count, size_bytes FROM album_stats")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, (r.get::<_, i64>(1)?, r.get::<_, u64>(2)?)))
        })?;
        let mut out = HashMap::new();
        for r in rows {
            let (id, v) = r?;
            out.insert(id, v);
        }
        Ok(out)
    }

    /// 一次取全库三张表的路径事实，并按相册目录前缀聚合
    ///
    /// `album_paths`：`(album_id, 绝对路径)`。虚拟相册（哨兵路径）由调用方过滤，
    /// 传进来也不会命中任何前缀。
    pub fn album_health_facts(
        &self,
        user_id: i64,
        album_paths: &[(i64, String)],
    ) -> Result<HashMap<i64, AlbumFacts>, DbError> {
        let mut matcher = OwnerMatcher::new(album_paths);
        let mut out: HashMap<i64, AlbumFacts> = HashMap::new();
        for (id, _) in album_paths {
            out.insert(*id, AlbumFacts::default());
        }

        // ---- 1. 已入库 ----
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM photo_content_scan WHERE user_id = ?1")?;
        let rows = stmt.query_map(params![user_id], |r| r.get::<_, String>(0))?;
        for r in rows {
            let path = r?;
            for id in matcher.owners_of_file(&path) {
                let f = out.get_mut(&id).expect("prefix 表与 facts 表同源");
                f.scanned += 1;
                if f.sample_paths.len() < SAMPLE_PER_ALBUM {
                    f.sample_paths.push(path.clone());
                }
            }
        }

        // ---- 2. 缩略图缓存 ----
        let mut stmt = self.conn.prepare(
            "SELECT source_path, size_bytes FROM photo_thumb_cache WHERE user_id = ?1",
        )?;
        let rows = stmt.query_map(params![user_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for r in rows {
            let (path, bytes) = r?;
            for id in matcher.owners_of_file(&path) {
                let f = out.get_mut(&id).expect("prefix 表与 facts 表同源");
                f.thumbs += 1;
                f.thumb_bytes += bytes;
            }
        }

        // ---- 3. 图像向量 ----
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM photo_embeddings WHERE user_id = ?1")?;
        let rows = stmt.query_map(params![user_id], |r| r.get::<_, String>(0))?;
        for r in rows {
            let path = r?;
            for id in matcher.owners_of_file(&path) {
                out.get_mut(&id).expect("prefix 表与 facts 表同源").embeddings += 1;
            }
        }

        Ok(out)
    }
}

/// 把「一个文件属于哪些相册」变成一次查表
///
/// ## 为什么不用「每行 × 每个相册」直接 starts_with
///
/// 真实库实测：130 个相册 / 2.5 万张照片。逐行逐册比较是 3 × 325 万次字符串
/// 前缀判断（Python 原型实测 1.4s，Rust 虽快一个量级，但库再大一倍就要秒级）。
/// 而同一目录下的文件**归属完全一致**——按父目录缓存后，比较量降到
/// 「不同目录数 × 相册数」，2.5 万张照片的实际目录数是百量级。
///
/// ## 归属口径（与 `count_scanned_by_prefix` 等价）
///
/// 文件 p 属于相册 A ⇔ `p` 以「A.path + 分隔符」开头
/// ⇔ A.path == parent(p)，或 parent(p) 以「A.path + 分隔符」开头。
/// 后一种写法让「直接放在相册根目录里的文件」与「更深层子目录里的文件」
/// 用同一套判断，嵌套相册也自然成立（同一文件同时属于父子两册，与现有统计一致）。
struct OwnerMatcher<'a> {
    /// (album_id, 去尾分隔符的相册路径)
    roots: Vec<(i64, &'a str)>,
    cache: HashMap<String, Vec<i64>>,
}

impl<'a> OwnerMatcher<'a> {
    fn new(album_paths: &'a [(i64, String)]) -> Self {
        Self {
            roots: album_paths
                .iter()
                .map(|(id, p)| (*id, p.trim_end_matches(['\\', '/'])))
                .collect(),
            cache: HashMap::new(),
        }
    }

    /// 文件所属的相册 id（可能多个：嵌套相册时父子都算）
    fn owners_of_file(&mut self, path: &str) -> Vec<i64> {
        let dir = std::path::Path::new(path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("");
        if let Some(hit) = self.cache.get(dir) {
            return hit.clone();
        }
        let owned: Vec<i64> = self
            .roots
            .iter()
            .filter(|(_, root)| Self::dir_under(dir, root))
            .map(|(id, _)| *id)
            .collect();
        self.cache.insert(dir.to_string(), owned.clone());
        owned
    }

    /// 目录 `dir` 是否落在相册根 `root` 之下（含根目录本身）
    fn dir_under(dir: &str, root: &str) -> bool {
        if root.is_empty() {
            return false;
        }
        let normalized = dir.trim_end_matches(['\\', '/']);
        if normalized.eq_ignore_ascii_case(root) {
            return true;
        }
        // 分隔符方向：允许子路径用 / 或 \（历史数据里两者混用）；
        // 相册根自身的写法不归一（与 count_scanned_by_prefix 完全同一口径）。
        let stripped = normalized
            .strip_prefix(root)
            .or_else(|| {
                // 大小写不敏感比较（Windows 路径口径；不做 to_lowercase 以免每行分配）
                if normalized.len() > root.len()
                    && normalized.is_char_boundary(root.len())
                    && normalized[..root.len()].eq_ignore_ascii_case(root)
                {
                    Some(&normalized[root.len()..])
                } else {
                    None
                }
            });
        matches!(stripped, Some(rest) if rest.starts_with('\\') || rest.starts_with('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时库：`Database::open` 会建好全部表（体检读的就是真实 schema）
    fn temp_db(tag: &str) -> Database {
        let dir = std::env::temp_dir().join(format!(
            "pms-health-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Database::open(&dir.join("t.db")).unwrap()
    }

    /// 插一条入库记录（真实 schema 的必填列都要给）
    fn insert_scan(db: &Database, path: &str, user_id: i64) {
        db.conn
            .execute(
                "INSERT INTO photo_content_scan (photo_hash, path, parent_dir, user_id, scanned_at)
                 VALUES (?1, ?2, ?3, ?4, 0)",
                params![format!("H{path}"), path, "", user_id],
            )
            .unwrap();
    }

    fn insert_thumb(db: &Database, path: &str, bytes: i64, user_id: i64) {
        db.conn
            .execute(
                "INSERT INTO photo_thumb_cache
                 (photo_hash, source_path, thumb_path, user_id, size_bytes, mtime_ns, generated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, 0)",
                params![format!("T{path}"), path, "", user_id, bytes],
            )
            .unwrap();
    }

    fn insert_embedding(db: &Database, path: &str, user_id: i64) {
        db.conn
            .execute(
                "INSERT INTO photo_embeddings (photo_hash, user_id, path, dim, model, embedding, generated_at)
                 VALUES (?1, ?2, ?3, 2, 'm', x'0000', '0')",
                params![format!("E{path}"), user_id, path],
            )
            .unwrap();
    }

    #[test]
    fn facts_are_counted_by_dir_prefix() {
        let db = temp_db("prefix");
        insert_scan(&db, "C:\\alb\\a\\1.jpg", 1);
        insert_scan(&db, "C:\\alb\\a\\2.jpg", 1);
        insert_scan(&db, "C:\\alb\\ab\\3.jpg", 1); // 兄弟目录，不能算进 a
        insert_scan(&db, "C:\\alb\\b\\4.jpg", 1);
        insert_scan(&db, "C:\\alb\\a\\9.jpg", 2); // 别的用户
        insert_thumb(&db, "C:\\alb\\a\\1.jpg", 100, 1);
        insert_thumb(&db, "C:\\alb\\b\\4.jpg", 50, 1);
        insert_embedding(&db, "C:\\alb\\a\\1.jpg", 1);

        let albums = vec![
            (1i64, "C:\\alb\\a".to_string()),
            (2i64, "C:\\alb\\b".to_string()),
        ];
        let facts = db.album_health_facts(1, &albums).unwrap();

        assert_eq!(facts[&1].scanned, 2, "只算 a 目录前缀、且只算当前用户");
        assert_eq!(facts[&1].thumbs, 1);
        assert_eq!(facts[&1].thumb_bytes, 100);
        assert_eq!(facts[&1].embeddings, 1);
        assert_eq!(facts[&2].scanned, 1, "兄弟目录不被 a 吃掉");
        assert_eq!(facts[&2].thumbs, 1);
        assert_eq!(facts[&2].embeddings, 0);
        // 抽样只收本册的路径
        assert!(facts[&1].sample_paths.iter().all(|p| p.contains("\\a\\")));
    }

    #[test]
    fn matcher_covers_root_files_nesting_and_case() {
        let albums = vec![
            (1i64, "C:\\alb\\a".to_string()),
            (2i64, "C:\\alb\\a\\sub".to_string()),
            (3i64, "C:\\alb\\ab".to_string()),
        ];
        let mut m = OwnerMatcher::new(&albums);

        // 直接放在相册根里的文件：父目录 == 相册根
        assert_eq!(m.owners_of_file("C:\\alb\\a\\1.jpg"), vec![1]);
        // 子目录：同时属于子相册与父相册（与现有统计一致）
        assert_eq!(m.owners_of_file("C:\\alb\\a\\sub\\2.jpg"), vec![1, 2]);
        // 兄弟目录不被吃掉（ab 不应命中 a）
        assert_eq!(m.owners_of_file("C:\\alb\\ab\\3.jpg"), vec![3]);
        // Windows 大小写不敏感
        assert_eq!(m.owners_of_file("c:\\ALB\\A\\4.jpg"), vec![1]);
        // 相册根以外的路径不属于任何相册
        assert!(m.owners_of_file("D:\\other\\y.jpg").is_empty());
    }

    #[test]
    fn sample_paths_are_capped() {
        let db = temp_db("cap");
        for i in 0..(SAMPLE_PER_ALBUM + 10) {
            insert_scan(&db, &format!("C:\\alb\\a\\{i}.jpg"), 1);
        }
        let facts = db
            .album_health_facts(1, &[(1, "C:\\alb\\a".to_string())])
            .unwrap();
        assert_eq!(facts[&1].scanned, (SAMPLE_PER_ALBUM + 10) as i64);
        assert_eq!(facts[&1].sample_paths.len(), SAMPLE_PER_ALBUM);
    }
}
