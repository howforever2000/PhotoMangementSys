/**
 * 路径片段处理（FEAT-150 工坊工具用）
 *
 * 为什么不用 `node:path`：渲染进程里跑的是浏览器环境，没有 node 模块；
 * 而且这里需要同时认 Windows 的 `\` 与 POSIX 的 `/` —— 用户照片路径来自后端，
 * 分隔符取决于图库所在位置，不能假设一种。
 */

/** 从 Windows / POSIX 路径取文件名 */
export function baseName(path: string): string {
  const i = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return i >= 0 ? path.slice(i + 1) : path;
}

/** 取所在目录（末尾不带分隔符）；没有分隔符时返回空串 */
export function dirName(path: string): string {
  const i = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return i > 0 ? path.slice(0, i) : "";
}

/**
 * 拼目录与子项，分隔符跟随 `dir` 自己的风格。
 *
 * 空目录直接返回子项 —— 否则会拼出 `\_compressed` 这种以分隔符开头的相对路径，
 * 后端 `create_dir_all` 会把它当成盘符根目录下的目录，写到开发者意料之外的地方。
 */
export function joinPath(dir: string, name: string): string {
  if (!dir) return name;
  const sep = dir.includes("\\") ? "\\" : "/";
  return dir.endsWith(sep) ? `${dir}${name}` : `${dir}${sep}${name}`;
}
