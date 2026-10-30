//! Python 并发闸（有界并发信号量 + EXITING / 停止复查）。
//!
//! 从 `runtime.rs` 拆出：纯闸门语义，与执行器（run_python_at）无依赖——
//! EXITING 标志仍在 runtime，这里按 `super::runtime::` 反向引用。

use std::sync::Mutex;

use crate::bot::StopToken;
use crate::error::CommandError;

use super::runtime::EXITING;

/// Python 并发闸从互斥锁改**有界并发**（信号量语义）——std Mutex 全程
/// 持锁把所有 Python 任务串行化，一个长任务能把后面全部排队堵死。许可数 2：
/// 并行 run 各自独立子进程 + 独立产物目录，2 是 CPU/内存争用的保守上界。
pub(crate) const PY_RUN_MAX_CONCURRENT: usize = 2;

/// 可用许可数（Condvar 信号量：run_python 跑在 spawn_blocking 阻塞线程上，
/// 保持同步 API；等待带 200ms 超时轮询以便复查 EXITING）
pub(crate) static PY_PERMITS: Mutex<usize> = Mutex::new(PY_RUN_MAX_CONCURRENT);
static PY_PERMITS_CV: std::sync::Condvar = std::sync::Condvar::new();

/// 闸门许可句柄（RAII：Drop 归还许可并唤醒一个等待者）。
/// acquired=false = 因应用退出拒发（不是真持有），Drop 无副作用。
pub struct PyGateGuard {
    acquired: bool,
}

impl Drop for PyGateGuard {
    fn drop(&mut self) {
        if self.acquired {
            let mut avail = crate::db::lock::lock_or_recover(&PY_PERMITS, "py::gate::PY_PERMITS");
            *avail += 1;
            drop(avail);
            PY_PERMITS_CV.notify_one();
        }
    }
}

/// 取一个 Python 执行许可（阻塞至有空位）。EXITING 三点复查（原「拿锁后复查」
/// 语义平移）：等待前、每轮唤醒后（循环顶）、拿到许可后终检——排队者过闸即拒。
/// pub(crate)：py/document 的 doc 生成流持一个许可跨 dotnet+python 兜底
///（内部 run_python_ungated 不重复计闸）。
pub(crate) fn py_gate_acquire() -> Result<PyGateGuard, CommandError> {
    py_gate_acquire_inner(true, None)
}

/// 同上，另接停止令牌：排队等许可期间每轮唤醒（200ms 粒度）顺带查一次——
/// 两个许可被长任务占满时，/stop 的排队者最坏要再等 2×600s 才拿闸，那时
/// 才响应停止等于白等。拿到许可就跑、因停止而退出的过闸即拒。
pub(crate) fn py_gate_acquire_with_stop(
    stop: Option<&StopToken>,
) -> Result<PyGateGuard, CommandError> {
    py_gate_acquire_inner(true, stop)
}

fn py_gate_acquire_inner(
    check_exiting: bool,
    stop: Option<&StopToken>,
) -> Result<PyGateGuard, CommandError> {
    let mut avail = crate::db::lock::lock_or_recover(&PY_PERMITS, "py::gate::PY_PERMITS");
    loop {
        // EXITING 复查①：本轮循环顶（= 等待前 / 每轮唤醒后）；停止同点复查
        if check_exiting && EXITING.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(CommandError::DomainRule {
                domain: "python".to_string(),
                reason: "应用正在退出，不再启动新的 Python 任务".to_string(),
            });
        }
        if stop.is_some_and(|t| t.stopped()) {
            return Err(CommandError::DomainRule {
                domain: "python".to_string(),
                reason: "执行已被用户停止，不再排队等待 Python 许可".to_string(),
            });
        }
        if *avail > 0 {
            // EXITING 复查②（拿到许可后终检）：等待期间退出标志可能置位，
            // 置位即归还许可并拒绝——不 spawn 无人收割的孤儿进程
            if check_exiting && EXITING.load(std::sync::atomic::Ordering::SeqCst) {
                *avail += 1;
                drop(avail);
                PY_PERMITS_CV.notify_one();
                return Err(CommandError::DomainRule {
                    domain: "python".to_string(),
                    reason: "应用正在退出，不再启动新的 Python 任务".to_string(),
                });
            }
            *avail -= 1;
            return Ok(PyGateGuard { acquired: true });
        }
        let (g, _timeout) = PY_PERMITS_CV
            .wait_timeout(avail, std::time::Duration::from_millis(200))
            .unwrap_or_else(|e| {
                eprintln!("[mutex_poisoned] py::gate::PY_PERMITS_CV: {e:?}");
                e.into_inner()
            });
        avail = g;
    }
}

/// 测试入口：同一条许可获取路径、不查 EXITING（并行测试里其他用例可能置位）。
/// 阻塞式取许可（非 try 语义），名字按评审 M 采纳与行为对齐。
#[cfg(test)]
pub(crate) fn py_gate_acquire_for_test() -> PyGateGuard {
    py_gate_acquire_inner(false, None).expect("非退出态取许可必须成功")
}

/// 闸门测试的串行锁。
///
/// 许可池（`PY_PERMITS`）与 `EXITING` 标志是**进程级全局**的，而 Rust 测试默认
/// 并行跑：两个用例各拿许可会互相看见「只剩 1 个」（断言漂移），一个用例读到的
/// 许可数被另一个改掉，读到 0 再去减一就直接下溢 panic。碰这两样全局的用例都
/// 要先拿这把锁——锁本身是测试设施，中毒后照常取用（见 `gate_test_lock`）。
#[cfg(test)]
pub(crate) static GATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 取闸门测试锁。守卫活到用例结束即自动释放。
#[cfg(test)]
pub(crate) fn gate_test_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::db::lock::lock_or_recover(&GATE_TEST_LOCK, "py::gate::GATE_TEST_LOCK")
}
