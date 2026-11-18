// 资源使用情况面板类型定义（FEAT-179 迭代）
//
// 严格对应 Rust 侧 `src-tauri/src/sys_monitor.rs` 的结构体字段，
// 保证 invoke 参数与返回值类型安全（改任一字段两侧必须同步）。

/** 组件类别（对应 Rust ComponentKind，snake_case 序列化） */
export type ComponentKind = "app" | "vcr" | "ai_studio" | "gateway" | "sd_server";

/** 单个组件的采样事实 */
export interface ComponentUsage {
  /** "app" | "vcr" | "ai_studio" | "gateway" | "sd_server" */
  key: string;
  name: string;
  kind: ComponentKind;
  running: boolean;
  pid: number | null;
  /** CPU 占用百分比（采样间隔内；首次为启动以来均值） */
  cpu_percent: number;
  memory_bytes: number;
  memory_percent: number;
  /** 是否可由面板启动 */
  can_start: boolean;
  /** 是否可由面板停止 */
  can_stop: boolean;
  note: string;
}

/** GPU 配置态摘要（来自识别服务 /gpu，非实时利用率） */
export interface GpuUsage {
  enabled: boolean;
  provider: string;
  devices: string[];
  note: string;
}

/** 一次资源采样快照（对应 Rust ResourceUsageSnapshot） */
export interface ResourceUsageSnapshot {
  components: ComponentUsage[];
  gpu: GpuUsage;
  network_note: string;
  system_cpu_percent: number;
  system_memory_used: number;
  system_memory_total: number;
  captured_at: number;
  sample_interval_ms: number;
}
