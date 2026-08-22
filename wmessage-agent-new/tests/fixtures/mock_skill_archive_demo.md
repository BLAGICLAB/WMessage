---
name = "mock-skill-archive-demo"
description = "演示任务归档：列任务 + 查详情"
mode = "auto"
risk = "low"
intents = ["任务归档", "归档演示"]
react_max_steps = 15
timeout_s = 60
---

# 任务归档演示 Skill（Phase 1 mock）

调用 list_tasks 拿所有任务，然后对第一个调 query_single_task 查详情，输出汇总。

## Step 1: 列出所有任务
list_tasks({})

## Step 2: 查询第一张卡详情
query_single_task({"id": "${step1.id}"})
