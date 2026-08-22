"""IPC package — JSON-RPC 2.0 over stdin/stdout (主通道) + 本地 HTTP (备选)。

参见 wmessage-agent-spec.md §Y IPC 协议细节。
"""
from ipc.server import IPCServer, IPCError

__all__ = ["IPCServer", "IPCError"]
