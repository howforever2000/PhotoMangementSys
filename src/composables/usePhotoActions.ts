import { ref } from "vue";
import { useRouter } from "vue-router";
import { useAlbumStore } from "../stores/album";
import { useNotify } from "./useNotify";
import { dedupePhotoPaths, workshopRouteTarget } from "../utils/workshopHandoff";
import type { PhotoMarkState } from "../types/photo";

/**
 * FEAT-095：照片「归档 / 隐藏 / 取消」原子动作
 *
 * 单一事实源 —— 缩略图右键菜单（PhotoGrid / PersonGallery / CategoryGallery /
 * LocationGallery）与原图预览器（PhotoLightbox）都走这里，避免 5 处各写一遍
 * 归档逻辑导致行为漂移（早期 PersonGallery 与 CategoryGallery 的删除语义就曾不一致）。
 *
 * 只打标记，不移动文件：照片仍在原目录，EXIF / 评分 / 标签 / 人物关联全部保留。
 */
export function usePhotoActions() {
  const albumStore = useAlbumStore();
  const notify = useNotify();
  const router = useRouter();

  /** 正在处理（防重复点击） */
  const busy = ref(false);
  /** path → 标记状态缓存（右键菜单据此显示「取消归档 / 取消隐藏」） */
  const markStates = ref<Record<string, PhotoMarkState | null>>({});

  /**
   * 拉取一批照片的标记状态并写入本地缓存
   *
   * 静默失败：查询标记状态只是让菜单文案更准，失败时按「未标记」处理即可，
   * 不该打扰用户（与项目既有 getPhotoRatings 的容错风格一致）。
   */
  async function loadMarkStates(paths: string[]): Promise<void> {
    if (!paths.length) return;
    try {
      const rows = await albumStore.getPhotoMarkStates(paths);
      const next = { ...markStates.value };
      for (const r of rows) next[r.path] = r.state;
      markStates.value = next;
    } catch {
      /* 标记状态查询失败按未标记处理 */
    }
  }

  /** 取单张照片的标记状态（未加载过返回 null） */
  function markStateOf(path: string): PhotoMarkState | null {
    return markStates.value[path] ?? null;
  }

  /**
   * 设置 / 取消标记
   *
   * @param state `"archived"` | `"hidden"`
   * @param paths  目标照片路径（支持批量）
   * @param marked true = 打标记；false = 取消（取回原相册）
   */
  async function apply(state: PhotoMarkState, paths: string[], marked: boolean): Promise<boolean> {
    if (!paths.length || busy.value) return false;
    busy.value = true;
    const label = state === "archived" ? "归档" : "隐藏";
    try {
      const outcome = marked
        ? await albumStore.setPhotoMarks(state, paths, true)
        : await albumStore.setPhotoMarks(state, paths, false);
      // 同步本地缓存，避免菜单文案在下次右键时还是旧状态
      const next = { ...markStates.value };
      for (const p of paths) next[p] = marked ? state : null;
      markStates.value = next;

      if (outcome.failed_paths.length) {
        notify.warning(
          `已${marked ? label : "取消"}${label} ${outcome.marked + outcome.unmarked} 张`,
          `失败 ${outcome.failed_paths.length} 张：${outcome.failed_paths.slice(0, 3).join("、")}`,
        );
        return false;
      }
      const n = outcome.marked + outcome.unmarked;
      notify.success(
        `已${marked ? "" : "取消"}${label} ${n} 张`,
        marked
          ? state === "archived"
            ? "可在相册管理的「归档」相册中查看"
            : "可在相册管理的「隐藏」相册中查看（需密码解锁）"
          : "照片已回到原相册",
      );
      return true;
    } catch (e) {
      notify.error(`${marked ? "" : "取消"}${label}失败`, String(e));
      return false;
    } finally {
      busy.value = false;
    }
  }

  /** 归档照片 */
  const archive = (paths: string[]) => apply("archived", paths, true);
  /** 取消归档 */
  const unarchive = (paths: string[]) => apply("archived", paths, false);
  /** 隐藏照片 */
  const hide = (paths: string[]) => apply("hidden", paths, true);
  /** 取消隐藏 */
  const unhide = (paths: string[]) => apply("hidden", paths, false);

  /**
   * 把照片送到创意工坊（FEAT-150 / FEAT-187）
   *
   * 单一编排入口：原图预览、缩略图右键菜单、四个画廊的多选条都调这一个函数，
   * 免得「单张送 A 工位、多张送 B 工位」这种分裂（早期归档逻辑就曾在 5 处各写一遍
   * 而漂移，见 `PhotoActionMenu` 头注释）。
   *
   * 行为：
   *   - 传单张（`tool` 缺省为 `compress`）→ 与旧行为逐字一致：落到压缩工位；
   *   - 传多张 → 不带 `tool`，由工坊落到默认工位（①一键出片，批量加工的主路径），
   *     因为「我勾了 20 张」的诉求是批量出片，不是批量压缩。
   *
   * 空输入直接返回 false，调用方（按钮 `disabled`）不该依赖这个兜底。
   */
  function openInWorkshop(paths: string | readonly string[], tool?: string): boolean {
    const list = dedupePhotoPaths(typeof paths === "string" ? [paths] : paths);
    const target = workshopRouteTarget(list, tool ?? (list.length === 1 ? "compress" : undefined));
    if (!target) return false;
    void router.push(target);
    return true;
  }

  /**
   * 是否处于「归档 / 隐藏相册」内（决定菜单显示「取消归档」还是「归档」）
   *
   * 归档相册内 → 标 archived；隐藏相册内 → 标 hidden；普通相册 → null。
   */
  function isInsideVirtualAlbum(albumKind: string | undefined): boolean {
    return albumKind === "archived" || albumKind === "hidden";
  }

  return {
    busy,
    markStates,
    loadMarkStates,
    markStateOf,
    apply,
    archive,
    unarchive,
    hide,
    unhide,
    openInWorkshop,
    isInsideVirtualAlbum,
  };
}
