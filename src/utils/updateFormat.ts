/**
 * 应用内更新的显示格式化（速度 / 剩余时间 / 检查时间 / 发布时间）
 *
 * 为什么单独抽一层：这几个字符串直接决定用户对「还要等多久」的判断，
 * 而且它们全都有空值语义（速度未知 → 不显示而不是显示 `0 KB/s`）。
 * 抽成纯函数才能在 `node --test` 里钉住边界，组件里只负责摆放。
 */
import { formatBytes } from "./bytes.ts";

/** 两位补零（`5` → `05`） */
function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

/**
 * 下载速度 → `"1.2 MB/s"`
 *
 * `bps <= 0` 返回空串：测速/收尾阶段后端会把速度置 0，此时显示 `0 KB/s`
 * 会让用户以为下载卡死了；不显示才符合「暂时没有速度读数」的事实。
 */
export function formatSpeed(bps: number): string {
  if (!Number.isFinite(bps) || bps <= 0) return "";
  return `${formatBytes(bps)}/s`;
}

/**
 * 剩余时间 → `剩余 45 秒` / `剩余 3 分 20 秒` / `剩余 1 小时 05 分`
 *
 * 边界：
 * - 速度未知或体积未知（`bps <= 0 || total <= 0`）→ 空串，算不出就不编；
 * - 已经下满（`total <= received`）→ 空串，此时该显示的是「即将完成」而不是「剩余 0 秒」；
 * - 秒数**向上取整**：向下取整会在最后一秒反复显示「剩余 0 秒」。
 *
 * ≥ 1 小时只给「小时 + 分」：这个量级的下载，秒级精度对用户没有决策价值，
 * 反而让进度文案每秒跳动一次。分钟补零两位（`05`）避免「1 小时 5 分」读成别的数。
 */
export function formatEta(received: number, total: number, bps: number): string {
  if (!Number.isFinite(bps) || bps <= 0) return "";
  if (!Number.isFinite(total) || total <= 0 || total <= received) return "";
  const sec = Math.ceil((total - received) / bps);
  if (sec < 60) return `剩余 ${sec} 秒`;
  // 用「总分钟」再拆小时，天然处理进位（3599s 向上取整成 60 分 → 1 小时 00 分）
  const totalMin = Math.ceil(sec / 60);
  if (totalMin < 60) {
    return `剩余 ${Math.floor(sec / 60)} 分 ${sec % 60} 秒`;
  }
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  return `剩余 ${h} 小时 ${pad2(m)} 分`;
}

/**
 * 上次检查时间 → `MM-DD HH:mm`（本地时区）
 *
 * `ms <= 0` 返回 `未检查`：后端 `checked_at_ms` 在从未检查过时是 0，
 * 显示成 `01-01 08:00` 会让用户误以为刚刚查过。
 */
export function formatCheckedAt(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "未检查";
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "未检查";
  return `${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

/**
 * 发布时间 → `YYYY-MM-DD`（本地时区）
 *
 * 空串 / 解析失败都返回空串，由调用方决定「整段不渲染」。
 * 只到日期：Release 的发布时间精确到分钟对用户没有意义，日期足够说明「新不新」。
 */
export function formatPublishedAt(iso: string): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}
