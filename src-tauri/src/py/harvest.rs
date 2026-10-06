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
        if name == "run.py" || name == "params.json" {
            continue;
        }
        let src = e.path();
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
    if src.is_dir() {
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
