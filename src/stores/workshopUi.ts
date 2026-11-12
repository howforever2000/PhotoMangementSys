/**
 * 创意工坊界面状态 store（FEAT-150 后台化）
 *
 * 为什么单独存一份 UI 状态：
 *   - `App.vue` 的 `<router-view/>` 没有 keep-alive，离开 `/workshop` 组件即销毁；
 *     若工具选择 / 素材托盘留在组件局部 ref，切页再回来会被重置回压缩工位、托盘清空。
 *   - 但「后台任务继续跑」这件事已经由后端 job + `compressTask`/`aiStudio` store 保证，
 *     这里只需把"用户上次在哪、带了哪些照片"这层很薄的视图状态也提出来，两者配合才是
 *     完整的「切页不打断」：任务在跑、回来还停在原工位、圆点提示运行中。
 */
import { defineStore } from "pinia";

/** 创意工坊工具 id（与 `src/views/Workshop.vue` 的 ToolId 一一对应） */
export type WorkshopToolId =
  | "compress"
  | "ocr"
  | "local_film"
  | "local_ctransfer"
  | "local_restore"
  | "ai_journal"
  | "ai_beauty"
  | "ai_style"
  | "ai_upscale"
  | "ai_smart";

interface WorkshopUiState {
  /** 上次选中的工具（离开页面再回来恢复，不重置到压缩） */
  active: WorkshopToolId;
  /** 素材托盘（跨工具 / 跨页面切换保留） */
  tray: string[];
}

export const useWorkshopUiStore = defineStore("workshopUi", {
  state: (): WorkshopUiState => ({
    active: "compress",
    tray: [],
  }),

  actions: {
    setActive(id: WorkshopToolId): void {
      this.active = id;
    },
    setTray(list: string[]): void {
      this.tray = list;
    },
    addPhotos(list: string[]): void {
      this.tray = [...new Set([...this.tray, ...list])];
    },
    removePhoto(path: string): void {
      this.tray = this.tray.filter((p) => p !== path);
    },
    clearTray(): void {
      this.tray = [];
    },
  },
});
