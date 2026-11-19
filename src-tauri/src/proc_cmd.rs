//! 子进程命令构造（统一「不弹控制台窗口」口径）
//! ===================================================================
//! 职责只有一个：构造 `std::process::Command`，并在 Windows 上强制带上
//! `CREATE_NO_WINDOW`。所有拉起控制台程序（tasklist / taskkill / powershell /
//! reg / 服务解释器）的地方都必须走这里，不要各自 `Command::new`。
//!
//! 为什么必须统一（BUG-2026-1119-001）：
//! 发布版主进程是 GUI 子系统（main.rs 的 `windows_subsystem = "windows"`），
//! **没有可继承的控制台**。此时拉起任何控制台子程序，系统都会为它新建一个
//! 控制台窗口，除非显式传 `CREATE_NO_WINDOW`。资源使用情况面板每 ~2s 轮询一次，
//! 每轮要做 2~4 次 PID 正身核验（tasklist / powershell），漏掉这个标志就会
//! 按轮询节奏连续闪黑窗；而 `tauri dev` 下父进程是控制台程序、子进程继承控制台，
//! 开发态完全看不出来 —— 这就是它长期只在发布版可见的原因。
//!
//! 边界：只封装「启动」这一步，不碰参数、环境变量、stdio 与生命周期；
//! 图形程序（explorer.exe）与 `open` / `xdg-open` 不属于控制台程序，走原样调用。

use std::ffi::OsStr;
use std::process::Command;

/// Windows `CREATE_NO_WINDOW`（0x0800_0000）：以无控制台方式创建子进程
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 构造一个不会再弹出控制台窗口的命令
///
/// 用法与 `std::process::Command::new` 完全一致：
/// ```ignore
/// let mut cmd = crate::proc_cmd::command("tasklist");
/// cmd.args(["/FI", "PID eq 1", "/NH"]).output();
/// ```
pub fn command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造出的命令必须能正常跑起来（验证标志不影响执行）
    #[test]
    fn command_runs_and_captures_output() {
        // 不依赖具体输出内容：只验证「带标志构造 + 执行 + 取回 stdout」链路可用
        let out = if cfg!(target_os = "windows") {
            command("cmd")
                .args(["/C", "echo proc_cmd_ok"])
                .output()
                .expect("cmd 应可执行")
        } else {
            command("echo")
                .arg("proc_cmd_ok")
                .output()
                .expect("echo 应可执行")
        };
        assert!(out.status.success(), "子进程应成功退出");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("proc_cmd_ok"),
            "应能捕获子进程 stdout"
        );
    }

    /// 全仓库口径检查：控制台程序的调用点不得再出现裸 `Command::new`
    ///
    /// 这条单测是「防回归」而非「防误用」：它把纪律钉在可执行的断言上，
    /// 后来者新增 tasklist / powershell 调用若忘了走本模块，测试会直接点名。
    #[test]
    fn no_raw_command_new_for_console_tools() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders: Vec<String> = Vec::new();
        for (path, text) in rust_sources(&root) {
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            // 本模块自身（封装点）、测试代码与图形程序调用不在约束范围
            if name == "proc_cmd.rs" {
                continue;
            }
            for needle in [
                "Command::new(\"tasklist\")",
                "Command::new(\"taskkill\")",
                "Command::new(\"powershell\")",
                "Command::new(\"reg\")",
            ] {
                if text.contains(needle) {
                    offenders.push(format!("{name}: {needle}"));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "以下调用点必须改用 crate::proc_cmd::command（否则发布版会闪黑窗）：{offenders:#?}"
        );
    }

    /// 递归读取 `src/` 下的 .rs 源文件（(路径, 内容)）
    fn rust_sources(root: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
                    if let Ok(text) = std::fs::read_to_string(&p) {
                        out.push((p, text));
                    }
                }
            }
        }
        out
    }
}
