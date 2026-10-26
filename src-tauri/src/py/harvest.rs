//! 运行目录产物回收 + 旧 run 清扫

use std::path::Path;
use std::time::{Duration, SystemTime};

use tauri::AppHandle;

use crate::py::audit::py_audit;
use crate::py::runtime::py_runs_root;

pub fn harvest_run_outputs(dir: &Path, gen_dir: &Path, audit: &mut dyn FnMut(&str)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut moved = 0usize;
    for e in entries.flatten() {
        let name = e.file_name();
        // run.py（执行入口）/ script.py（脚本正文）/ params.json（入参）不是产物：
        // 搬进 AI_Gen_Files 只会给用户塞一堆看不懂的 .py 垃圾。
        if name == "run.py" || name == "script.py" || name == "params.json" {
            continue;
        }
        let src = e.path();
        // 符号链接不搬运不跟随：rename 会把链接本体搬进 AI_Gen_Files（目标仍在
        // run 目录外），顺着拷会把白名单外目标的内容带进产物目录——留痕跳过
        let is_symlink = std::fs::symlink_metadata(&src)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink {
            audit(&format!(
                "run_python | harvest 跳过符号链接：{}",
                src.display()
            ));
            continue;
        }
        let dest = dedup_dest(gen_dir, &name);
        // rename 失败（跨盘等）走 copy+remove；remove 失败必须把刚拷出的副本
        // 删掉回滚——否则源/目标双份留存且不计数、不出审计，用户毫无感知
        let ok = match std::fs::rename(&src, &dest) {
            Ok(()) => true,
            Err(_) => {
                if copy_rec(&src, &dest).is_ok() {
                    if remove_rec(&src) {
                        true
                    } else {
                        // 源删不掉：撤销副本回到原状，并响亮留痕
                        let _ = remove_rec(&dest);
                        audit(&format!(
                            "run_python | harvest 回滚：{src:?} 删除失败（拷贝副本已撤销），文件保留在原处"
                        ));
                        false
                    }
                } else {
                    false
                }
            }
        };
        if ok {
            moved += 1;
            // 搬运会把「来自互联网」标记一起带过来：跨盘拷贝走 CopyFileEx（复制全部
            // 备用数据流），同盘 rename 则是同一个文件、标记原样跟着走。产物目录里的
            // 文件不该带这个标记——带了就进 Office 受保护视图，还会招杀软连坐拦截
            let stripped = crate::platform::zone_id::strip_zone_identifier_rec(&dest, audit);
            if stripped > 0 {
                audit(&format!(
                    "run_python | 剥「来自互联网」标记 | files: {stripped} | {}",
                    dest.display()
                ));
            }
        }
    }
    if moved > 0 {
        audit(&format!(
            "run_python | harvested={moved} | 运行目录产物已移入 AI_Gen_Files"
        ));
    }
}

pub fn dedup_dest(dir: &Path, name: &std::ffi::OsStr) -> std::path::PathBuf {
    let mut candidate = dir.join(name);
    let mut n = 1;
    while candidate.exists() {
        let p = std::path::Path::new(name);
        let stem = p.file_stem().unwrap_or(name).to_string_lossy();
        let new_name = match p.extension() {
            Some(ext) => format!("{stem} ({n}).{}", ext.to_string_lossy()),
            None => format!("{stem} ({n})"),
        };
        candidate = dir.join(new_name);
        n += 1;
    }
    candidate
}

pub fn copy_rec(src: &Path, dest: &Path) -> std::io::Result<()> {
    // symlink_metadata 看条目本身：嵌在子目录里的符号链接同样跳过不跟随，
    // 防止把 run 目录外的目标内容拷进 AI_Gen_Files
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_rec(&e.path(), &dest.join(e.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dest).map(|_| ())
    }
}

pub fn remove_rec(p: &Path) -> bool {
    if p.is_dir() {
        std::fs::remove_dir_all(p).is_ok()
    } else {
        std::fs::remove_file(p).is_ok()
    }
}

pub fn sweep_stale_py_runs(app: &AppHandle) {
    let root = py_runs_root();
    let removed = sweep_stale_py_runs_in(&root, Duration::from_secs(3600), SystemTime::now());
    if removed > 0 {
        py_audit(app, &format!("py_runs sweep | removed={removed}"));
    }
}

pub fn sweep_stale_py_runs_in(root: &Path, max_age: Duration, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        // 未来 mtime（时钟回拨/NTP 校正、或刚重建的目录晚于调用方采集的 now）
        // 会让 duration_since 得 None——按「刚创建」保留（age=0），等真实时间
        // 越过歪掉的 mtime 后自然恢复常规清扫；按过期删会误删新鲜目录
        let stale = e
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age >= max_age);
        if stale {
            let _ = std::fs::remove_dir_all(&p);
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 搬运进产物目录的文件不该带「来自互联网」标记。
    ///
    /// 搬运本身就是传播途径：同盘 rename 是同一个文件、标记原样跟着走；
    /// 跨盘拷贝走 CopyFileEx，复制全部备用数据流。产物落在 AI_Gen_Files 后
    /// 是要交给用户双击打开的，带着标记就会进 Office 受保护视图。
    #[cfg(windows)]
    #[test]
    fn harvested_file_loses_download_mark() {
        let run = tempfile::tempdir().expect("临时运行目录");
        let gen = tempfile::tempdir().expect("临时产物目录");
        let src = run.path().join("downloaded.docx");
        std::fs::write(&src, b"PK\x03\x04 payload").expect("写源文件");
        crate::platform::zone_id::test_mark_file(&src);
        assert!(crate::platform::zone_id::test_has_mark(&src));

        let mut audit = |_m: &str| {};
        harvest_run_outputs(run.path(), gen.path(), &mut audit);

        let dest = gen.path().join("downloaded.docx");
        assert!(dest.exists(), "产物没搬过来");
        assert!(
            !crate::platform::zone_id::test_has_mark(&dest),
            "搬过来的产物还带着「来自互联网」标记"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"PK\x03\x04 payload",
            "剥标记不该动主数据流"
        );
    }

    /// 非 Windows 平台搬运照常，标记那套是 no-op。
    #[test]
    #[cfg(not(windows))]
    fn harvest_moves_files_without_marks() {
        let run = tempfile::tempdir().expect("临时运行目录");
        let gen = tempfile::tempdir().expect("临时产物目录");
        std::fs::write(run.path().join("a.txt"), b"x").expect("写源文件");
        let mut msgs: Vec<String> = Vec::new();
        harvest_run_outputs(run.path(), gen.path(), &mut |m| msgs.push(m.to_string()));
        assert!(gen.path().join("a.txt").exists());
        assert!(
            !msgs.iter().any(|m| m.contains("失败")),
            "不该有失败审计：{msgs:?}"
        );
    }
}
