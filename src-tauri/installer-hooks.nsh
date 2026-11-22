; =============================================================================
; 安装器钩子（由 tauri 配置 bundle.windows.nsis.installerHooks 引入）
;
; 唯一职责：让「卸载 / 升级」不删模型（BUG-2026-1122-001）。
;
; 背景：Tauri NSIS 升级页默认「先卸载旧版再安装」，旧 uninstall.exe 会 Delete 掉
; 上一版安装器登记过的 `vcr\models\*`；模型一旦住在安装目录，每次升级都要重下。
; 程序侧已把运行期模型目录固定到 %APPDATA%\<identifier>\vcr-models
; （src-tauri/src/vision.rs::resolve_model_dir），这里再兜一层：卸载前把安装目录里
; 残留的模型「按缺失」补进用户模型目录，随后卸载器删的只是安装目录那一份。
;
; 「按缺失」由 robocopy 的三个排除开关实现：
;   /XC 排除已变更、/XN 排除比目标新、/XO 排除比目标旧
;   → 只有目标不存在的文件会被复制；绝不拿安装包里的旧档覆盖用户已下载的新档
;     （与 vision::merge_missing_tree 的语义一致）。
;
; 注意：路径里的 identifier 必须与 tauri.conf.json 的 `identifier` 一致；
;      改标识符时这里与 vision::resolve_model_dir 的 `vcr-models` 落点必须同步。
; =============================================================================

!macro NSIS_HOOK_PREUNINSTALL
  IfFileExists "$INSTDIR\vcr\models\*.*" 0 pms_models_kept
    nsExec::ExecToLog 'robocopy "$INSTDIR\vcr\models" "$APPDATA\com.photomanagementsys.app\vcr-models" /E /XC /XN /XO /R:0 /W:0 /NJH /NJS /NP'
    Pop $0
  pms_models_kept:
!macroend
