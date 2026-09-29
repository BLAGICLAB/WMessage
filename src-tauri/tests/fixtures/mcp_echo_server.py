# MCP stdio 测试服务器（零依赖，手写 JSON-RPC 2.0 + 换行分帧）：
# 供 manager e2e 测试真实走完 spawn → initialize → tools/list → tools/call → shutdown。
# 不是产品代码；协议面刻意最小化，只应答 rmcp 客户端会发的四个方法。
import json
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


def main():
    # 阶段 6 e2e：stderr 管道捕获的确定性锚点（manager 环形缓冲应捕获本行）
    print("echo-test stderr ready", file=sys.stderr, flush=True)
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
                        "serverInfo": {"name": "echo-test", "version": "0.1.0"},
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
                            {"type": "text", "text": "echo: " + str(args.get("text", ""))}
                        ],
                        "isError": False,
                    },
                }
            )
        elif rid is not None:
            # 未知请求（ping 等）：协议级报错而不是静默，测试里能及时发现
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
