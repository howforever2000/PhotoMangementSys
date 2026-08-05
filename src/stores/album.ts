import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import type { Album, CreateAlbumInput, UpdateAlbumInput } from "../types/album";

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
        const fresh = await invoke<Album[]>("get_albums");
        const withFolder = fresh.filter(a => a.folder_id != null);
        console.log("[STORE] fetchAlbums: total=", fresh.length, "with_folder_id=", withFolder.length,
          "ids=", withFolder.map(a => `${a.id}=${a.folder_id}`));
        // 兜底：如果新返回的相册 folder_id 为 null 但旧缓存中有值，保留旧值
        for (const album of fresh) {
          const old = this.albums.find((a) => a.id === album.id);
          if (old && old.folder_id != null && album.folder_id == null) {
            console.log("[STORE] fetchAlbums FALLBACK: album_id=", album.id, "keeping old folder_id=", old.folder_id);
            album.folder_id = old.folder_id;
            album.folder_path = old.folder_path || album.folder_path;
          }
        }
        this.albums = fresh;
        return this.albums;
      } finally {
        this.isLoading = false;
      }
    },

    /** 获取单个相册详情 */
    async fetchAlbum(id: number): Promise<Album> {
      this.isLoading = true;
      try {
        const album = await invoke<Album>("get_album", { id });
        console.log("[STORE] fetchAlbum: id=", id, "folder_id=", album.folder_id, "folder_path=", album.folder_path);
        // 如果后端返回的 folder_id 为 null，但 store.albums 中该相册已有 folder_id，
        // 保留已有的值（防止后端偶发覆盖导致的显示丢失）
        const existing = this.albums.find((a) => a.id === id);
        if (existing && existing.folder_id != null && album.folder_id == null) {
          console.log("[STORE] fetchAlbum FALLBACK: keeping old folder_id=", existing.folder_id);
          album.folder_id = existing.folder_id;
          album.folder_path = existing.folder_path || album.folder_path;
        }
        this.currentAlbum = album;
        return album;
      } finally {
        this.isLoading = false;
      }
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
  },
});
