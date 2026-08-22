"""IPC server 测试 — JSON-RPC 2.0 协议 + 多 method + 错误处理。"""
from __future__ import annotations

import json

import pytest

from ipc.server import (
    IPCServer,
    PARSE_ERROR,
    INVALID_REQUEST,
    METHOD_NOT_FOUND,
    INVALID_PARAMS,
    INTERNAL_ERROR,
)
from tests.conftest import IPCTestHarness


# ───────────────────────── Handlers ─────────────────────────


def chat_handler(params):
    return {"echo": params.get("msg", "")}


def ping_handler(params):
    return {"pong": True}


def boom_handler(params):
    raise RuntimeError("boom from handler")


def type_error_handler(params):
    raise TypeError("bad signature")


@pytest.fixture
def server():
    return IPCServer(
        handlers={
            "chat": chat_handler,
            "ping": ping_handler,
            "boom": boom_handler,
            "type_err": type_error_handler,
        },
        ready_payload={"version": "0.1.0"},
    )


@pytest.fixture
def harness(server):
    return IPCTestHarness(server)


# ───────────────────────── 启动 ─────────────────────────


def test_ready_event_emitted_on_start(harness):
    """启动时先 emit ready 事件。"""
    harness.run()
    events = harness.events()
    assert len(events) >= 1
    assert events[0]["event"] == "ready"
    assert events[0]["version"] == "0.1.0"


def test_ready_payload_passed_through(harness):
    """ready 事件 payload 透传。"""
    harness.run()
    ready = harness.events()[0]
    assert ready["version"] == "0.1.0"


# ───────────────────────── 正常请求 ─────────────────────────


def test_valid_chat_request_returns_response(harness):
    """合法 chat 请求 → 返回响应。"""
    harness.send_request("chat", {"msg": "hi"}, id=1)
    harness.run()

    responses = harness.responses()
    assert len(responses) == 1
    assert responses[0]["id"] == 1
    assert responses[0]["jsonrpc"] == "2.0"
    assert responses[0]["result"] == {"echo": "hi"}


def test_multiple_requests_get_multiple_responses(harness):
    """多个请求 → 多个响应（按序）。"""
    harness.send_request("chat", {"msg": "a"}, id=1)
    harness.send_request("chat", {"msg": "b"}, id=2)
    harness.send_request("ping", {}, id=3)
    harness.run()

    responses = harness.responses()
    assert len(responses) == 3
    assert responses[0]["id"] == 1
    assert responses[1]["id"] == 2
    assert responses[2]["id"] == 3
    assert responses[2]["result"] == {"pong": True}


def test_handler_returning_none_response(harness):
    """handler 返回 None → response.result=None。"""
    def none_handler(params):
        return None
    harness.server.handlers["none"] = none_handler
    harness.send_request("none", {}, id=1)
    harness.run()

    responses = harness.responses()
    assert responses[0]["result"] is None


# ───────────────────────── 错误处理 ─────────────────────────


def test_invalid_json_returns_parse_error(harness):
    """非法 JSON → Parse error。"""
    harness.send_raw("this is not json")
    harness.run()

    responses = harness.responses()
    assert len(responses) == 1
    # Parse error 响应也带 id（即使 id 是 None）
    assert responses[0]["error"]["code"] == PARSE_ERROR


def test_non_object_json_returns_invalid_request(harness):
    """JSON 不是 object → Invalid Request。"""
    harness.send_raw("[1, 2, 3]")
    harness.run()

    responses = harness.responses()
    assert responses[0]["error"]["code"] == INVALID_REQUEST


def test_missing_jsonrpc_returns_invalid_request(harness):
    """缺 jsonrpc 字段 → Invalid Request。"""
    harness.send_raw('{"method": "chat", "id": 1}')
    harness.run()

    responses = harness.responses()
    assert responses[0]["error"]["code"] == INVALID_REQUEST
    assert responses[0]["id"] == 1


def test_wrong_jsonrpc_returns_invalid_request(harness):
    """jsonrpc 不是 "2.0" → Invalid Request。"""
    harness.send_raw('{"jsonrpc": "1.0", "method": "chat", "id": 1}')
    harness.run()

    responses = harness.responses()
    assert responses[0]["error"]["code"] == INVALID_REQUEST


def test_unknown_method_returns_method_not_found(harness):
    """未知 method → Method not found。"""
    harness.send_request("does_not_exist", {}, id=1)
    harness.run()

    responses = harness.responses()
    assert responses[0]["error"]["code"] == METHOD_NOT_FOUND
    assert "does_not_exist" in responses[0]["error"]["message"]


def test_handler_raises_returns_internal_error(harness):
    """handler 抛异常 → Internal error。"""
    harness.send_request("boom", {}, id=1)
    harness.run()

    responses = harness.responses()
    assert responses[0]["error"]["code"] == INTERNAL_ERROR
    assert "boom from handler" in responses[0]["error"]["message"]


def test_handler_type_error_returns_invalid_params(harness):
    """handler 抛 TypeError → Invalid params。"""
    harness.send_request("type_err", {}, id=1)
    harness.run()

    responses = harness.responses()
    assert responses[0]["error"]["code"] == INVALID_PARAMS


def test_params_not_dict_returns_invalid_params(harness):
    """params 不是 dict → Invalid params。"""
    harness.send_raw('{"jsonrpc": "2.0", "method": "chat", "id": 1, "params": [1, 2]}')
    harness.run()

    responses = harness.responses()
    assert responses[0]["error"]["code"] == INVALID_PARAMS


