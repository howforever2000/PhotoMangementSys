import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ClassifyProgress } from "../types/photo";
import { useAlbumStore } from "./album";
import type {
  AlbumContentRow,
  CombinedScanOutcome,
  ContentScanFilters,
  ContentSearchHit,
  ScanOutcome,
  ScanReport,
  UnifiedScanRow,
  VcrGpuStatus,
} from "../types/content";

// ---- FEAT-038：全局照片扫描入库（跨相册批量，后台执行） ----

/** 全局扫描中单个相册的执行状态 */
export type GlobalScanItemStatus = "pending" | "running" | "done" | "failed" | "stopped";

/** 全局扫描任务里的一个相册条目（状态机：pending → running → done/failed；停止时 pending → stopped） */
export interface GlobalScanItem {
  albumId: number;
  albumName: string;
  status: GlobalScanItemStatus;
  /** 本次扫描报告：识别总张数 */
  total: number;
  /** 成功写入/更新的记录数（入库张数） */
  written: number;
  /** 识别失败张数 */
  failed: number;
  /** 失败/异常信息 */
  error: string;
}

/** 全局照片扫描入库任务（单例：同一时刻只有一个全局扫描） */
export interface GlobalScanJob {
  running: boolean;
  /** 用户已请求停止：当前相册完成后不再继续下一个 */
  stopping: boolean;
  /** 勾选的扫描类型（basic / tone / ai） */
  types: string[];
  batch: number;
  /** 逐相册条目（扫描队列快照，含执行状态） */
  items: GlobalScanItem[];
  /** 正在扫描的 items 下标（空闲 -1） */
  currentIndex: number;
  /** 当前相册的照片级进度（来自 classify-progress 事件，仅 AI 类型有） */
  currentProgress: ClassifyProgress | null;
  error: string;
  /** 自增代次号：防止旧任务的收尾覆盖新任务状态 */
  scanId: number;
  /** 任务结束时间（ms，null=未结束/运行中） */
  finishedAt: number | null;
}

function emptyGlobalJob(): GlobalScanJob {
  return {
    running: false,
    stopping: false,
    types: [],
    batch: 8,
    items: [],
    currentIndex: -1,
    currentProgress: null,
    error: "",
    scanId: 0,
    finishedAt: null,
  };
}

/** 组合扫描的单个后台任务状态（按相册隔离，键 = albumId） */
export interface CombinedScanJob {
  running: boolean;
  /** 勾选的扫描类型（basic / tone / ai） */
  types: string[];
  batch: number;
  error: string;
  report: ScanReport | null;
  rows: UnifiedScanRow[];
  /** AI 识别进度（来自 classify-progress 事件） */
  progress: ClassifyProgress | null;
  /** 自增代次号：防止旧扫描的收尾覆盖新扫描状态 */
  scanId: number;
}

function emptyJob(): CombinedScanJob {
  return {
    running: false,
    types: [],
    batch: 8,
    error: "",
    report: null,
    rows: [],
    progress: null,
    scanId: 0,
  };
}

/**
 * 内容扫描与智能搜索状态 —— 对应 Rust `content` 服务层
 *
 * 前端对后端命令的 Service 封装：
 * - `scanAlbumContent`：对相册执行 AI 内容扫描并落库（复用相册详情的识别能力）
 * - `searchPhotoContent`：按关键词搜索照片内容（albumId 空 = 全局，非空 = 单相册内）
 */
