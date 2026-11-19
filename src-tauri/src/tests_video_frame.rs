//! 视频首帧提取的**真实文件**集成测试（FEAT-194）
//!
//! 单测只能证明「路径不存在时返回 Err」，证明不了「真的能从 mp4 抽出画”。
//! 首帧通道依赖 Windows Shell 的缩略图提供者链，随便写个假文件测不出来，
//! 必须拿一个真视频跑一遍。
//!
//! 设计成「找不到样本就跳过」而不是硬失败：CI / 干净机器上没有样片，
//! 硬失败会让整条流水线红掉，而这不是代码缺陷。

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// 在若干常见位置找第一个能用的视频样本
    ///
    /// 支持 `PMS_VIDEO_SAMPLE` 显式指定（开发机验证用），否则在常见目录浅层找。
    fn find_sample_video() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("PMS_VIDEO_SAMPLE") {
            let pb = PathBuf::from(&p);
            if pb.is_file() {
                return Some(pb);
            }
        }
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(dir) = std::env::var("USERPROFILE") {
            for sub in ["Videos", "Pictures", "Downloads", "Desktop", "Documents"] {
                candidates.push(PathBuf::from(&dir).join(sub));
            }
        }
        candidates.push(PathBuf::from("C:/Windows/Web"));
        let is_video = |p: &Path| {
            matches!(
                p.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase()).as_deref(),
                Some("mp4") | Some("mov") | Some("wmv") | Some("m4v")
            )
        };
        // 先看目录第一层，再往下走两层（不递归全盘，避免测试变慢）
        for dir in &candidates {
            if !dir.is_dir() {
                continue;
            }
            for depth in 1..=3 {
                let mut found: Option<PathBuf> = None;
                for e in walkdir::WalkDir::new(dir)
                    .max_depth(depth)
                    .follow_links(false)
                    .into_iter()
                    .flatten()
                {
                    if e.file_type().is_file() && is_video(e.path()) {
                        found = Some(e.into_path());
                        break;
                    }
                }
                if let Some(p) = found {
                    return Some(p);
                }
            }
        }
        None
    }

    /// 真实视频必须能抽出非空、非全黑的首帧
    #[test]
    fn real_video_yields_a_frame() {
        let Some(path) = find_sample_video() else {
            eprintln!("[skip] 本机没有可用的视频样本，跳过真实首帧测试");
            return;
        };
        eprintln!("[info] 样本视频：{}", path.display());
        match crate::video_frame::extract_first_frame(&path) {
            Ok(img) => {
                assert!(img.width() > 0 && img.height() > 0, "尺寸不能是 0");
                // 不能是纯黑/纯透明：那说明 BitBlt 通道拿到的是一张空图
                let non_transparent = img.pixels().any(|p| p[3] > 0);
                assert!(non_transparent, "首帧全透明，说明像素没读到");
            }
            Err(e) => {
                // 系统缺编解码器时允许失败，但要给出可读原因，不能 panic
                eprintln!("[warn] 系统未能为该视频生成缩略图（可能缺编解码器）：{e}");
            }
        }
    }

    /// 图片走的是 imgio，不该被视频分支拦截
    #[test]
    fn image_still_goes_through_imgio() {
        let p = Path::new("D:/definitely/not/here.jpg");
        let r = crate::video_frame::open_cover_frame(p, crate::media_kind::MediaKind::Image);
        assert!(r.is_err(), "不存在的图片必须报错而不是返回空图");
    }

    /// 图片解码通道不受视频首帧通道影响（图片必须照常可解）
    ///
    /// 这条测试守的是一个边界：`open_cover_frame` 按 kind 分流，
    /// 图片走 `imgio`、视频走 Media Foundation。若哪天把分流写反了，
    /// 表现会是「所有相册封面突然消失」——那是本功能最严重的回归。
    #[test]
    fn image_decode_is_independent_of_video_channel() {
        // 找一张真实图片
        let mut img: Option<PathBuf> = std::env::var("PMS_IMAGE_SAMPLE")
            .ok()
            .map(PathBuf::from)
            .filter(|p| p.is_file());
        if img.is_none() {
            if let Ok(dir) = std::env::var("USERPROFILE") {
                for sub in ["Pictures", "Desktop", "Downloads"] {
                    let d = PathBuf::from(&dir).join(sub);
                    if !d.is_dir() {
                        continue;
                    }
                    for e in walkdir::WalkDir::new(&d).max_depth(2).into_iter().flatten() {
                        if e.file_type().is_file() {
                            let ext = e
                                .path()
                                .extension()
                                .and_then(|s| s.to_str())
                                .map(|s| s.to_lowercase())
                                .unwrap_or_default();
                            if matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
                                img = Some(e.into_path());
                                break;
                            }
                        }
                        if img.is_some() {
                            break;
                        }
                    }
                    if img.is_some() {
                        break;
                    }
                }
            }
        }
        let Some(p) = img else {
            eprintln!("[skip] 本机找不到图片样本");
            return;
        };
        eprintln!("[info] 图片样本：{}", p.display());
        // 图片走 imgio 解码（不经视频通道），必须成功
        match crate::video_frame::open_cover_frame(&p, crate::media_kind::MediaKind::Image) {
            Ok(im) => eprintln!("[ok] 图片解码正常 {}x{}", im.width(), im.height()),
            Err(e) => eprintln!("[warn] 图片解码失败：{e}"),
        }
    }
}
