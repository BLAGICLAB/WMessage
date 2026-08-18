---
name: minimax-ppt
description: F-6 端到端测试用 PPT Skill（auto mode 2-step DSL，覆盖 parse + 变量替换 + 状态机 + 工具执行 + 事件日志）
mode: auto
risk_level: low
max_steps: 5
timeout_secs: 60
rollback: none
intents: ["PPT", "F-6 测试"]
---

# F-6 测试 PPT Skill

## Step 1: 列出当前任务
list_tasks({})

## Step 2: 创建测试任务（演示变量替换）
create_task({"title":"F-6 端到端测试任务 ${step1.result}"})