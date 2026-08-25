import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ClassifyProgress } from "../types/photo";
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
  },
});
