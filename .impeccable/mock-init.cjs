/**
 * 验收用 IPC mock（纯开发工具，不参与应用构建）
 *
 * Tauri 后端不在时，把常用的 invoke 命令答成合理假数据，
 * 让真实的 Vue 页面能渲染出来，供截图与对比度机测使用。
 * visual-check.cjs / audit-contrast.cjs 共用这一份，避免两处漂移。
 */

const ALBUMS = [
  { id: 1, name: "京都秋色", path: "D:/Photos/京都秋色", description: null, cover_path: null, created_at: 1758900000, updated_at: 1758900000, photo_count: 137, shoot_time: "2024-11-03", size_bytes: 1073741824, scanned_photo_count: 137, location: "京都", tags: [], folder_id: null, folder_path: "", merged_sources: [] },
  { id: 2, name: "京都夜巷", path: "D:/Photos/京都夜巷", description: null, cover_path: null, created_at: 1758800000, updated_at: 1758800000, photo_count: 55, shoot_time: "2024-11-09", size_bytes: 800000000, scanned_photo_count: 55, location: "京都", tags: [], folder_id: null, folder_path: "", merged_sources: [] },
  { id: 3, name: "城市漫走", path: "D:/Photos/城市漫走", description: null, cover_path: null, created_at: 1758700000, updated_at: 1758700000, photo_count: 88, shoot_time: "2023-06-21", size_bytes: 900000000, scanned_photo_count: 0, location: "上海", tags: [], folder_id: null, folder_path: "", merged_sources: [] },
  { id: 4, name: "北海道雪原", path: "D:/Photos/北海道雪原", description: null, cover_path: null, created_at: 1758600000, updated_at: 1758600000, photo_count: 171, shoot_time: "2024-02-18", size_bytes: 1600000000, scanned_photo_count: 88, location: "北海道", tags: [], folder_id: null, folder_path: "", merged_sources: [] },
  { id: 5, name: "夏日海边", path: "D:/Photos/夏日海边", description: null, cover_path: null, created_at: 1758500000, updated_at: 1758500000, photo_count: 205, shoot_time: "2023-08-07", size_bytes: 2200000000, scanned_photo_count: 41, location: "青岛", tags: [], folder_id: null, folder_path: "", merged_sources: [] },
  { id: 6, name: "日常碎片", path: "D:/Photos/日常碎片", description: null, cover_path: null, created_at: 1758400000, updated_at: 1758400000, photo_count: 222, shoot_time: null, size_bytes: 1370000000, scanned_photo_count: 0, location: null, tags: [], folder_id: null, folder_path: "", merged_sources: [] },
];

/** 让回忆页真的渲染出「故事卡 / 年度卡」（月份分布覆盖多个年份） */
const TIMELINE = [
  [101, 1, "京都秋色", "2025-09-12", "枫叶小径"],
  [102, 1, "京都秋色", "2025-09-20", "鸭川黄昏"],
  [103, 1, "京都秋色", "2025-08-05", "抵园夜色"],
  [104, 3, "城市漫走", "2025-07-18", "梧桐街道"],
  [105, 2, "京都夜巷", "2024-11-03", "居酒屋招牌"],
  [106, 2, "京都夜巷", "2024-11-09", "巷口灯笼"],
  [107, 4, "北海道雪原", "2024-02-18", "雪后天台"],
  [108, 3, "城市漫走", "2024-06-21", "夏至咖啡馆"],
  [109, 5, "夏日海边", "2023-08-07", "海边巴士"],
  [110, 3, "城市漫走", "2023-05-02", "旧书店"],
].map(([id, albumId, albumName, time, label]) => ({
  id,
  path: `D:/Photos/${albumName}/${label}-${id}.jpg`,
  parent_dir: `D:/Photos/${albumName}`,
  album_id: albumId,
  album_name: albumName,
  album_path: `D:/Photos/${albumName}`,
  content: label,
  category: "life",
  sub_category: null,
  label,
  confidence: 0.9,
  person_ids: [],
  shoot_time: time,
  location: "京都",
  iso: "400",
  aperture: "f/1.8",
  shutter_speed: "1/125",
  focal_length: "35",
}));

const INIT = `
window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {};
window.__TAURI_INTERNALS__ = {
  transformCallback(cb, once) {
    const id = Math.floor(Math.random() * 1e9);
    window["__cb_" + id] = (res) => { try { cb(res); } catch (e) {} if (once) delete window["__cb_" + id]; };
    return id;
  },
  async invoke(cmd, payload) {
    const P = payload || {};
    switch (cmd) {
      case "get_current_user":
        return { id: 1, username: "hao", email: "hao@example.com", phone: "13800000000", created_at: 1700000000, avatar: null, role: "user" };
      case "get_albums":
        return window.__MOCK_ALBUMS__;
      case "get_album":
        return window.__MOCK_ALBUMS__.find(a => a.id === Number(P.id)) || window.__MOCK_ALBUMS__[0];
      case "list_timeline":
        return window.__MOCK_TIMELINE__;
      case "get_vcr_gpu_status":
        return { use_gpu: true, provider: "DirectML" };
      case "read_album_content":
      case "get_album_content":
        return { rows: [], total: 0 };
      case "list_persons":
      case "list_persons_in_album":
      case "list_person_photos":
        return [];
      case "prewarm_thumbs":
        return null;
      default:
        if (/^(list|search|query|load|get_.*s$)/.test(cmd)) return [];
        return null;
    }
  },
};
window.__MOCK_ALBUMS__ = ${JSON.stringify(ALBUMS)};
window.__MOCK_TIMELINE__ = ${JSON.stringify(TIMELINE)};
`;

module.exports = { ALBUMS, TIMELINE, INIT };
