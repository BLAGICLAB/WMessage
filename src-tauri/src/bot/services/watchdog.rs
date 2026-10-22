//! 受管服务的「宿主死了，服务也得死」看门狗。
//!
//! 沙箱层（`py/runtime.rs`）早就有同类机制：Windows 用 Job Object
//! （`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`——宿主进程一死，内核就杀掉整组），
//! Unix 往注入脚本里塞一段 `getppid()` 轮询。服务层不能照抄注入脚本那招：
//! 受管服务是任意第三方程序（node / npx / python…），没法往里塞代码。
//!
//! **unix**：用「管道 EOF」当宿主死亡信号。宿主持有管道写端，看门狗助手拿读端
//! 当 stdin；宿主一旦非正常死亡（SIGKILL / OOM / 崩溃），内核关闭它的 fd →
//! 写端随之关闭 → 助手的 `read` 立刻收到 EOF → 它去 TERM 服务进程组。
//!
//! 为什么不用 `prctl(PR_SET_PDEATHSIG)`：
//! 一是那是 Linux 专有，macOS 根本没有；
//! 二是它按「父**线程**」算——服务若从一个会退休的线程 spawn（工具链正是
//! 用 blocking 线程池 spawn 服务的），那个线程一退出服务就被误杀，看门狗
//! 反倒成了杀手。
//!
//! **Windows**：复用沙箱层那套 Job Object（同一份实现，见 `py::runtime::win_job`）
//! ——把服务进程塞进一个 `KILL_ON_JOB_CLOSE` 的 job，宿主进程一死，job 句柄
//! 随进程关闭，内核杀掉整组。
//!
//! 看门狗按服务 id 登记在**进程级表**里（不是服务登记条目的一个字段）：
//! 句柄含管道 fd / job 句柄，两者都没有 Clone——塞进服务条目就得给那条
//! `#[derive(Clone)]` 开口子，而那条 Clone 并没有别的用处。

use std::collections::HashMap;
use std::process::Child;
use std::sync::{LazyLock, Mutex};

/// 服务 id → 看门狗。进程级单例：看门狗的存活条件是「宿主还活着」，
/// 而宿主活着与否与某个服务条目无关。
static WATCHDOGS: LazyLock<Mutex<HashMap<String, Watchdog>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 给刚 spawn 的服务装上看门狗。
///
/// `service_pgid` 是服务进程组 id（unix：spawn 时 `process_group(0)`，故
/// 等于服务 pid）；`child` 是服务进程本身（Windows 侧要它的句柄）。
/// 装不上（缺 /bin/sh、job 创建失败等）就不登记——**不因看门狗装不上而
/// 拒绝服务启动**：看门狗是兜底，不是服务能不能跑的前提。
pub fn attach(service_id: &str, service_pgid: i32, child: &Child) -> bool {
    let wd = match Watchdog::create(service_pgid, child) {
        Some(w) => w,
        None => return false,
    };
    let mut map = WATCHDOGS.lock().unwrap_or_else(|e| e.into_inner());
    map.insert(service_id.to_string(), wd);
    true
}

/// 服务停了 / 自己退了：收掉看门狗助手（别留一个空转的 sh 进程）。
pub fn disarm(service_id: &str) {
    let mut map = WATCHDOGS.lock().unwrap_or_else(|e| e.into_inner());
    // 移除即 Drop：Drop 里杀助手进程
    map.remove(service_id);
}

/// 当前登记着的看门狗数（测试与诊断用）。
#[cfg(test)]
pub fn registered_count() -> usize {
    WATCHDOGS.lock().unwrap_or_else(|e| e.into_inner()).len()
}

/// 看门狗句柄。宿主活着时它什么都不做，宿主一死它就把服务进程组收掉。
///
/// unix 侧的「管道写端」是这个句柄的关键：写端在宿主进程里，宿主一死内核
/// 就把它关掉，助手（服务之外的另一个进程）因此收到 EOF。
pub struct Watchdog {
    /// unix 看门狗（字段名带下划线 = 只为持有它，Drop 里收助手）。
    #[cfg(unix)]
    _unix: unix::UnixWatchdog,
    #[cfg(windows)]
    _win: windows::WinWatchdog,
}

