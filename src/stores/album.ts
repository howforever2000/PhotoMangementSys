import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import type { Album, CreateAlbumInput, UpdateAlbumInput, BatchAlbumOutcome, MergeAlbumOutcome } from "../types/album";
import type { PhotoInfo, PhotoDeleteOutcome, ExportOutcome, PhotoMoveOutcome, PhotoRating, PrewarmOutcome } from "../types/photo";

/** 批量导入结果（对应后端 Rust `ImportResult`） */
export interface ImportResult {
  imported: number;
  skipped: number;
  errors: string[];
}

/**
 * 相册全局状态 —— 对应需求 §3.2 前端状态定义
 *
 * 相当于前端对后端命令的一层 Service 封装（类比 axios API 层），
 * 组件通过调用这里的 action 与 Rust 后端交互。
 */
export const useAlbumStore = defineStore("album", {
  state: () => ({
    /** 当前相册列表缓存 */
    albums: [] as Album[],
    /** 当前正在查看的相册详情 */
    currentAlbum: null as Album | null,
    /** 全局加载状态 */
    isLoading: false,
  }),

  actions: {
    /** 获取相册列表并刷新缓存 */
    async fetchAlbums(): Promise<Album[]> {
      this.isLoading = true;
      try {
        // folder_id/folder_path 由后端以 folder_albums 为唯一事实源填充，前端不做兜底
        this.albums = await invoke<Album[]>("get_albums");
        return this.albums;
      } finally {
        this.isLoading = false;
      }
    },

    /** 获取单个相册详情 */
    async fetchAlbum(id: number): Promise<Album> {
      this.isLoading = true;
      try {
        this.currentAlbum = await invoke<Album>("get_album", { id });
        return this.currentAlbum;
      } finally {
        this.isLoading = false;
      }
    },

    /** 列出相册文件夹内所有图片的绝对路径（无需扫描即可浏览照片） */
    async listAlbumPhotos(albumId: number): Promise<string[]> {
      return await invoke<string[]>("list_album_photos", { albumId });
    },

    /** 批量生成/复用照片网格缩略图，返回 [(原图路径, 缩略图路径)] */
    async getPhotoThumbs(albumId: number, paths: string[]): Promise<[string, string][]> {
      if (!paths.length) return [];
      return await invoke<[string, string][]>("get_photo_thumbs", { albumId, paths });
    },

    /** 读取单张照片信息（分辨率/文件大小/RGB 直方图，按需实时读，不落库） */
    async getPhotoInfo(path: string): Promise<PhotoInfo> {
      return await invoke<PhotoInfo>("get_photo_info", { path });
    },

    /** 批量「相册记录删除」：从网格浏览中移除+清扫描记录，本地文件保留（可恢复） */
    async deletePhotoRecords(albumId: number, paths: string[]): Promise<PhotoDeleteOutcome> {
      return await invoke<PhotoDeleteOutcome>("delete_photo_records", { albumId, paths });
    },

    /** 批量「本地文件删除」：删磁盘文件并级联清理记录与缩略图缓存（不可恢复） */
    async deletePhotoFiles(albumId: number, paths: string[]): Promise<PhotoDeleteOutcome> {
      return await invoke<PhotoDeleteOutcome>("delete_photo_files", { albumId, paths });
    },

    /** 批量导出：把选中照片原图复制到目标目录，可选生成信息清单 */
    async exportPhotos(paths: string[], destDir: string, exportInfo = true): Promise<ExportOutcome> {
      return await invoke<ExportOutcome>("export_photos", { paths, destDir, exportInfo });
    },

    /** 给一批照片打分（rating 0-5，0 清除） */
    async setPhotoRatings(paths: string[], rating: number): Promise<void> {
      await invoke<void>("set_photo_rating", { paths, rating });
    },

    /** 查询一批照片的打分，返回 [(path, rating)] */
    async getPhotoRatings(paths: string[]): Promise<PhotoRating[]> {
      if (!paths.length) return [];
      return await invoke<PhotoRating[]>("get_photo_ratings", { paths });
    },

    /** 把一批照片移动到另一相册（物理移动文件并同步记录），返回移动统计 */
    async movePhotosToAlbum(albumId: number, paths: string[], targetAlbumId: number): Promise<PhotoMoveOutcome> {
      return await invoke<PhotoMoveOutcome>("move_photos_to_album", {
        albumId,
        paths,
        targetAlbumId,
      });
    },

    /**
     * 显式预热一个相册下若干照片的网格缩略图缓存。
     * 任何调用者都可使用：PersonGallery、Memories、首页热门照片等
     * 都需要「缩略图快速复用」的场景都能调。与 PhotoGrid 内部
     * IntersectionObserver 互补：PhotoGrid 是边滚边热；本接口是
     * 「一次全量」预热，适用于“拉一批照片后一次性展示”。
     */
    async prewarmThumbs(albumId: number, paths: string[]): Promise<PrewarmOutcome> {
      if (!paths.length) return { requested: 0, hit: 0, generated: 0, failed: 0 };
      return await invoke<PrewarmOutcome>("prewarm_thumbs", { albumId, paths });
    },

    /** 获取人物头像缓存路径（代表脸 bbox 裁剪；服务未运行时抛错由调用方回退） */
    async getPersonAvatar(pid: string, forceRefresh = false): Promise<string> {
      return await invoke<string>("get_person_avatar", { pid, forceRefresh });
    },

    /** 创建相册，成功后刷新列表并返回新相册 */
    async createAlbum(input: CreateAlbumInput): Promise<Album> {
      const album = await invoke<Album>("create_album", { input });
      await this.fetchAlbums();
      return album;
    },

    /** 批量导入相册（遍历一级子文件夹建相册），成功后刷新列表 */
    async importAlbums(rootPath: string): Promise<ImportResult> {
      const result = await invoke<ImportResult>("import_albums", {
        rootPath,
      });
      await this.fetchAlbums();
      return result;
    },

    /** 更新相册，成功后刷新列表 */
    async updateAlbum(input: UpdateAlbumInput): Promise<void> {
      await invoke<void>("update_album", { input });
      await this.fetchAlbums();
      if (this.currentAlbum && this.currentAlbum.id === input.id) {
        await this.fetchAlbum(input.id);
      }
    },

    /** 重命名相册（可选同步重命名本地文件夹） */
    async renameAlbum(id: number, newName: string, renameFolder = true): Promise<Album> {
      const album = await invoke<Album>("rename_album", { id, newName, renameFolder });
      await this.fetchAlbums();
      if (this.currentAlbum?.id === id) {
        this.currentAlbum = album;
      }
      return album;
    },

    /** 删除相册，成功后刷新列表 */
    async deleteAlbum(id: number): Promise<void> {
      await invoke<void>("delete_album", { id });
      this.albums = this.albums.filter((a) => a.id !== id);
      if (this.currentAlbum?.id === id) {
        this.currentAlbum = null;
      }
    },

    /** 批量删除相册（仅删数据库记录，不删本地文件），成功后刷新列表 */
    async deleteAlbums(ids: number[]): Promise<number> {
      if (ids.length === 0) return 0;
      const deleted = await invoke<number>("delete_albums", { ids });
      this.albums = this.albums.filter((a) => !ids.includes(a.id));
      if (this.currentAlbum && ids.includes(this.currentAlbum.id)) {
        this.currentAlbum = null;
      }
      return deleted;
    },

    /** 批量移动相册到指定分组（folderId=null → 顶级/不分组） */
    async batchMoveAlbumToFolder(ids: number[], folderId: number | null): Promise<BatchAlbumOutcome> {
      return await invoke<BatchAlbumOutcome>("batch_move_album_to_folder", {
        albumIds: ids,
        folderId,
      });
    },

    /** 批量设置相册地点（空字符串清除） */
    async batchSetAlbumLocation(ids: number[], location: string): Promise<BatchAlbumOutcome> {
      return await invoke<BatchAlbumOutcome>("batch_set_album_location", { albumIds: ids, location });
    },

    /** 批量加/删相册标签（mode: add / remove） */
    async batchSetAlbumTag(ids: number[], tags: string[], mode: "add" | "remove"): Promise<BatchAlbumOutcome> {
      return await invoke<BatchAlbumOutcome>("batch_set_album_tag", { albumIds: ids, tags, mode });
    },

    /** 合并相册：mode="move" 物理移入目标文件夹；mode="record" 仅删源相册记录（文件保留原地） */
    async mergeAlbums(sourceIds: number[], targetId: number, mode: "move" | "record" = "move"): Promise<MergeAlbumOutcome> {
      return await invoke<MergeAlbumOutcome>("merge_albums", { sourceIds, targetId, mode });
    },
  },
});
