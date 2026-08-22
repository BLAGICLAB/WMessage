"""JSON-RPC 2.0 server over stdin/stdout。

协议格式：
- 请求/响应：每行一个 JSON 对象
  - 请求：  {"jsonrpc": "2.0", "id": "...", "method": "...", "params": {...}}
  - 响应：  {"jsonrpc": "2.0", "id": "...", "result": {...}} | {"error": {"code": ..., "message": ..., "data": ...}}
- 通知（无 id）：服务端不响应
- 事件推送：{"event": "skill.start", ...}（无 id，区别于响应）

JSON-RPC 2.0 错误码：
- -32700 Parse error
- -32600 Invalid Request
- -32601 Method not found
- -32602 Invalid params
- -32603 Internal error
- -32000 ~ -32099 自定义（业务错误）

参见 wmessage-agent-spec.md §Y.4 错误码体系。
"""
from __future__ import annotations

import json
import sys
from typing import Any, Callable, Dict, IO, Optional

# JSON-RPC 2.0 标准错误码
PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_ERROR = -32603


class IPCError(Exception):
    """IPC server / protocol 错误。"""


class IPCServer:
    """JSON-RPC 2.0 server — stdin/stdout 一行一消息。

    使用方式：
        server = IPCServer({"chat": chat_handler, "ping": ping_handler})
        server.run()             # 默认用 sys.stdin / sys.stdout
        server.run(stdin, stdout) # 测试时可注入 stream

    线程模型：单线程顺序处理 stdin 各行 + emit_event 写 stdout（Phase 1 简化版）。
    后续 Phase 2 可加 reader thread 分离读写。
    """

    def __init__(
        self,
        handlers: Dict[str, Callable[[Dict[str, Any]], Any]],
        ready_payload: Optional[Dict[str, Any]] = None,
        on_event: Optional[Callable[[Dict[str, Any]], None]] = None,
    ):
        """初始化。

        Args:
            handlers: method → handler(params) dict
            ready_payload: 启动后 emit_event("ready", **ready_payload) 的 payload
            on_event: 收到 event 时的回调（默认 print 到 stderr，仅调试用）
        """
        self.handlers = handlers
        self.ready_payload = ready_payload or {}
        self.on_event = on_event
        self._stopped = False

    # ──────────────── 主循环 ────────────────

    def run(
        self,
        stdin: Optional[IO[str]] = None,
        stdout: Optional[IO[str]] = None,
        stderr: Optional[IO[str]] = None,
    ) -> None:
        """主循环：读 stdin → 处理 → 写 stdout / event。"""
        stdin = stdin if stdin is not None else sys.stdin
        stdout = stdout if stdout is not None else sys.stdout
        stderr = stderr if stderr is not None else sys.stderr

        self._emit_event_to(stdout, "ready", **self.ready_payload)

        for line in stdin:
            if self._stopped:
                break
            line = line.strip()
            if not line:
                continue
            self._handle_line(line, stdout, stderr)
            # 处理后立即检查 stop（处理 shutdown / 致命错误后立即退出，
            # 不再阻塞在 stdin.readline() 等下一行）
            if self._stopped:
                break

    def stop(self) -> None:
        """请求停止（主循环在处理完当前行后退出）。"""
        self._stopped = True

    # ──────────────── 单行处理 ────────────────

    def _handle_line(
        self, line: str, stdout: IO[str], stderr: IO[str]
    ) -> None:
        try:
            msg = json.loads(line)
        except json.JSONDecodeError as e:
            self._send_error_to(stdout, None, PARSE_ERROR, f"Parse error: {e}")
            return

        if not isinstance(msg, dict):
            self._send_error_to(stdout, None, INVALID_REQUEST, "Invalid Request: not a JSON object")
            return

        # jsonrpc 字段
        if msg.get("jsonrpc") != "2.0":
            self._send_error_to(
                stdout, msg.get("id"), INVALID_REQUEST, "Invalid Request: jsonrpc must be '2.0'"
            )
            return

        # method 字段
        method = msg.get("method")
        if not method or not isinstance(method, str):
            self._send_error_to(
                stdout, msg.get("id"), INVALID_REQUEST, "Invalid Request: missing or invalid method"
            )
            return

        # 事件推送（method 是 "event"）— 不响应
        if method == "event":
            if self.on_event is not None:
                try:
                    self.on_event(msg.get("params", {}))
                except Exception as e:  # noqa: BLE001
                    print(f"[ipc] event handler error: {e}", file=stderr)
            return

        # method dispatch
        handler = self.handlers.get(method)
        if handler is None:
            self._send_error_to(
                stdout, msg.get("id"), METHOD_NOT_FOUND, f"Method not found: {method}"
            )
            return

        params = msg.get("params", {})
        if params is not None and not isinstance(params, dict):
            self._send_error_to(
                stdout, msg.get("id"), INVALID_PARAMS, "Invalid params: must be object"
            )
            return

        try:
            result = handler(params or {})
        except TypeError as e:
            # handler 签名错（如 params 期望 dict 但传了别的）
            self._send_error_to(stdout, msg.get("id"), INVALID_PARAMS, f"Invalid params: {e}")
            return
        except Exception as e:  # noqa: BLE001
            self._send_error_to(stdout, msg.get("id"), INTERNAL_ERROR, f"Internal error: {e}")
            return

        # 通知（无 id）不响应；请求（有 id）响应
        if "id" in msg:
            self._send_result_to(stdout, msg["id"], result)

    # ──────────────── 输出 ────────────────

    def emit_event(self, event: str, **kwargs: Any) -> None:
        """写入事件到 sys.stdout。"""
        self._emit_event_to(sys.stdout, event, **kwargs)

    def _emit_event_to(self, stdout: IO[str], event: str, **kwargs: Any) -> None:
        d = {"event": event, **kwargs}
        try:
            stdout.write(json.dumps(d, default=str) + "\n")
            stdout.flush()
        except Exception:  # noqa: BLE001
            pass

    def _send_result_to(
        self, stdout: IO[str], id: Any, result: Any
    ) -> None:
        d = {"jsonrpc": "2.0", "id": id, "result": result}
        try:
            stdout.write(json.dumps(d, default=str) + "\n")
            stdout.flush()
        except Exception:  # noqa: BLE001
            pass

    def _send_error_to(
        self,
        stdout: IO[str],
        id: Any,
        code: int,
        message: str,
        data: Any = None,
    ) -> None:
        err: Dict[str, Any] = {"code": code, "message": message}
        if data is not None:
            err["data"] = data
        d = {"jsonrpc": "2.0", "id": id, "error": err}
        # error 响应必须有 id（即使是 null）
        if "id" not in d:
            d["id"] = id
        try:
            stdout.write(json.dumps(d, default=str) + "\n")
            stdout.flush()
        except Exception:  # noqa: BLE001
            pass
