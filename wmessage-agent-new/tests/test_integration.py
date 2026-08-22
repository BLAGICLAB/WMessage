"""集成测试 — spawn main.py 子进程，stdin/stdout 真实通信。"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time

import pytest

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAIN_PY = os.path.join(PROJECT_ROOT, "main.py")


def _spawn_agent():
    """spawn main.py 子进程。"""
    env = os.environ.copy()
    env["PYTHONPATH"] = PROJECT_ROOT + os.pathsep + env.get("PYTHONPATH", "")
    return subprocess.Popen(
        [sys.executable, MAIN_PY],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,  # 行缓冲
        env=env,
        cwd=PROJECT_ROOT,
    )


def _send(proc, msg):
    """发一个 JSON-RPC 消息。"""
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()


def _read_line(proc, timeout_s=5):
    """读一行 stdout，超时返回 None。"""
    import select

    if os.name == "posix":
        ready, _, _ = select.select([proc.stdout], [], [], timeout_s)
        if not ready:
            return None
    return proc.stdout.readline()


def _read_until_response(proc, req_id, timeout_s=5):
    """读到带指定 id 的响应为止。

    实现：每个 _read_line 调用给 1s 独立 budget（不递减），
    总超时由外层 deadline 守门。避免压力测试下递减 timeout 导致后续 readline 可用时间被吃完。
    """
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        line = _read_line(proc, timeout_s=1.0)
        if line is None:
            continue  # 1s readline 超时但总 deadline 还有 → 继续
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg.get("id") == req_id:
            return msg
    return None


def _cleanup(proc):
    """优雅关闭子进程。"""
    try:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
    except Exception:
        pass


# ───────────────────────── 端到端 ─────────────────────────


@pytest.mark.slow
def test_spawn_and_ready_event():
    """spawn 后立刻收到 ready 事件。"""
    proc = _spawn_agent()
    try:
        line = _read_line(proc, timeout_s=5)
        assert line is not None, "启动超时"
        msg = json.loads(line)
        assert msg["event"] == "ready"
        assert msg["version"] == "0.2.0"
        assert msg["phase"] == 2
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_chat_end_to_end():
    """完整 chat 流程：spawn → 发 chat → 收到 PlanResult + LoopResult。"""
    proc = _spawn_agent()
    try:
        # 跳过 ready
        _read_line(proc, timeout_s=5)

        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "chat",
                "params": {
                    "message": "实现 hello world 函数。写单元测试。",
                },
            },
        )
        resp = _read_until_response(proc, 1, timeout_s=10)
        assert resp is not None, "chat 响应超时"
        assert resp["id"] == 1
        result = resp["result"]
        # Phase 2 返回结构：plan + loop
        assert "plan" in result
        assert "loop" in result
        assert len(result["plan"]["graph"]["tasks"]) >= 2  # 至少 2 个 task
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_ping_pong():
    """ping → pong。"""
    proc = _spawn_agent()
    try:
        _read_line(proc, timeout_s=5)  # skip ready
        _send(proc, {"jsonrpc": "2.0", "id": 1, "method": "ping", "params": {}})
        resp = _read_until_response(proc, 1, timeout_s=5)
        assert resp is not None
        assert resp["result"]["pong"] is True
        assert isinstance(resp["result"]["ts"], int)
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_multiple_round_trips():
    """多轮请求。"""
    proc = _spawn_agent()
    try:
        _read_line(proc, timeout_s=5)

        for i in range(3):
            _send(proc, {"jsonrpc": "2.0", "id": i, "method": "ping", "params": {}})
            resp = _read_until_response(proc, i, timeout_s=5)
            assert resp is not None
            assert resp["id"] == i
            assert resp["result"]["pong"] is True
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_shutdown_gracefully():
    """shutdown → 进程退出码 0。

    模拟 Rust 父进程真实行为：发完 shutdown 后关闭 stdin
    （让子进程的 for line in stdin 读到 EOF 自然退出循环）。
    """
    proc = _spawn_agent()
    _read_line(proc, timeout_s=5)  # skip ready
    _send(proc, {"jsonrpc": "2.0", "id": 1, "method": "shutdown", "params": {}})
    proc.stdin.close()  # 模拟 Rust 父进程关闭 stdin

    # 等待响应 + 进程退出
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        _cleanup(proc)
        pytest.fail("shutdown 后进程未在 3s 内退出")

    assert proc.returncode == 0


@pytest.mark.slow
def test_invalid_input_reports_error_continues():
    """非法 JSON → error 响应 + 进程继续。"""
    proc = _spawn_agent()
    try:
        _read_line(proc, timeout_s=5)  # skip ready

        # 发非法 JSON
        proc.stdin.write("not json\n")
        proc.stdin.flush()

        # 读 error 响应
        line = _read_line(proc, timeout_s=5)
        assert line is not None
        msg = json.loads(line)
        assert "error" in msg
        assert msg["error"]["code"] == -32700

        # 后续 ping 应正常工作
        _send(proc, {"jsonrpc": "2.0", "id": 100, "method": "ping", "params": {}})
        resp = _read_until_response(proc, 100, timeout_s=5)
        assert resp is not None
        assert resp["result"]["pong"] is True
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_chat_with_skill_body():
    """chat + skillBody → 整段作为 1 个 task（Phase 2 + PREV 主循环端到端）。"""
    proc = _spawn_agent()
    try:
        _read_line(proc, timeout_s=5)

        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "chat",
                "params": {
                    "message": "演示任务归档",
                    "skillBody": "# 归档演示\n调用 list_tasks 拿所有任务",
                },
            },
        )
        resp = _read_until_response(proc, 1, timeout_s=10)
        assert resp is not None
        result = resp["result"]
        # Phase 2: 返回完整 PREV 主循环结果
        assert "plan" in result
        assert "loop" in result
        assert "audit_lines" in result
        # Skill body → 整段 1 task
        assert len(result["plan"]["graph"]["tasks"]) == 1
        # mock LLM fallback → task_done → OK
        assert result["loop"]["status"] in {"ok", "await_user"}
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_chat_invalid_params_returns_error_in_result():
    """chat 参数非法 → 不抛 error 响应，result 里返回 error 字段。"""
    proc = _spawn_agent()
    try:
        _read_line(proc, timeout_s=5)

        # maxTasks=0 → ValueError
        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "chat",
                "params": {"message": "x", "maxTasks": 0},
            },
        )
        resp = _read_until_response(proc, 1, timeout_s=5)
        assert resp is not None
        # handler 把异常包成 {"error": ..., "kind": "ValueError"}
        assert "error" in resp["result"]
        assert resp["result"]["kind"] == "ValueError"
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_chat_ready_event_reports_phase_2():
    """ready 事件 phase=2。"""
    proc = _spawn_agent()
    try:
        line = _read_line(proc, timeout_s=5)
        msg = json.loads(line)
        assert msg["event"] == "ready"
        assert msg["phase"] == 2
        assert msg["version"] == "0.2.0"
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_chat_full_prev_loop_with_audit():
    """完整 PREV 主循环跑通 + 审计事件。"""
    proc = _spawn_agent()
    try:
        _read_line(proc, timeout_s=5)

        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "chat",
                "params": {
                    "message": "A. B. C.",
                    "maxTasks": 5,
                },
            },
        )
        resp = _read_until_response(proc, 1, timeout_s=10)
        assert resp is not None
        result = resp["result"]

        # plan
        assert "plan" in result
        assert len(result["plan"]["graph"]["tasks"]) == 3  # 3 个句子

        # loop result
        assert "loop" in result
        loop = result["loop"]
        # mock LLM 自动 task_done → OK
        assert loop["status"] == "ok"
        assert len(loop["completed_tasks"]) == 3
        assert loop["replan_count"] == 0

        # audit_lines: 至少 skill.start + 3 task.done + skill.finish
        audit = "\n".join(result["audit_lines"])
        assert "skill.start" in audit
        assert "skill.finish" in audit
        assert audit.count("task.done") >= 3
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_ping_under_load():
    """高频 ping 应都正常返回。

    注：Python agent 单线程逐条处理 stdin，10 个连续 ping 顺序处理实测 ~3s/ping，
    10s/30s 总预算都不够。用 60s 兑底。生产环境 Tauri Rust 侧会异步处理不有此问题。
    """
    proc = _spawn_agent()
    try:
        _read_line(proc, timeout_s=5)
        for i in range(10):
            _send(proc, {"jsonrpc": "2.0", "id": i, "method": "ping", "params": {}})
        for i in range(10):
            resp = _read_until_response(proc, i, timeout_s=60)
            assert resp is not None, f"ping #{i} 超时"
            assert resp["result"]["pong"] is True
    finally:
        _cleanup(proc)


# ───────────────────────── Builtin Skill 路由集成测试 ─────────────────────────


@pytest.mark.slow
def test_chat_intent_routes_to_docx_skill():
    """chat 查询触发 minimax-docx intent router → PREV 跑通。"""
    proc = _spawn_agent()
    try:
        _read_line(proc, timeout_s=5)
        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "chat",
                "params": {"message": "帮我生成一个 Word 文档，标题是季度总结"},
            },
        )
        resp = _read_until_response(proc, 1, timeout_s=10)
        assert resp is not None

        result = resp["result"]
        # 命中 docx skill
        assert result["routed_skill"] == "minimax-docx"
        # plan 含 1 task（skill body 整段）
        assert len(result["plan"]["graph"]["tasks"]) == 1
        # loop 跑通
        assert result["loop"]["status"] in {"ok", "await_user"}
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_chat_intent_routes_to_xlsx_skill():
    """'做个表格' → minimax-xlsx。"""
    proc = _spawn_agent()
    try:
        _read_line(proc, timeout_s=5)
        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "chat",
                "params": {"message": "做个 excel 表格汇总"},
            },
        )
        resp = _read_until_response(proc, 1, timeout_s=10)
        assert resp is not None
        result = resp["result"]
        assert result["routed_skill"] == "minimax-xlsx"
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_chat_intent_routes_to_pptx_skill():
    """'做个 PPT' → minimax-pptx。"""
    proc = _spawn_agent()
    try:
        _read_line(proc, timeout_s=5)
        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "chat",
                "params": {"message": "做一个 ppt 演示文稿"},
            },
        )
        resp = _read_until_response(proc, 1, timeout_s=10)
        assert resp is not None
        result = resp["result"]
        assert result["routed_skill"] == "minimax-pptx"
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_chat_no_intent_route():
    """未命中任何 builtin skill → routed_skill=null。"""
    proc = _spawn_agent()
    try:
        _read_line(proc, timeout_s=5)
        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "chat",
                "params": {"message": "讲个笑话"},
            },
        )
        resp = _read_until_response(proc, 1, timeout_s=10)
        assert resp is not None
        result = resp["result"]
        assert result["routed_skill"] is None
    finally:
        _cleanup(proc)


@pytest.mark.slow
def test_explicit_skill_body_overrides_intent_route():
    """显式传入 skillBody → 跳过 intent router。"""
    proc = _spawn_agent()
    try:
        _read_line(proc, timeout_s=5)
        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "chat",
                "params": {
                    "message": "做个 word",
                    "skillBody": "# 自定义 skill\n走自己的逻辑",
                },
            },
        )
        resp = _read_until_response(proc, 1, timeout_s=10)
        assert resp is not None
        result = resp["result"]
        assert result["routed_skill"] is None
    finally:
        _cleanup(proc)