export const useContentStore = defineStore("content", {
  state: () => ({
    /** 全局/单相册内容搜索结果 */
    hits: [] as ContentSearchHit[],
    /** 内容搜索加载状态 */
    isSearching: false,
    /** 内容扫描状态 */
    isScanning: false,
    /** 上次内容扫描报告 */
    lastReport: null as ScanOutcome | null,
    /** 组合扫描统一行（FEAT-026） */
    combinedRows: [] as UnifiedScanRow[],
    /** 读表统一行（FEAT-026） */
    albumContentRows: [] as AlbumContentRow[],
    /** 读表总条数 */
    albumContentTotal: 0 as number,
    /** 过滤搜索命中 */
    filterHits: [] as AlbumContentRow[],
    /** GPU 加速可行性状态 */
    gpuStatus: null as VcrGpuStatus | null,
    /** 组合扫描后台任务（键 = albumId；脱离组件存活，支持退出相册后后台继续） */
    combinedJobs: {} as Record<number, CombinedScanJob>,
    /** FEAT-038：全局照片扫描入库任务（单例；脱离组件存活，后台执行） */
    globalScanJob: emptyGlobalJob(),
    /** 当前接收 `classify-progress` 进度事件的相册（同一时刻只追踪一个活动扫描） */
    activeScanAlbum: null as number | null,
    /** 全局进度监听是否已就绪（只注册一次） */
    _progressReady: false,
  }),

  actions: {
    /** 对相册执行 AI 内容扫描并落库（二次扫描按哈希覆盖更新） */
    async scanAlbumContent(albumId: number, batchSize = 8): Promise<ScanOutcome> {
      this.isScanning = true;
      try {
        this.lastReport = await invoke<ScanOutcome>("scan_album_content", {
          albumId,
          batchSize,
        });
        return this.lastReport;
      } finally {
        this.isScanning = false;
      }
    },

    /** 查询 GPU 加速可行性（会确保识别服务就绪） */
    async fetchGpuStatus(): Promise<VcrGpuStatus> {
      this.gpuStatus = await invoke<VcrGpuStatus>("get_vcr_gpu_status");
      return this.gpuStatus;
    },

    /**
     * 按关键词搜索照片内容（智能搜索）
     * @param albumId 空 → 群相册/全局搜索；非空 → 单相册内部搜索
     */
    async searchPhotoContent(keyword: string, albumId?: number): Promise<ContentSearchHit[]> {
      this.isSearching = true;
      try {
        const res = await invoke<ContentSearchHit[]>("search_photo_content", {
          keyword,
          albumId: albumId ?? null,
        });
        this.hits = res;
        return res;
      } finally {
        this.isSearching = false;
      }
    },

    /**
     * FEAT-D：确保单张照片已扫描并落库（用于大图查看器点击原图时）。
     *
     * 行为：
     * - photo_content_scan 中已有 → 命中返回（零 IO）。
     * - 没有 → 后台调单张识别、EXIF + 哈希 → 落库。
     * - 失败返回 null（前端继续展示原图，不阻塞浏览）。
     *
     * 本接口在调用者（PhotoLightbox）中以 fire-and-forget 方式调用，
     * 不阻塞 lightbox 打开速度；扫描完成后再轻量更新一次 meta。
     */
    async ensurePhotoScanned(albumId: number, path: string): Promise<AlbumContentRow | null> {
      try {
        return await invoke<AlbumContentRow | null>("ensure_photo_scanned", { albumId, path });
      } catch {
        return null;
      }
    },

    /** 清空内容搜索结果 */
    clearHits() {
      this.hits = [];
    },

    // ---- FEAT-026：组合扫描 + 读表 + 条件搜索 ----

    /** 组合扫描（EXIF + 影调 + AI 可选组合） */
    async scanAlbumCombined(
      albumId: number,
      scanTypes: string[],
      batchSize = 8,
    ): Promise<CombinedScanOutcome> {
      this.isScanning = true;
      try {
        const outcome = await invoke<CombinedScanOutcome>("scan_album_combined", {
          albumId,
          scanTypes,
          batchSize,
        });
        this.combinedRows = outcome.rows;
        this.lastReport = { report: outcome.report, results: [] };
        return outcome;
      } finally {
        this.isScanning = false;
      }
    },

    /** 读表：分页读取单相册已扫描内容 */
    async readAlbumContent(
      albumId: number,
      page = 1,
      pageSize = 20,
    ): Promise<{ rows: AlbumContentRow[]; total: number }> {
      this.isSearching = true;
      try {
        const res: [AlbumContentRow[], number] = await invoke<
          [AlbumContentRow[], number]
        >("read_album_content", {
          albumId,
          page,
          pageSize,
        });
        this.albumContentRows = res[0];
        this.albumContentTotal = res[1];
        return { rows: res[0], total: res[1] };
      } finally {
        this.isSearching = false;
      }
    },

    /** 带过滤条件的内容搜索 */
    async searchPhotoContentWithFilters(
      keyword: string,
      albumId: number,
      filters: ContentScanFilters,
    ): Promise<AlbumContentRow[]> {
      this.isSearching = true;
      try {
        const res = await invoke<AlbumContentRow[]>("search_photo_content_with_filters", {
          keyword,
          albumId,
          filters,
        });
        this.filterHits = res;
        return res;
      } finally {
        this.isSearching = false;
      }
    },

    // ---- 组合扫描后台任务（FEAT-026 增强） ----

    /** 取某相册的组合扫描任务；不存在则初始化 */
    jobFor(albumId: number): CombinedScanJob {
      if (!this.combinedJobs[albumId]) {
        this.combinedJobs[albumId] = emptyJob();
      }
      return this.combinedJobs[albumId];
    },

    /** 全局注册一次 `classify-progress` 事件监听，路由到当前活动扫描任务 */
    ensureProgressListener() {
      if (this._progressReady) return;
      this._progressReady = true;
      listen<ClassifyProgress>("classify-progress", (e) => {
        const albumId = this.activeScanAlbum;
        if (albumId == null) return;
        const job = this.combinedJobs[albumId];
        if (job) job.progress = e.payload;
        // FEAT-038：全局扫描也接收当前相册的照片级进度
        const g = this.globalScanJob;
        if (g.running && g.currentIndex >= 0 && g.items[g.currentIndex]?.albumId === albumId) {
          g.currentProgress = e.payload;
        }
      }).catch(() => {
        // 监听失败不阻塞；扫描仍能正常完成，仅无实时进度
      });
    },

    /**
     * 后台启动组合扫描（EXIF + 影调 + AI 可选组合）
     *
     * 任务状态存于 store（脱离组件），即使退出相册页后端仍继续扫描、
     * 重新进入时能读到进度与结果。至少勾选一项即可扫描（支持单选项）。
     */
    async startCombinedScan(albumId: number, types: string[], batchSize = 8): Promise<void> {
      const job = this.jobFor(albumId);
      if (job.running) return; // 已有任务进行中，忽略重复点击
      // FEAT-038：与全局扫描互斥（后端共享取消标记 + 单活动进度路由）
      if (this.globalScanJob.running) {
        job.error = "全局照片扫描进行中，请等待其完成或停止后再开始单相册扫描";
        return;
      }
      job.running = true;
      job.error = "";
      job.report = null;
      job.progress = null;
      job.types = [...types];
      job.batch = batchSize;
      job.scanId += 1;
      const myId = job.scanId;
      this.activeScanAlbum = albumId;
      this.ensureProgressListener();
      try {
        const outcome = await invoke<CombinedScanOutcome>("scan_album_combined", {
          albumId,
          scanTypes: types,
          batchSize,
        });
        // 若期间已发起新扫描，丢弃这次旧结果
        if (job.scanId !== myId) return;
        job.report = outcome.report;
        // 用扫描返回的统一行直接填充表格（含单选项场景）
        job.rows = outcome.rows;
      } catch (e) {
        if (job.scanId !== myId) return;
        job.error = `组合扫描失败：${e}`;
      } finally {
        if (job.scanId === myId) {
          job.running = false;
          if (this.activeScanAlbum === albumId) this.activeScanAlbum = null;
        }
      }
    },

    /**
     * 停止组合扫描：通知后端提前结束，并立即结束 UI 的“扫描中”状态。
     * 后端返回的部分结果仍会写入任务（显示已处理部分）。
     */
    async stopCombinedScan(albumId: number): Promise<void> {
      const job = this.jobFor(albumId);
      if (!job.running) return;
      try {
        await invoke("cancel_scan");
      } catch {
        // 忽略取消命令异常；后端任务仍会自行结束
      }
      // 递增代次：让仍在后台的 invoke 收尾不再改写状态
      job.scanId += 1;
      job.running = false;
    },

    /**
     * FEAT-037：对多个相册批量组合扫描入库（批量管理用）。
     *
     * 逐个相册串行调用 `startCombinedScan`（后端 scan_album_combined 是单相册命令，
     * 且共享同一取消标记 + 单 activeScanAlbum 进度路由，串行可避免并发冲突与进度混乱）。
     * 每个相册扫描在后台完成后返回，全部结束返回汇总。
     *
     * @param albumIds 选中的相册 id 列表
     * @param types 扫描类型（basic / tone / ai，默认三项全选）
     * @param batchSize 批次大小
     * @returns 已扫描相册数 / 失败信息
     */
    async scanAlbumsCombined(
      albumIds: number[],
      types: string[],
      batchSize = 8,
      onProgress?: (done: number, total: number, currentAlbumId: number) => void,
    ): Promise<{ scanned: number; failed: { albumId: number; error: string }[] }> {
      const result = { scanned: 0, failed: [] as { albumId: number; error: string }[] };
      const total = albumIds.length;
      let done = 0;
      for (const albumId of albumIds) {
        const job = this.jobFor(albumId);
        onProgress?.(done, total, albumId);
        try {
          // 若该相册已有扫描任务在运行，跳过（避免 startCombinedScan 因 running 直接 return 被误判为成功）
          if (job.running) {
            result.failed.push({ albumId, error: "该相册已有扫描任务进行中，已跳过" });
          } else {
            // 串行等待该相册扫描完成（startCombinedScan 内部 await invoke）
            await this.startCombinedScan(albumId, types, batchSize);
            if (job.error) {
              result.failed.push({ albumId, error: job.error });
            } else {
              result.scanned += 1;
            }
          }
        } catch (e) {
          result.failed.push({ albumId, error: String(e) });
        }
        done += 1;
        onProgress?.(done, total, albumId);
      }
      return result;
    },

    // ---- FEAT-038：全局照片扫描入库（跨相册批量，后台执行） ----

    /**
     * 启动全局照片扫描入库：同步校验 + 初始化队列后立即返回，
     * 扫描循环在后台执行（fire-and-forget，脱离组件存活）。
     *
     * 串行逐相册调用后端 `scan_album_combined`
     * （与相册管理中的组合扫描同一后端命令，复用 EXIF / 影调 / AI 识别能力）。
     * - 与单相册组合扫描互斥（共享后端取消标记与进度路由）
     * - 每个相册完成后刷新相册列表（更新「已入库」徽标）
     *
     * @returns 校验与启动是否成功（false 时原因见 `globalScanJob.error`）
     */
    beginGlobalScan(
      entries: { id: number; name: string }[],
      types: string[],
      batchSize = 8,
    ): boolean {
      const job = this.globalScanJob;
      if (job.running) {
        job.error = "全局扫描已在进行中";
        return false;
      }
      if (!entries.length) {
        job.error = "请至少勾选一个相册";
        return false;
      }
      if (!types.length) {
        job.error = "请至少勾选一项扫描类型";
        return false;
      }
      // 与单相册组合扫描互斥：后端共享取消标记 + 单活动进度路由
      const runningSingle = Object.values(this.combinedJobs).some((j) => j.running);
      if (runningSingle) {
        job.error = "有单相册扫描任务进行中，请等待其完成或停止后再开始全局扫描";
        return false;
      }
      job.running = true;
      job.stopping = false;
      job.error = "";
      job.types = [...types];
      job.batch = batchSize;
      job.items = entries.map((e) => ({
        albumId: e.id,
        albumName: e.name,
        status: "pending" as GlobalScanItemStatus,
        total: 0,
        written: 0,
        failed: 0,
        error: "",
      }));
      job.currentIndex = -1;
      job.currentProgress = null;
      job.finishedAt = null;
      job.scanId += 1;
      this.ensureProgressListener();
      // 后台执行扫描循环：不 await，任务独立于组件生命周期存活
      const myId = job.scanId;
      void this.runGlobalScanLoop(myId, [...types], batchSize);
      return true;
    },

    /** 全局扫描后台循环：串行逐相册扫描（由 beginGlobalScan 启动） */
    async runGlobalScanLoop(myId: number, types: string[], batchSize: number): Promise<void> {
      const job = this.globalScanJob;
      const albumStore = useAlbumStore();
      try {
        for (let i = 0; i < job.items.length; i++) {
          if (job.scanId !== myId) return; // 已被重置/新任务接管
          // 用户请求停止：剩余相册标记「已停止」并结束
          if (job.stopping) {
            for (let k = i; k < job.items.length; k++) {
              if (job.items[k].status === "pending") job.items[k].status = "stopped";
            }
            break;
          }
          const item = job.items[i];
          item.status = "running";
          job.currentIndex = i;
          job.currentProgress = null;
          this.activeScanAlbum = item.albumId;
          try {
            const outcome = await invoke<CombinedScanOutcome>("scan_album_combined", {
              albumId: item.albumId,
              scanTypes: types,
              batchSize,
            });
            if (job.scanId !== myId) return;
            item.total = outcome.report.total;
            item.written = outcome.report.written;
            item.failed = outcome.report.failed;
            item.status = "done";
          } catch (e) {
            if (job.scanId !== myId) return;
            item.status = "failed";
            item.error = String(e);
          }
          job.currentProgress = null;
          job.currentIndex = -1;
          this.activeScanAlbum = null;
          // 刷新相册列表，让「已入库」徽标与统计实时更新（失败不影响流程）
          albumStore.fetchAlbums().catch(() => {});
        }
        job.finishedAt = Date.now();
      } finally {
        if (job.scanId === myId) {
          job.running = false;
          job.stopping = false;
          job.currentIndex = -1;
          job.currentProgress = null;
          if (this.activeScanAlbum != null) this.activeScanAlbum = null;
        }
      }
    },

    /**
     * 停止全局扫描：通知后端取消当前相册的识别（部分结果仍会落库），
     * 当前相册收尾后不再继续剩余相册（标记为「已停止」）。
     */
    async stopGlobalScan(): Promise<void> {
      const job = this.globalScanJob;
      if (!job.running || job.stopping) return;
      job.stopping = true;
      try {
        await invoke("cancel_scan");
      } catch {
        // 忽略取消命令异常；当前相册完成后循环仍会按 stopping 结束
      }
    },

    /** 清空全局扫描记录（仅运行中禁用） */
    clearGlobalScan(): void {
      const job = this.globalScanJob;
      if (job.running) return;
      job.items = [];
      job.currentProgress = null;
      job.currentIndex = -1;
      job.error = "";
      job.finishedAt = null;
    },
  },
});