impl Watchdog {
    fn create(service_pgid: i32, child: &Child) -> Option<Self> {
        #[cfg(unix)]
        {
            let _ = child;
            unix::attach(service_pgid).map(|u| Self { _unix: u })
        }
        #[cfg(windows)]
        {
            let _ = service_pgid;
            windows::attach(child).map(|w| Self { _win: w })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (service_pgid, child);
            None
        }
    }
}

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::os::unix::io::FromRawFd;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};

    pub struct UnixWatchdog {
        /// 看门狗助手（/bin/sh）进程句柄：收尾时要 wait 它，否则留下僵尸。
        pub(super) helper: Child,
        /// 管道写端：宿主持有，它一关，助手就收到 EOF。
        /// Option 是为了能单独关掉它（宿主死亡 / 服务收尾）。
        pub(super) write_end: Option<File>,
    }

    impl UnixWatchdog {
        /// 关掉写端 = 宿主死了（内核关 fd 的效果）。助手随即 TERM 服务进程组。
        ///
        /// 真实路径上有两处会走到：宿主进程消失（内核替我们关）与服务收尾
        /// （看门狗登记表移除 → Drop → 收助手 + 关写端）。
        pub(super) fn close_write_end(&mut self) {
            self.write_end = None;
        }
    }

    impl Drop for UnixWatchdog {
        fn drop(&mut self) {
            // 助手只是「等宿主死亡」的哨兵：宿主还活着而服务收尾时，先把它收掉
            // （kill + wait：wait 才是回收，不 wait 会留一个僵尸进程）。
            // 助手自成进程组，杀它不会牵连宿主所在的组。
            let _ = self.helper.kill();
            let _ = self.helper.wait();
            self.close_write_end();
        }
    }

    pub fn attach(service_pgid: i32) -> Option<UnixWatchdog> {
        let (read_end, write_end) = pipe()?;
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            // 阻塞读一行：宿主活着时管道里没有数据，读一直挂着；
            // 宿主一死 → 写端关闭 → read 返回 EOF → 往下走杀进程组。
            .arg("read _ 2>/dev/null; kill -TERM -\"$1\" 2>/dev/null")
            .arg("wm-service-watchdog")
            .arg(service_pgid.to_string())
            .stdin(Stdio::from(read_end))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // 自成进程组：收看门狗时不会连带宿主所在的组（宿主可能在别人的组里，
        // 按组杀会连宿主一起带走）。
        cmd.process_group(0);
        let child = cmd.spawn().ok()?;
        Some(UnixWatchdog {
            helper: child,
            write_end: Some(write_end),
        })
    }

    /// 建一对管道（读端 / 写端），两端都带 FD_CLOEXEC。
    ///
    /// CLOEXEC 是必须的：看门狗助手绝不能继承**写端**——它一旦继承了，
    /// 宿主死了写端也还开着，EOF 永远不来，看门狗就成了摆设。
    fn pipe() -> Option<(File, File)> {
        let mut fds = [0i32; 2];
        // SAFETY: pipe() 成功时填两个合法 fd；失败返回 -1（不碰 fds）。
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return None;
        }
        for fd in fds {
            // SAFETY: fd 是刚拿到的合法 fd；F_SETFD 改的是这个 fd 自己的标志位。
            unsafe {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        // SAFETY: 两个 fd 刚从 pipe() 得到，尚未交给任何 File，ownership 转移给这里。
        let read_end = unsafe { File::from_raw_fd(fds[0]) };
        let write_end = unsafe { File::from_raw_fd(fds[1]) };
        Some((read_end, write_end))
    }
}

#[cfg(windows)]
mod windows {
    use std::process::Child;

    pub struct WinWatchdog {
        /// Job 句柄：宿主持有它，宿主一死句柄随进程关闭 → job 里的进程全灭。
        _job: crate::py::runtime::win_job::JobGuard,
    }

    pub fn attach(child: &Child) -> Option<WinWatchdog> {
        let job = crate::py::runtime::win_job::create_kill_on_close()?;
        crate::py::runtime::win_job::assign(&job, child);
        Some(WinWatchdog { _job: job })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// 看门狗的核心语义：**写端一关（= 宿主死了），服务进程组就被收掉**。
    ///
    /// 这条不依赖「真的杀死宿主」——直接关掉写端就等价于宿主进程被内核
    /// 清理掉时发生的那件事（进程的所有 fd 被内核关闭）。macOS/Linux 上可实测。
    #[cfg(unix)]
    #[test]
    fn closing_the_write_end_kills_the_service_group() {
        use std::os::unix::process::CommandExt;
        // 起一个自己的组当"服务"：组 id = 它的 pid
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("30");
        cmd.process_group(0);
        let mut svc = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("应能起 sleep");
        let pgid = svc.id() as i32;
        assert!(crate::bot::procs::pid_exists(svc.id()), "服务此刻应活着");

        // 装看门狗
        let mut wd = unix::attach(pgid).expect("看门狗应装得上");
        assert!(
            crate::bot::procs::pid_exists(svc.id()),
            "看门狗待命期间服务不该被动"
        );

        // 关键动作：关掉写端 = 宿主死了（助手收到 EOF → 去杀进程组）
        wd.close_write_end();
        // 判「服务死了」要用 wait（回收）而不是 pid_exists：服务是本进程的
        // 子进程，被杀后未回收前是**僵尸**，而 kill(pid, 0) 对僵尸照样返回成功
        // ——按 pid_exists 判会把「已经死了」读成「还活着」。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut exited = false;
        while std::time::Instant::now() < deadline {
            if svc.try_wait().ok().flatten().is_some() {
                exited = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(exited, "写端关闭（宿主死亡）后看门狗应杀掉服务进程组");
        let _ = svc.kill();
        let _ = svc.wait();
    }

    /// 反向：看门狗在宿主活着时**不许**动服务（管道还开着，助手一直挂着）。
    #[cfg(unix)]
    #[test]
    fn watchdog_does_not_touch_the_service_while_the_host_is_alive() {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("30");
        cmd.process_group(0);
        let mut svc = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("应能起 sleep");
        let pgid = svc.id() as i32;
        let wd = unix::attach(pgid).expect("看门狗应装得上");
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(
            crate::bot::procs::pid_exists(svc.id()),
            "宿主还活着时看门狗不许杀服务"
        );
        drop(wd);
        let _ = svc.kill();
        let _ = svc.wait();
    }

    /// 登记表：attach 登记、disarm 移除（助手随之收掉）。
    #[cfg(unix)]
    #[test]
    fn attach_and_disarm_maintain_the_registry() {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("5");
        cmd.process_group(0);
        let mut svc = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("应能起 sleep");
        let pgid = svc.id() as i32;
        let before = registered_count();
        assert!(attach("wd-test", pgid, &svc), "看门狗应装得上并登记");
        assert_eq!(registered_count(), before + 1);
        disarm("wd-test");
        assert_eq!(registered_count(), before, "disarm 后不该还挂着");
        let _ = svc.kill();
        let _ = svc.wait();
    }
}
