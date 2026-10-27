// 数据库启动故障探针（BUG-2026-1026-002）：用与应用完全相同的 rusqlite（bundled）
// 把「写不进目录 / 打不开库 / WAL 起不来 / DDL 失败」四种情况分开。
//
// 用法：cargo run --example db_probe -- "<db路径>"
//
// 判读：
//   std write=ERR 拒绝访问  → **权限/占用**问题（目录不可写），不是库坏了
//   sqlite=ERR …           → 库文件本身打不开（同样先看上一行）
//   wal/ddl=ERR            → 能打开但写不进去（sidecar 建不出来）
//   全 OK                  → 库健康，问题在上层（如应用取错了数据目录）
//
// 经典用法：同一个 exe 拷到别的分区再跑一次 —— 两边结论不同就说明是环境权限。
fn main() {
    let path = std::env::args().nth(1).expect("需要 db 路径参数");
    println!("cwd      = {:?}", std::env::current_dir());
    println!("path     = {path}");
    let p = std::path::Path::new(&path);
    println!("exists   = {}", p.exists());
    match std::fs::metadata(p) {
        Ok(m) => println!("meta     = len={} readonly={}", m.len(), m.permissions().readonly()),
        Err(e) => println!("meta err = {e}"),
    }
    // 1) 纯 std 写入（绕开 sqlite）
    let probe = p.with_file_name("_probe_write.tmp");
    match std::fs::write(&probe, b"x") {
        Ok(_) => {
            println!("std write= OK");
            let _ = std::fs::remove_file(&probe);
        }
        Err(e) => println!("std write= ERR {e}"),
    }
    // 2) rusqlite 打开
    match rusqlite::Connection::open(p) {
        Ok(c) => {
            println!("sqlite   = opened");
            match c.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)) {
                Ok(m) => println!("journal  = {m}"),
                Err(e) => println!("journal  = ERR {e}"),
            }
            match c.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;") {
                Ok(_) => println!("wal      = OK"),
                Err(e) => println!("wal      = ERR {e}"),
            }
            match c.execute_batch("CREATE TABLE IF NOT EXISTS _probe(x); DROP TABLE _probe;") {
                Ok(_) => println!("ddl      = OK"),
                Err(e) => println!("ddl      = ERR {e}"),
            }
        }
        Err(e) => println!("sqlite   = ERR {e}"),
    }
}
