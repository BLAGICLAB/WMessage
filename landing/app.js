/* ============================================================
   WMessage 落地页交互层
   两个内嵌窗口（首屏 / 演示区）共用同一套视图与状态；
   动效一律走 transform / opacity，触发即改、连续即换。
   ============================================================ */
(() => {
  "use strict";

  /* ── 工具 ─────────────────────────────────────────── */
  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));
  const el = (html) => {
    const t = document.createElement("template");
    t.innerHTML = html.trim();
    return t.content.firstElementChild;
  };
  const icon = (id) => `<svg><use href="#i-${id}" /></svg>`;
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  /* ── 导航 ─────────────────────────────────────────── */
  const NAV_MAIN = [
    { key: "home", label: "首页", icon: "home" },
    { key: "board", label: "任务", icon: "tasks" },
    { key: "graph", label: "图谱", icon: "graph" },
    { key: "archive", label: "归档", icon: "archive" },
    { key: "workspace", label: "工作区", icon: "folder" },
    { key: "trash", label: "回收站", icon: "trash" },
  ];
  const NAV_AGENT = [
    { key: "chat", label: "聊天", icon: "chat" },
    { key: "schedule", label: "定时", icon: "alarm" },
    { key: "apps", label: "程序", icon: "apps" },
    { key: "activity", label: "活动", icon: "activity" },
    { key: "notifications", label: "通知", icon: "bell", badge: 3 },
  ];

  /* ── 真实工具名（跑马灯用） ───────────────────────── */
  const TOOLS = [
    "query_tasks",
    "query_single_task",
    "create_task",
    "complete_task",
    "edit_task",
    "edit_file",
    "read_file",
    "grep_files",
    "run_python",
    "ast_inspect",
    "web_search",
    "fetch_url",
    "extract_document",
    "spawn_subagent",
    "check_subagent",
    "cancel_subagent",
    "apps_create",
    "apps_list",
    "list_archive",
    "tag_merge",
  ];

  /* ── 能力清单 ─────────────────────────────────────── */
  const SPECS = [
    {
      t: "周时间网格",
      d: "任务页就是一周：周一到周日 × 8:00–18:00。任务从右侧池子<b>拖进格子</b>排期，30 分钟吸附，块能拖、能拉长、拖回池子即取消排期。",
      tools: ["WeekGrid", "TaskPool"],
    },
    {
      t: "毛玻璃编辑浮窗",
      d: "<b>双击</b>任意计划块或池里的任务，全局浮窗从左侧滑入，改标题、备注、标签、子任务、截止都在这一处。关掉时可选「保存关闭」或「不保存关闭」。",
      tools: ["TaskEditorOverlay"],
    },
    {
      t: "内置机器人",
      d: "64 个主可见工具常驻：查任务、改文件、跑 Python、搜网、抓文档、理标签、发通知。聊天里的每一次调用都能展开看思考过程。",
      tools: ["query_tasks", "edit_file", "run_python", "web_search"],
    },
    {
      t: "受管子 agent",
      d: "主 agent 用 <b>spawn_subagent</b> 派发长任务，research / coder / general 三档，各有工具白名单；执行契约走 recon → executor → verifier 三段式。",
      tools: ["spawn_subagent", "check_subagent"],
    },
    {
      t: "定时任务",
      d: "一次 / 每天 / 每周 / 每月四档，到点自动交给机器人执行，结果前置「⏰ 自动执行」写回备注。错过触发有 2 小时补跑窗口。",
      tools: ["scheduled_jobs"],
    },
    {
      t: "程序（apps）",
      d: "零 LLM 的确定性小程序，画廊上架、试跑、版本快照回滚。AI 写完的代码先落<b>草稿</b>，你试跑满意了再上架。",
      tools: ["apps_create", "apps_update_files"],
    },
    {
      t: "图谱与记忆",
      d: "任务关系成图，母卡与子卡之间有线可追；语义长期记忆（记忆体 v2）跨会话记住你的偏好，不用反复交代。",
      tools: ["graphology", "memory"],
    },
    {
      t: "归档与回收站",
      d: "完成满 7 天自动归档并由模型打标，归档页可搜索、按标签筛。删除走软删除，回收站能原样恢复，绑的文件走系统废纸篓。",
      tools: ["list_archive", "restore_task"],
    },
  ];

  /* ── 演示区说明 ───────────────────────────────────── */
  const NOTES = {
    home: [
      {
        h: "首页是仪表盘，不是装饰",
        p: "问候语、三池、KPI 栏（8 选 4）、到期轮播、周柱图、通知中心。任务卡、图谱、日程三处联动——点哪儿都能回到同一件事。",
      },
    ],
    board: [
      {
        h: "一周就是一块布",
        p: "列是周一到周日，行是 8 点到 18 点。任务从右侧池子拖进格子即排期，块可以移动、底缘拉伸、拖回池子清除排期。",
      },
      {
        h: "双击开浮窗",
        p: "双击任意计划块，左侧滑入毛玻璃编辑浮窗。改完选「保存关闭」或「不保存关闭」——不保存时新建卡直接取消，已存在的卡回滚到打开时的状态。",
      },
    ],
    chat: [
      {
        h: "思考过程可以展开",
        p: "机器人每次调用工具都留痕，<code>思考过程</code> 折叠着，点开看它调了哪些工具、拿到了什么。",
      },
      {
        h: "执行契约三段式",
        p: "长任务交给子 agent：<code>recon</code> 只读侦察 → <code>executor</code> 动手执行 → <code>verifier</code> 独立复核。复核不过，带着具体拒收理由回去重试。",
      },
    ],
    graph: [
      {
        h: "任务之间本来就有关系",
        p: "母卡与子卡连边，标签与部门着色。悬停节点看它的邻居与度数——孤儿节点一眼可见。",
      },
    ],
    apps: [
      {
        h: "聊天即 IDE",
        p: "AI 写的程序先落到草稿区，<code>apps_update_files</code> 改文件、<code>apps_create</code> 建程序，试跑满意再上架上架货架。",
      },
      {
        h: "版本可回滚",
        p: "每次上架留版本快照，试跑记录、运行历史都在库里。出问题一键回滚到上一个能跑的版本。",
      },
    ],
  };

  /* ── 视图：首页 ───────────────────────────────────── */
  function viewHome() {
    const kpis = [
      ["本周完成", "12", ""],
      ["待办", "8", ""],
      ["逾期", "2", "is-due"],
      ["已归档", "5", ""],
    ];
    const due = [
      ["今天 18:00", "Windows 绿色版出包复验"],
      ["今天 21:00", "子 agent 审计结论整理"],
      ["明天 10:00", "图谱性能回归"],
    ];
    const pools = [
      ["待办", ["需求整理", "周报自动生成", "记忆体 v2 评测"]],
      ["今日", ["出包复验", "归档迁移"]],
      ["完成", ["看板拖拽原型", "标签近义合并"]],
    ];
    return `
      <div class="v-head">
        <div>
          <h3>下午好，张晓峰</h3>
          <p>今天有 2 个任务到期 · 本周已完成 12 个</p>
        </div>
        <div class="v-tabs"><span class="v-tab is-active">日</span><span class="v-tab">周</span><span class="v-tab">月</span></div>
      </div>
      <div class="kpi-row">
        ${kpis
          .map(
            ([k, v, cls]) => `<div class="kpi"><p>${k}</p><b class="${cls}">${v}</b></div>`
          )
          .join("")}
      </div>
      <div class="home-cols">
        <div class="panel">
          <div class="panel__head"><span>到期轮播</span><span>今天</span></div>
          ${due
            .map(
              ([t, n]) =>
                `<div class="due-item"><time>${t}</time><span>${n}</span></div>`
            )
            .join("")}
        </div>
        <div class="panel">
          <div class="panel__head"><span>本周完成</span><span>周一 → 周日</span></div>
          <div class="bars">
            ${[6, 5, 8, 7, 9, 11, 12]
              .map((h, i) => `<i data-h="${h}" class="${i === 6 ? "is-on" : ""}"></i>`)
              .join("")}
          </div>
        </div>
      </div>
      <div class="pool-row">
        ${pools
          .map(
            ([name, items]) => `<div class="pool"><h4>${name}</h4><ul>${items
              .map((n) => `<li><i></i>${n}</li>`)
              .join("")}</ul></div>`
          )
          .join("")}
      </div>`;
  }

  /* ── 视图：任务页（周时间网格） ───────────────────── */
  const DAYS = [
    ["周一", "11/02", true],
    ["周二", "11/03", false],
    ["周三", "11/04", false],
    ["周四", "11/05", false],
    ["周五", "11/06", false],
    ["周六", "11/07", false],
    ["周日", "11/08", false],
  ];
  const HOURS = ["08", "09", "10", "11", "12", "13", "14", "15", "16", "17", "18"];
  const PLANS = [
    { c: 1, top: 13.6, h: 13.6, t: "整理需求清单", s: "1.5 小时", tag: "blue", done: true, t1: "09:30", t2: "11:00" },
    { c: 1, top: 54.5, h: 13.6, t: "周报自动生成", s: "1.5 小时", tag: "violet", t1: "14:00", t2: "15:30" },
    { c: 2, top: 18.2, h: 18.2, t: "子 agent 审计", s: "2 小时", tag: "teal", t1: "10:00", t2: "12:00" },
    { c: 3, top: 9.1, h: 9.1, t: "Windows 出包复验", s: "1 小时", tag: "clay", t1: "09:00", t2: "10:00" },
    { c: 3, top: 63.6, h: 9.1, t: "归档迁移", s: "1 小时", tag: "rose", done: true, t1: "15:00", t2: "16:00" },
    { c: 4, top: 27.3, h: 13.6, t: "图谱性能回归", s: "1.5 小时", tag: "graphite", t1: "11:00", t2: "12:30" },
    { c: 0, top: 72.7, h: 9.1, t: "标签近义合并", s: "1 小时", tag: "blue", archived: true, t1: "16:00", t2: "17:00" },
    { c: 5, top: 9.1, h: 9.1, t: "记忆体 v2 评测", s: "1 小时", tag: "green", t1: "09:00", t2: "10:00" },
  ];
  const POOL = [
    ["需求整理", "需求", "blue"],
    ["周报自动生成", "自动化", "violet"],
    ["子 agent 审计", "审计", "teal"],
    ["Windows 出包复验", "发布", "clay"],
    ["图谱性能回归", "性能", "graphite"],
  ];

  function viewBoard() {
    // 计划块必须落在各自那一天的列里：列跨 11 个小时行，块绝对定位在列内
    const cols = DAYS.map(([, , today], i) => {
      const blocks = PLANS.filter((p) => p.c === i)
        .map(
          (p, k) =>
            `<div class="plan${p.done ? " plan--done" : ""}${p.archived ? " plan--archived" : ""}"
               data-block="${k}" data-time="${p.t1}" data-time2="${p.t2}" data-day="${DAYS[i][0]}"
               style="top:${p.top}%;height:${p.h}%;border-left-color:var(--tag-${p.tag})"
               title="双击编辑">
              <strong>${p.t}</strong><span>${p.s}</span>
            </div>`
        )
        .join("");
      return `<div class="week__col${today ? " is-today" : ""}" style="grid-column:${
        i + 2
      };grid-row:2 / span 11">${blocks}</div>`;
    }).join("");
    const dayHeads = DAYS.map(
      ([d, date, today], i) =>
        `<div class="week__day${today ? " is-today" : ""}" style="grid-column:${
          i + 2
        };grid-row:1"><span>${d}</span><b>${date}</b></div>`
    ).join("");
    const timeLabels = HOURS.map(
      (h, i) =>
        `<div class="week__time" style="grid-column:1;grid-row:${i + 2}">${h}:00</div>`
    ).join("");
    return `
      <div class="v-board">
        <div class="week">
          <div class="week__corner" style="grid-column:1;grid-row:1"></div>
          ${dayHeads}${timeLabels}${cols}
        </div>
        <aside class="pool-side">
          <div class="pool-side__head"><span>任务池</span><span>5 个未完成</span></div>
          ${POOL.map(
            ([t, tag, color]) =>
              `<div class="pool-card"><strong><span class="tag" style="background:var(--tag-${color})"></span>${t}</strong><span>${tag} · 拖到右侧格子排期</span></div>`
          ).join("")}
        </aside>
      </div>
      <div class="overlay" data-overlay>
        <div class="overlay__scrim" data-close-editor></div>
        <aside class="editor">
          <h4>编辑任务</h4>
          <div class="field"><label>标题</label><div data-edit-title>整理需求清单</div></div>
          <div class="field"><label>备注</label><div>归档前先确认标签已打</div></div>
          <div class="field"><label>计划时间</label><div data-edit-time>周二 09:30 – 11:00</div></div>
          <div class="field"><label>截止</label><div>2026-11-05</div></div>
          <div class="editor__row"><span>需求</span><span>本周</span><span>机器人</span></div>
          <div class="editor__foot">
            <button class="btn btn--primary" type="button" data-close-editor>保存关闭</button>
            <button class="btn btn--ghost" type="button" data-close-editor>不保存关闭</button>
          </div>
        </aside>
      </div>`;
  }

  /* ── 视图：聊天 ───────────────────────────────────── */
  const CHAT_SCRIPTS = {
    main: [
      { user: "帮我整理本周的任务进展，重点看归档情况", delay: 420 },
      {
        trace: {
          tools: [
            ["query_tasks", "✓"],
            ["query_single_task", "✓"],
            ["list_archive", "✓"],
          ],
        },
        delay: 760,
      },
      { bot: "本周共有 12 个任务完成，其中 5 个已归档。详细清单如下：", delay: 620 },
      {
        list: ["梳理需求清单 — 已完成并归档", "看板拖拽原型 — 已完成", "本周归档复盘 — 已归档"],
        delay: 520,
      },
      { bot: "需要我把归档任务导出成周报吗？", delay: 520 },
      { actions: ["复制", "移除", "整理 WMessage 需求清单"], delay: 480 },
    ],
    overdue: [
      { user: "哪些任务逾期了？", delay: 420 },
      { trace: { tools: [["query_tasks", "✓"]] }, delay: 700 },
      { bot: "有 2 个任务过了截止时间，其中 1 个今天 18:00 到期。", delay: 560 },
      { list: ["Windows 出包复验 — 逾期 1 天", "记忆体 v2 评测 — 今天 18:00"], delay: 520 },
      { actions: ["重排到本周", "标记完成"], delay: 460 },
    ],
    archive: [
      { user: "把本月完成的任务归档并打标签", delay: 420 },
      {
        trace: {
          tools: [
            ["query_tasks", "✓"],
            ["list_archive", "✓"],
            ["tag_merge", "✓"],
          ],
        },
        delay: 760,
      },
      { bot: "已归档 5 个任务，按语义聚成 3 个标签簇，主标签各留一个。", delay: 620 },
      { list: ["审计 · 4 个 → 审计", "发布 · 3 个 → 发布", "性能 · 2 个 → 性能"], delay: 520 },
      { actions: ["撤销本次合并", "导出 JSON"], delay: 460 },
    ],
  };

  function viewChat() {
    return `
      <div class="v-chat">
        <div class="chat__head">
          <b>默认会话</b>
          <span class="pill pill--brand">MiniMax-M3</span>
          <span class="pill">深度 标准</span>
          <span class="pill">权限 询问</span>
        </div>
        <div class="chat__body"></div>
        <div class="chat__quick">
          <button type="button" data-script="main">整理本周进展</button>
          <button type="button" data-script="overdue">查逾期任务</button>
          <button type="button" data-script="archive">归档本月完成</button>
        </div>
        <div class="chat__input">
          <span>描述任务，或输入 / 唤起命令…</span>
          <kbd>↵</kbd>
        </div>
      </div>`;
  }

  /* ── 视图：图谱 ───────────────────────────────────── */
  const GRAPH_NODES = [
    { id: "n1", x: 92, y: 66, label: "需求整理", done: true },
    { id: "n2", x: 208, y: 40, label: "看板原型", done: true },
    { id: "n3", x: 300, y: 96, label: "子 agent 审计" },
    { id: "n4", x: 178, y: 132, label: "周报生成" },
    { id: "n5", x: 66, y: 176, label: "标签合并", done: true },
    { id: "n6", x: 268, y: 196, label: "图谱性能" },
    { id: "n7", x: 372, y: 172, label: "记忆体 v2" },
    { id: "n8", x: 380, y: 58, label: "出包复验" },
    { id: "n9", x: 150, y: 240, label: "定时任务" },
    { id: "n10", x: 452, y: 128, label: "程序画廊" },
  ];
  const GRAPH_EDGES = [
    ["n1", "n2"],
    ["n1", "n4"],
    ["n2", "n3"],
    ["n2", "n8"],
    ["n3", "n7"],
    ["n4", "n6"],
    ["n4", "n9"],
    ["n5", "n9"],
    ["n6", "n7"],
    ["n6", "n10"],
    ["n7", "n10"],
    ["n3", "n6"],
  ];

  function viewGraph() {
    const pos = Object.fromEntries(GRAPH_NODES.map((n) => [n.id, n]));
    const edges = GRAPH_EDGES.map(([a, b], i) => {
      const A = pos[a];
      const B = pos[b];
      const len = Math.round(Math.hypot(B.x - A.x, B.y - A.y));
      return `<line class="gedge" data-edge="${i}" data-a="${a}" data-b="${b}" style="--len:${len}" x1="${A.x}" y1="${A.y}" x2="${B.x}" y2="${B.y}" />`;
    }).join("");
    const nodes = GRAPH_NODES.map(
      (n, i) =>
        `<g class="gnode${n.done ? " is-done" : ""}" data-node="${n.id}" style="transition-delay:${i * 45}ms">
           <circle cx="${n.x}" cy="${n.y}" r="13" />
           <text x="${n.x}" y="${n.y + 25}">${n.label}</text>
         </g>`
    ).join("");
    return `
      <div class="v-graph">
        <div class="graph__canvas">
          <svg viewBox="0 0 520 280" preserveAspectRatio="xMidYMid meet">
            ${edges}${nodes}
          </svg>
        </div>
        <aside class="graph__side">
          <div class="graph__card" data-graph-card>
            <h4>需求整理</h4>
            <p>4 个邻居 · 3 个子任务 · 归属：张晓峰</p>
          </div>
          <div class="graph__card">
            <h4>图例</h4>
            <p class="graph__legend">
              <span><i></i>进行中</span>
              <span><i class="is-done"></i>已完成</span>
              <span>连线 = 母卡 → 子卡</span>
            </p>
          </div>
        </aside>
      </div>`;
  }

  /* ── 视图：程序 ───────────────────────────────────── */
  function viewApps() {
    const apps = [
      ["周报生成", "读任务池 → 生成周报 Markdown", "v1.4", "运行 86 次"],
      ["CSV 合并", "多个 CSV 按列合并去重", "v2.1", "运行 42 次"],
      ["截图拼接", "把一组图拼成横向长图", "v1.0", "运行 17 次"],
    ];
    return `
      <div class="v-apps">
        <div class="apps__flow">
          <b>聊天即 IDE</b>
          <span>AI 写代码</span>→<span>落草稿</span>→<span>试跑</span>→<span>上架</span>→<span>版本快照可回滚</span>
        </div>
        <div class="apps__grid">
          ${apps
            .map(
              ([n, d, v, r]) =>
                `<div class="app-card"><h4>${n}</h4><p>${d}</p>
                   <div class="app-card__foot"><span>${v}</span><em>${r}</em></div>
                   <div class="app-card__foot"><span>试跑</span><span>上架</span></div>
                 </div>`
            )
            .join("")}
          <div class="app-card app-card--draft">
            <h4>归档清洗（草稿）</h4>
            <p>刚由聊天写入，待试跑</p>
            <div class="app-card__foot"><span>draft</span><em>刚刚</em></div>
            <div class="app-card__foot"><span>试跑</span><span>上架</span></div>
          </div>
        </div>
      </div>`;
  }

  /* ── 视图：紧凑列表（归档 / 工作区 / 回收站 / 定时 / 活动 / 通知） ── */
  const LISTS = {
    archive: {
      head: "归档",
      tabs: ["全部", "需求", "发布", "性能"],
      rows: [
        ["梳理需求清单", "需求 · 已归档", "11/01"],
        ["看板拖拽原型", "需求 · 已归档", "10/30"],
        ["Windows 绿色版出包", "发布 · 已归档", "10/29"],
        ["图谱万点压测", "性能 · 已归档", "10/27"],
      ],
    },
    workspace: {
      head: "工作区",
      tabs: ["全部", "链接", "文件夹"],
      rows: [
        ["SPEC.md", "文件 · 179 KB", "10/31"],
        ["docs/ 实机测试手册", "文件夹 · 12 个文件", "10/28"],
        ["门禁脚本速查", "链接 · scripts/", "10/24"],
        ["图标精灵", "链接 · lucide", "09/30"],
      ],
    },
    trash: {
      head: "回收站",
      tabs: ["全部"],
      rows: [
        ["旧版看板列数据", "软删除 · 2026-09-02", "09/02"],
        ["过期回归报告", "软删除 · 2026-09-05", "09/05"],
      ],
    },
    schedule: {
      head: "定时任务",
      tabs: ["全部", "每天", "每周"],
      rows: [
        ["每日进度汇总", "每天 20:30 · 交给机器人执行", "20:30"],
        ["周一计划草稿", "每周一 09:00", "09:00"],
        ["月末归档扫描", "每月 28 日 23:00", "28日"],
      ],
    },
    activity: {
      head: "活动",
      tabs: ["全部", "工具", "任务"],
      rows: [
        ["query_tasks", "工具 · 会话默认", "14:02"],
        ["edit_file → SPEC.md", "工具 · 会话默认", "13:58"],
        ["完成「出包复验」", "任务 · 机器人", "11:40"],
        ["spawn_subagent → 子 agent 审计", "工具 · 会话默认", "10:12"],
      ],
    },
    notifications: {
      head: "通知中心",
      tabs: ["全部", "任务提问", "产物绑定"],
      rows: [
        ["需要你确认这个标签？", "任务提问 · 归档页", "刚刚"],
        ["周报.md 已生成", "产物绑定 · 12 个任务", "12 分钟前"],
        ["子 agent 复核未通过，已重试", "记忆提案", "1 小时前"],
      ],
    },
    settings: {
      head: "设置",
      tabs: ["通用", "机器人", "模型", "桌面"],
      rows: [
        ["主题", "跟随系统 · 可手动切换", ""],
        ["机器人供应商", "MiniMax-M3（keyring 存储）", ""],
        ["MCP 服务器", "3 个已连接 · 按需启停", ""],
        ["快捷键", "唤起 Cmd+Ctrl+W · 新建 Cmd+Ctrl+N", ""],
      ],
    },
  };

  function viewList(key) {
    const cfg = LISTS[key];
    return `
      <div class="v-list">
        <div class="list-head">
          <div class="list-search">${icon("search")}<span>搜索</span></div>
          <div class="v-tabs">${cfg.tabs
            .map((t, i) => `<span class="v-tab${i === 0 ? " is-active" : ""}">${t}</span>`)
            .join("")}</div>
        </div>
        ${cfg.rows
          .map(
            ([a, b, t]) =>
              `<div class="list-row"><strong>${a}</strong><span>${b}</span><time>${t}</time></div>`
          )
          .join("")}
      </div>`;
  }

  /* ── 视图注册表 ───────────────────────────────────── */
  const VIEWS = {
    home: { render: viewHome },
    board: { render: viewBoard },
    chat: { render: viewChat },
    graph: { render: viewGraph },
    apps: { render: viewApps },
    archive: { render: () => viewList("archive") },
    workspace: { render: () => viewList("workspace") },
    trash: { render: () => viewList("trash") },
    schedule: { render: () => viewList("schedule") },
    activity: { render: () => viewList("activity") },
    notifications: { render: () => viewList("notifications") },
    settings: { render: () => viewList("settings") },
  };

  /* ── 窗口实例 ─────────────────────────────────────── */
  let demoWindow = null;

  function buildRail(root, onView) {
    const main = $("[data-rail='main']", root) || $("#railMain", root);
    const agent = $("[data-rail='agent']", root) || $("#railAgent", root);
    if (!main || !agent) return;
    const mk = (item) =>
      `<button class="rail__item" type="button" data-view="${item.key}">${icon(item.icon)}<span>${item.label}</span>${
        item.badge ? `<span class="rail__badge">${item.badge}</span>` : ""
      }</button>`;
    main.innerHTML = NAV_MAIN.map(mk).join("");
    agent.innerHTML = NAV_AGENT.map(mk).join("");
    $$(".rail__item", root).forEach((b) =>
      b.addEventListener("click", () => onView(b.dataset.view))
    );
  }

  class MockWindow {
    constructor(root) {
      this.root = root;
      this.stage = $(".app__stage", root);
      this.timers = [];
      buildRail(root, (key) => this.setView(key));
      this.bindActions();
      this.setView("home", true);
    }

    bindActions() {
      $$('[data-action="search"]', this.root).forEach((b) =>
        b.addEventListener("click", () => openPalette())
      );
      $$('[data-action="new-task"]', this.root).forEach((b) =>
        b.addEventListener("click", () => {
          this.setView("board");
          setTimeout(() => this.openEditor(), 260);
        })
      );
    }

    clearTimers() {
      this.timers.forEach(clearTimeout);
      this.timers = [];
    }

    setView(key, instant) {
      const cfg = VIEWS[key];
      if (!cfg) return;
      this.clearTimers();
      $$(".rail__item", this.root).forEach((b) =>
        b.classList.toggle("is-active", b.dataset.view === key)
      );
      const old = $(".view", this.stage);
      const flush = ["board", "chat", "graph", "apps"].includes(key);
      const node = el(
        `<div class="view${flush ? " view--flush" : ""}">${cfg.render()}</div>`
      );
      this.stage.appendChild(node);
      // 旧视图淡出移除，新视图淡入——纯 opacity/transform，无布局抖动
      if (old) {
        old.classList.remove("is-in");
        setTimeout(() => old.remove(), 220);
      }
      requestAnimationFrame(() =>
        requestAnimationFrame(() => node.classList.add("is-in"))
      );
      this.mount(key, node);
    }

    mount(key, node) {
      if (key === "board") this.mountBoard(node);
      else if (key === "chat") this.mountChat(node);
      else if (key === "graph") this.mountGraph(node);
      else this.mountList(node);
    }

    /* 周网格：双击计划块 → 毛玻璃浮窗从左滑入 */
    mountBoard(node) {
      const overlay = $("[data-overlay]", node);
      const close = () => {
        if (!overlay) return;
        overlay.classList.remove("is-open");
        setTimeout(() => overlay.remove(), 260);
      };
      $$(".plan", node).forEach((p) =>
        p.addEventListener("dblclick", () => {
          const title = $("[data-edit-title]", node);
          const time = $("[data-edit-time]", node);
          if (title) title.textContent = p.querySelector("strong").textContent;
          if (time) {
            time.textContent = `${p.dataset.day} ${p.dataset.time} – ${p.dataset.time2}`;
          }
          overlay.classList.add("is-open");
        })
      );
      $$("[data-close-editor]", node).forEach((b) => b.addEventListener("click", close));
      this.openEditor = () => overlay.classList.add("is-open");
    }

    /* 聊天：脚本化的一问一答，工具调用逐条落下来 */
    mountChat(node) {
      const body = $(".chat__body", node);
      const run = (name) => this.playChat(body, CHAT_SCRIPTS[name] || CHAT_SCRIPTS.main);
      $$("[data-script]", node).forEach((b) =>
        b.addEventListener("click", () => run(b.dataset.script))
      );
      run("main");
    }

    playChat(body, steps) {
      this.clearTimers();
      body.innerHTML = "";
      let i = 0;
      const reveal = (node, delay) => {
        this.timers.push(
          setTimeout(() => {
            node.classList.add("is-in");
            body.scrollTop = body.scrollHeight;
          }, delay || 0)
        );
      };
      const next = () => {
        if (i >= steps.length) return;
        const step = steps[i++];
        if (step.user) {
          const n = el(`<div class="bubble-user">${step.user}</div>`);
          body.appendChild(n);
          reveal(n, 10);
        } else if (step.trace) {
          const tools = step.trace.tools
            .map(
              ([name, mark]) =>
                `<span class="tool">${name}${mark ? `<b>${mark}</b>` : ""}</span>`
            )
            .join("");
          const n = el(`<div class="trace"><div class="trace__head">${icon(
            "chevron"
          )}<span>思考过程</span></div><div class="trace__tools">${tools}</div></div>`);
          body.appendChild(n);
          reveal(n, 10);
          this.timers.push(
            setTimeout(() => {
              n.classList.add("is-open");
              $$(".tool", n).forEach((t, k) =>
                this.timers.push(
                  setTimeout(() => t.classList.add("is-in"), k * 220)
                )
              );
            }, 160)
          );
        } else if (step.bot) {
          const n = el(`<div class="bot">${step.bot}</div>`);
          body.appendChild(n);
          reveal(n, 10);
        } else if (step.list) {
          const n = el(`<ul class="bot-list">${step.list
            .map((t) => `<li>${t}</li>`)
            .join("")}</ul>`);
          body.appendChild(n);
          reveal(n, 10);
        } else if (step.actions) {
          const n = el(`<div class="actions">${step.actions
            .map((a) => `<span>${a}</span>`)
            .join("")}</div>`);
          body.appendChild(n);
          $$("span", n).forEach((s, k) =>
            this.timers.push(
              setTimeout(() => {
                s.classList.add("is-in");
                body.scrollTop = body.scrollHeight;
              }, k * 120)
            )
          );
        }
        this.timers.push(setTimeout(next, step.delay || 500));
      };
      next();
    }

    /* 图谱：悬停高亮邻居 */
    mountGraph(node) {
      const card = $("[data-graph-card]", node);
      const nodes = $$(".gnode", node);
      const edges = $$(".gedge", node);
      const label = (id) => {
        const n = GRAPH_NODES.find((x) => x.id === id);
        return n ? n.label : id;
      };
      nodes.forEach((n) => {
        const id = n.dataset.node;
        const neigh = GRAPH_EDGES.filter(([a, b]) => a === id || b === id);
        n.addEventListener("mouseenter", () => {
          n.classList.add("is-hot");
          edges.forEach((e) => {
            if (e.dataset.a === id || e.dataset.b === id) e.classList.add("is-hot");
          });
          if (card) {
            card.innerHTML = `<h4>${label(id)}</h4><p>${neigh.length} 个邻居 · ${
              neigh.length ? neigh.map(([a, b]) => label(a === id ? b : a)).join("、") : "独立节点"
            }</p>`;
          }
        });
        n.addEventListener("mouseleave", () => {
          n.classList.remove("is-hot");
          edges.forEach((e) => e.classList.remove("is-hot"));
        });
      });
    }

    mountList(node) {
      const rows = $$(".list-row", node);
      rows.forEach((r, i) => {
        this.timers.push(setTimeout(() => r.classList.add("is-in"), 60 + i * 55));
      });
    }
  }

  /* ── 演示区标签 ───────────────────────────────────── */
  const DEMO_TABS = [
    { key: "home", label: "首页", icon: "home" },
    { key: "board", label: "任务", icon: "tasks" },
    { key: "chat", label: "聊天", icon: "chat" },
    { key: "graph", label: "图谱", icon: "graph" },
    { key: "apps", label: "程序", icon: "apps" },
  ];

  function buildDemoTabs() {
    const wrap = $("#demoTabs");
    wrap.innerHTML = DEMO_TABS.map(
      (t, i) =>
        `<button class="demo__tab${i === 1 ? " is-active" : ""}" type="button" data-tab="${t.key}">${icon(
          t.icon
        )}${t.label}</button>`
    ).join("");
    $$(".demo__tab", wrap).forEach((b) =>
      b.addEventListener("click", () => {
        $$(".demo__tab", wrap).forEach((x) => x.classList.remove("is-active"));
        b.classList.add("is-active");
        demoWindow.setView(b.dataset.tab);
        renderNotes(b.dataset.tab);
      })
    );
  }

  function renderNotes(key) {
    const box = $("#demoNotes");
    const notes = NOTES[key] || [];
    box.innerHTML = notes
      .map((n) => `<div class="note"><h3>${n.h}</h3><p>${n.p}</p></div>`)
      .join("");
  }

  /* ── 命令面板 ─────────────────────────────────────── */
  const COMMANDS = [
    { label: "打开任务页", key: "board", run: (w) => w.setView("board") },
    { label: "打开聊天", key: "chat", run: (w) => w.setView("chat") },
    { label: "打开图谱", key: "graph", run: (w) => w.setView("graph") },
    { label: "打开程序", key: "apps", run: (w) => w.setView("apps") },
    { label: "打开归档页", key: "archive", run: (w) => w.setView("archive") },
    { label: "新建任务", key: "⌘N", run: (w) => w.setView("board") },
    { label: "切换深浅色", key: "⌘T", run: () => toggleTheme() },
    { label: "回到顶部", key: "home", run: () => window.scrollTo({ top: 0, behavior: "smooth" }) },
  ];
  let paletteIndex = 0;

  function openPalette() {
    const p = $("#palette");
    p.hidden = false;
    requestAnimationFrame(() => {
      p.classList.add("is-open");
      const input = $("#paletteInput");
      input.value = "";
      input.focus();
      renderPalette("");
    });
  }

  function closePalette() {
    const p = $("#palette");
    p.classList.remove("is-open");
    setTimeout(() => {
      p.hidden = true;
    }, 200);
  }

  function renderPalette(q) {
    const list = $("#paletteList");
    const items = COMMANDS.filter((c) => c.label.includes(q));
    paletteIndex = Math.min(paletteIndex, Math.max(0, items.length - 1));
    list.innerHTML = items
      .map(
        (c, i) =>
          `<li class="palette__item${i === paletteIndex ? " is-active" : ""}" data-cmd="${c.label}">${c.label}<b>${c.key}</b></li>`
      )
      .join("");
    $$(".palette__item", list).forEach((li) =>
      li.addEventListener("click", () => {
        const cmd = COMMANDS.find((c) => c.label === li.dataset.cmd);
        closePalette();
        if (cmd) cmd.run(demoWindow);
      })
    );
  }

  function bindPalette() {
    const input = $("#paletteInput");
    input.addEventListener("input", () => {
      paletteIndex = 0;
      renderPalette(input.value);
    });
    input.addEventListener("keydown", (e) => {
      const items = $$(".palette__item");
      if (e.key === "ArrowDown") {
        e.preventDefault();
        paletteIndex = (paletteIndex + 1) % Math.max(1, items.length);
        renderPalette(input.value);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        paletteIndex = (paletteIndex - 1 + items.length) % Math.max(1, items.length);
        renderPalette(input.value);
      } else if (e.key === "Enter") {
        e.preventDefault();
        const target = items[paletteIndex];
        if (target) target.click();
      } else if (e.key === "Escape") {
        closePalette();
      }
    });
    $$("[data-close-palette]").forEach((b) => b.addEventListener("click", closePalette));
  }

  /* ── 主题 ─────────────────────────────────────────── */
  function applyTheme(theme) {
    document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem("wm-theme", theme);
    } catch (err) {
      /* 隐私模式下写不进去也不该挂 */
    }
  }

  function toggleTheme() {
    applyTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark");
  }

  /* ── 计数器 ───────────────────────────────────────── */
  function animateCount(node) {
    const target = Number(node.dataset.count);
    if (reduced) {
      node.textContent = String(target);
      return;
    }
    const dur = 900;
    const t0 = performance.now();
    const tick = (t) => {
      const k = Math.min(1, (t - t0) / dur);
      const eased = 1 - Math.pow(1 - k, 3);
      node.textContent = String(Math.round(target * eased));
      if (k < 1) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }

  /* ── 进场编排 ─────────────────────────────────────── */
  function setupReveals() {
    const io = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (!entry.isIntersecting) return;
          entry.target.classList.add("is-in");
          io.unobserve(entry.target);
        });
      },
      { rootMargin: "0px 0px -12% 0px", threshold: 0.15 }
    );

    $$(".spec-row").forEach((row, i) => {
      row.style.transitionDelay = `${i * 55}ms`;
      io.observe(row);
    });
    const pipeline = $("#pipeline");
    if (pipeline) io.observe(pipeline);
    $$(".pipe-stage").forEach((s, i) => {
      s.style.transitionDelay = `${140 + i * 110}ms`;
      io.observe(s);
    });
    $$(".guard").forEach((g, i) => {
      g.style.transitionDelay = `${i * 70}ms`;
      io.observe(g);
    });
    $$("[data-count]").forEach((n) => {
      io.observe(n);
      const mo = new MutationObserver(() => {
        if (n.classList.contains("is-in")) {
          animateCount(n);
          mo.disconnect();
        }
      });
      mo.observe(n, { attributes: true, attributeFilter: ["class"] });
    });
  }

  /* ── 指针聚光（细指针设备）+ 首屏窗口轻倾斜 ────────── */
  function setupPointer() {
    if (reduced) return;
    if (!window.matchMedia("(hover: hover) and (pointer: fine)").matches) return;

    $$(".spec-row").forEach((row) => {
      const spot = el('<span class="spot"></span>');
      row.appendChild(spot);
      row.addEventListener("pointermove", (e) => {
        const r = row.getBoundingClientRect();
        spot.style.transform = `translate3d(${e.clientX - r.left}px, ${
          e.clientY - r.top
        }px, 0)`;
      });
    });

    const stage = $("#heroStage");
    const app = stage ? $(".app", stage) : null;
    if (!stage || !app) return;
    let raf = 0;
    let tx = 0;
    let ty = 0;
    const apply = () => {
      raf = 0;
      app.style.transform = `rotateX(${ty}deg) rotateY(${tx}deg)`;
    };
    stage.addEventListener(
      "pointermove",
      (e) => {
        const r = stage.getBoundingClientRect();
        tx = ((e.clientX - r.left) / r.width - 0.5) * 5;
        ty = -((e.clientY - r.top) / r.height - 0.5) * 3.5;
        if (!raf) raf = requestAnimationFrame(apply);
      },
      { passive: true }
    );
    stage.addEventListener("pointerleave", () => {
      tx = 0;
      ty = 0;
      if (!raf) raf = requestAnimationFrame(apply);
    });
  }

  /* ── 滚动进度 ─────────────────────────────────────── */
  function setupProgress() {
    const bar = $("#progressBar");
    let ticking = false;
    const update = () => {
      ticking = false;
      const h = document.documentElement.scrollHeight - window.innerHeight;
      const k = h > 0 ? window.scrollY / h : 0;
      bar.style.transform = `scaleX(${Math.min(1, Math.max(0, k))})`;
    };
    window.addEventListener(
      "scroll",
      () => {
        if (!ticking) {
          ticking = true;
          requestAnimationFrame(update);
        }
      },
      { passive: true }
    );
  }

  /* ── 跑马灯 ───────────────────────────────────────── */
  function setupMarquee() {
    const track = $(".marquee__track");
    if (!track) return;
    const items = TOOLS.map((t) => `<span>${t}</span>`).join("");
    track.innerHTML = items + items;
  }

  /* ── 能力清单 ─────────────────────────────────────── */
  function buildSpecs() {
    const list = $("#specList");
    list.innerHTML = SPECS.map(
      (s) => `<div class="spec-row">
        <dt><i></i>${s.t}</dt>
        <dd>${s.d}</dd>
        <div class="spec-row__tools">${s.tools
          .map((t) => `<span>${t}</span>`)
          .join("")}</div>
      </div>`
    ).join("");
  }

  /* ── 启动 ─────────────────────────────────────────── */
  function init() {
    try {
      const saved = localStorage.getItem("wm-theme");
      if (saved) applyTheme(saved);
      else if (window.matchMedia("(prefers-color-scheme: dark)").matches)
        applyTheme("dark");
    } catch (err) {
      /* 读不到就用默认浅色 */
    }

    setupMarquee();
    buildSpecs();
    buildDemoTabs();

    const hero = new MockWindow($("#app"));
    demoWindow = new MockWindow($("#app2"));
    demoWindow.setView("board");
    renderNotes("board");

    $$("[data-goto-view]").forEach((b) =>
      b.addEventListener("click", () => {
        hero.setView(b.dataset.gotoView);
        $("#demo").scrollIntoView({ behavior: reduced ? "auto" : "smooth", block: "start" });
      })
    );

    $("#themeToggle").addEventListener("click", toggleTheme);
    $("#openPalette").addEventListener("click", openPalette);
    bindPalette();

    setupReveals();
    setupPointer();
    setupProgress();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
