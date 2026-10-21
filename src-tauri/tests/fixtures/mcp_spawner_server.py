# MCP stdio 测试服务器（变体）：启动时拉一个**脱离进程组**的长驻子进程。
#
# 用途：断言「shutdown 之后 MCP 子进程与它那个逃逸子孙都没了」——
# rmcp 的 TokioChildProcess Drop 只是 fire-and-forget 一个 SIGKILL，
# 既不回收脱离进程组的子孙，那个 kill 在应用退出时还可能根本没来得及跑。
#
# 协议面与 mcp_echo_server.py 相同（手写 JSON-RPC 2.0 + 换行分帧）。
# 父子 pid 写进 MCP_SPAWNER_PIDFILE 指定的环境变量指向的文件。
import json
import os
import subprocess
import sys

TOOLS = [
    {
        "name": "echo",
        "description": "回显输入文本（e2e 测试用）",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string", "description": "要回显的文本"}},
            "required": ["text"],
        },
    }
]


def send(msg):
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def spawn_escaped_child():
    # setsid：脱离父进程的进程组 —— 组杀管不到它，只能靠 ppid 树回收
    child = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(300)"],
        preexec_fn=os.setsid,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return child.pid


def main():
    child_pid = spawn_escaped_child()
    pidfile = os.environ.get("MCP_SPAWNER_PIDFILE", "")
    if pidfile:
        with open(pidfile, "w") as f:
            f.write(f"{os.getpid()} {child_pid}")
    print("spawner-test stderr ready", file=sys.stderr, flush=True)
    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        req = json.loads(line)
        method = req.get("method")
        rid = req.get("id")
        if method == "initialize":
            send(
                {
                    "jsonrpc": "2.0",
                    "id": rid,
                    "result": {
                        "protocolVersion": req.get("params", {}).get(
                            "protocolVersion", "2025-03-26"
                        ),
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "spawner-test", "version": "0.1.0"},
                    },
                }
            )
        elif method == "tools/list":
            send({"jsonrpc": "2.0", "id": rid, "result": {"tools": TOOLS}})
        elif method == "tools/call":
            args = req.get("params", {}).get("arguments", {})
            send(
                {
                    "jsonrpc": "2.0",
                    "id": rid,
                    "result": {
                        "content": [
                            {"text": "echo: " + str(args.get("text", ""))}
                        ],
                        "isError": False,
                    },
                }
            )
        elif rid is not None:
            send(
                {
                    "jsonrpc": "2.0",
                    "id": rid,
                    "error": {"code": -32601, "message": f"method not found: {method}"},
                }
            )
        # notification（无 id，如 notifications/initialized）不回包


if __name__ == "__main__":
    main()