def test_missing_method_returns_invalid_request(harness):
    """缺 method 字段 → Invalid Request。"""
    harness.send_raw('{"jsonrpc": "2.0", "id": 1}')
    harness.run()

    responses = harness.responses()
    assert responses[0]["error"]["code"] == INVALID_REQUEST


def test_non_string_method_returns_invalid_request(harness):
    """method 不是字符串 → Invalid Request。"""
    harness.send_raw('{"jsonrpc": "2.0", "method": 123, "id": 1}')
    harness.run()

    responses = harness.responses()
    assert responses[0]["error"]["code"] == INVALID_REQUEST


# ───────────────────────── 通知（无 id） ─────────────────────────


def test_notification_no_response(harness):
    """notification（无 id）→ 不响应。"""
    harness.send_raw('{"jsonrpc": "2.0", "method": "chat", "params": {"msg": "x"}}')
    harness.run()

    responses = harness.responses()
    assert len(responses) == 0  # 没响应


def test_request_with_id_gets_response(harness):
    """有 id 的 request → 响应。"""
    harness.send_request("chat", {"msg": "x"}, id=42)
    harness.run()

    responses = harness.responses()
    assert len(responses) == 1
    assert responses[0]["id"] == 42


# ───────────────────────── event method ─────────────────────────


def test_event_method_calls_on_event_callback(harness):
    """event method 调 on_event 回调。"""
    captured = []

    def on_event(params):
        captured.append(params)

    server = IPCServer(
        handlers={"chat": chat_handler},
        on_event=on_event,
    )
    h = IPCTestHarness(server)
    h.send_event("tool.call", tool_name="read_file")
    h.run()

    assert len(captured) == 1
    assert captured[0]["event"] == "tool.call"
    assert captured[0]["tool_name"] == "read_file"


def test_event_method_no_response(harness):
    """event method → 不响应（只是回调）。"""
    events_captured = []

    def on_event(params):
        events_captured.append(params)

    server = IPCServer(
        handlers={"chat": chat_handler},
        on_event=on_event,
    )
    h = IPCTestHarness(server)
    h.send_event("skill.start")
    h.run()

    assert len(h.responses()) == 0
    assert len(events_captured) == 1


def test_event_method_error_doesnt_crash(harness):
    """on_event 抛异常 → 不崩（仅记 stderr）。"""
    def bad_on_event(params):
        raise RuntimeError("boom")

    server = IPCServer(
        handlers={"chat": chat_handler},
        on_event=bad_on_event,
    )
    h = IPCTestHarness(server)
    h.send_event("x")
    h.run()  # 不应崩


# ───────────────────────── Run 行为 ─────────────────────────


def test_empty_stdin_no_crash(harness):
    """空 stdin → 不崩，0 响应。"""
    harness.run()
    assert len(harness.responses()) == 0


def test_whitespace_lines_skipped(harness):
    """纯空白行被跳过。"""
    harness.stdin.write("\n")
    harness.stdin.write("   \n")
    harness.stdin.write("\t\n")
    harness.send_request("chat", {"msg": "x"}, id=1)
    harness.run()

    responses = harness.responses()
    assert len(responses) == 1


def test_invalid_request_does_not_block_subsequent(harness):
    """一个请求失败不影响后续。"""
    harness.send_raw("not json")
    harness.send_request("chat", {"msg": "ok"}, id=1)
    harness.run()

    responses = harness.responses()
    # 第一个 invalid_json → error response，第二个 ok → result response
    assert len(responses) == 2
    assert any(r.get("error") for r in responses)
    assert any(r.get("result") for r in responses)


# ───────────────────────── emit_event ─────────────────────────


def test_emit_event_writes_ndjson():
    """emit_event 写一行 JSON 到 stdout。"""
    import io
    from ipc.server import IPCServer

    server = IPCServer(handlers={})
    buf = io.StringIO()
    server._emit_event_to(buf, "skill.start", skill_name="x")

    output = buf.getvalue().strip()
    assert output.endswith("\n") is False  # 只一行不含尾换行？实际有 — 跳到 read splitlines
    lines = output.split("\n")
    assert len(lines) == 1
    parsed = json.loads(lines[0])
    assert parsed["event"] == "skill.start"
    assert parsed["skill_name"] == "x"


def test_emit_event_serializes_non_str_values():
    """emit_event 默认用 default=str 序列化非 str 值。"""
    import io
    from ipc.server import IPCServer

    server = IPCServer(handlers={})
    buf = io.StringIO()
    server._emit_event_to(buf, "x", ts=1234567890123, items=[1, 2, 3])

    parsed = json.loads(buf.getvalue())
    assert parsed["ts"] == 1234567890123
    assert parsed["items"] == [1, 2, 3]


# ───────────────────────── stop ─────────────────────────


def test_stop_returns_to_caller():
    """stop() 后主循环退出。"""
    import io
    from ipc.server import IPCServer

    server = IPCServer(handlers={"chat": chat_handler})
    stdin = io.StringIO()
    stdout = io.StringIO()
    stderr = io.StringIO()

    # 喂数据 + stop 在前 — 但 stop 是 method，只能从外部调
    # 测试：先发请求，handler 里调 stop
    def stop_in_handler(params):
        server.stop()
        return {"stopped": True}

    server.handlers["stop"] = stop_in_handler
    stdin.write(json.dumps({"jsonrpc": "2.0", "method": "stop", "id": 1}) + "\n")
    stdin.seek(0)
    server.run(stdin, stdout, stderr)

    responses = [json.loads(line) for line in stdout.getvalue().splitlines() if line.strip() and "id" in line]
    assert len(responses) == 1
    assert responses[0]["result"]["stopped"] is True
