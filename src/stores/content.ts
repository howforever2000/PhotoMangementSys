import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import type {
  AlbumContentRow,
  CombinedScanOutcome,
  ContentScanFilters,
  ContentSearchHit,
  ScanOutcome,
  UnifiedScanRow,
  VcrGpuStatus,
} from "../types/content";

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
  },
});
