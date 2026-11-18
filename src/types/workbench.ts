/**
 * 工作台缩略图数据（FEAT-180）
 * ===================================================================
 * 胶片条与选片网格共用同一份行数据。这里只定义「一个格子需要看到什么」，
 * 状态机仍归 workbench store（`WbItem`）所有，由页面做一次投影。
 */
import type { WbItemStatus } from "../stores/workbench";

export interface WbThumbItem {
  path: string;
  name: string;
  /** 原图地址（解析失败时为空串，显示占位） */
  src: string;
  status: WbItemStatus;
  progress: number;
  /** 是否勾选（批量套用的目标） */
  selected: boolean;
  /** 是否是本次样片 */
  sample: boolean;
}
