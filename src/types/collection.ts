// 通用「照片集合」页的最小行结构（FEAT-201）
//
// PhotoCollectionPage 是与具体后端命令解耦的共享全页组件：
// 宿主（CollectionView）负责把各命令的行（TimelineCard / CategoryPhoto /
// PersonPhotoItem …）映射成这里的 CollectionPhoto，组件只管
// 分页热加载 / 缩略图管线 / 灯箱 —— 新入口接进来不用再复制这套骨架。

export interface CollectionPhoto {
  path: string;
  /** 无归属照片（相册记录被删等）为 null，缩略图走 album 0 兜底命名空间 */
  albumId: number | null;
  shootTime?: string | null;
  location?: string | null;
}
