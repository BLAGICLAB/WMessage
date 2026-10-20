import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

// main.css 静态结构锁（vitest 配置 css:false 会把 .css 导入吞成空串，直接读文件）。
//
// 1) 滚动条主题适配：color-scheme（原生控件/滚动条随主题）+ webkit 细滚动条走
//    主题变量。原先全局没有滚动条样式，深色模式下滚动条仍是浅色原生样式——锁死防回退。
// 2) nm-card-hover 过渡对称：hover 块改了哪些属性，基类 transition 必须同样声明，
//    否则鼠标进入平滑、离开瞬间弹回。 换扁平材质后 hover 信号 = 背景抬升 +
//    边框提亮，不再有 transform/shadow 跳变；基类 transition 同时覆盖进出场。
// 3) 动效 token 层：曲线与时长的唯一来源。库里每条 transition 都得引用 token——
//    手写一次就多一个分叉，同一个「hover 一个控件」迟早有快有慢。
const css = readFileSync("src/ui/main.css", "utf-8");

/** src/ 下的业务 tsx（测试文件除外）：动效类串的扫描范围 */
function tsxFiles(): string[] {
  return readdirSync("src", { recursive: true })
    .map(String)
    .filter((f) => f.endsWith(".tsx") && !f.includes(".test."));
}

/** 去掉注释：注释里提到某个类名不算真的用了它 */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");
}

describe("滚动条主题适配（main.css）", () => {
  it("color-scheme 随深浅主题切换", () => {
    // 浅色根 + .dark 覆盖，二者缺一 WebView2 原生滚动条就不随主题
    expect(css).toMatch(/:root\s*\{[^}]*color-scheme:\s*light/s);
    expect(css).toMatch(/\.dark\s*\{[^}]*color-scheme:\s*dark/s);
  });

  it("webkit 细滚动条走主题变量（不硬编码颜色）", () => {
    expect(css).toContain("::-webkit-scrollbar-thumb");
    expect(css).toMatch(
      /::-webkit-scrollbar-thumb\s*\{[^}]*background:\s*var\(--t6\)/s,
    );
    // 轨道透明（不遮挡新拟态背景层次）
    expect(css).toMatch(
      /::-webkit-scrollbar-track\s*\{[^}]*background:\s*transparent/s,
    );
  });
});

describe("main.css nm-card-hover 过渡对称", () => {
  it("基类 transition 声明 hover 变化的属性（background/border），hover 块不引入 transform", () => {
    // 基类 .nm-card, .nm-card-hover 块：transition 覆盖 hover 全部变化属性
    expect(css).toMatch(
      /transition:\s*background-color var\(--dur-card\) var\(--ease-out\),\s*border-color var\(--dur-card\) var\(--ease-out\),\s*opacity var\(--dur-card\) var\(--ease-out\);[\s\S]*?\.nm-card-hover:hover/,
    );
    // hover 块只改背景与边框（扁平悬停），不得引入 transform/shadow 跳变
    const hoverBlock = css.match(/\.nm-card-hover:hover\s*\{([^}]*?)\}/);
    expect(hoverBlock).not.toBeNull();
    const body = hoverBlock?.[1] ?? "";
    expect(body).toContain("background-color");
    expect(body).toContain("border-color");
    expect(body).not.toContain("transform");
    expect(body).not.toContain("box-shadow");
    expect(body).not.toContain("transition");
  });
});

describe("main.css 主滚动容器 app-scroll", () => {
  it("关掉滚动锚定：吸顶头部高度每帧变化会与锚定补偿形成反馈环（头部抖动）", () => {
    // 浏览器默认给滚动容器开滚动锚定：视口外内容尺寸一变就改 scrollTop 补偿。
    // 吸顶头部的高度由滚动进度驱动、每帧都在变，而进度又来自 scrollY——
    // 两者相扣成环。关掉锚定，滚动语义由应用自己管。
    expect(css).toMatch(/\.app-scroll\s*\{[^}]*overflow-anchor:\s*none/);
  });
});

describe("动效 token 层（main.css）", () => {
  it("定义了动效曲线与时长 token", () => {
    expect(css).toMatch(/--ease-out:\s*cubic-bezier\(0\.23,\s*1,\s*0\.32,\s*1\)/);
    expect(css).toMatch(/--ease-pulse:\s*cubic-bezier\(0,\s*0,\s*0\.58,\s*1\)/);
    expect(css).toMatch(/--dur-chip:\s*100ms/);
    expect(css).toMatch(/--dur-card:\s*150ms/);
    expect(css).toMatch(/--dur-fade:\s*150ms/);
    expect(css).toMatch(/--dur-tooltip:\s*150ms/);
  });

  it("main.css 的 transition 声明不再有手写时长/曲线字面量", () => {
    // 减少动效兜底那行 transition-duration: 0.01ms 不在此列——它不是过渡，是全局掐断
    expect(css).not.toMatch(/transition:[^;]*\b\d+m?s\b/);
  });

  it("tsx 里的 transition-* 工具类都带 token 时长与曲线", () => {
    // 扫字符串字面量而不是 className 属性：Toggle / App 的类串是数组 join，
    // 类名活在变量里，按属性扫会漏掉那几处。
    const TRANSITION = /transition-(?:colors|opacity|transform|\[[^\]]+\])/;
    const offenders: string[] = [];
    for (const f of tsxFiles()) {
      const src = readFileSync(join("src", f), "utf-8");
      for (const m of src.matchAll(/"([^"]*)"|`([^`]*)`/g)) {
        const lit = m[1] ?? m[2] ?? "";
        if (!TRANSITION.test(lit)) continue;
        if (
          !/duration-\[var\(--dur-/.test(lit) ||
          !/ease-\[var\(--ease-/.test(lit)
        ) {
          offenders.push(`${f}: ${lit.trim().slice(0, 80)}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  it("没有死 token：每个动效 token 都得有人用", () => {
    // 定了不用的 token 就是下一个分叉源：先定义、后没人引用，久了就成了
    // 「这值到底是多少也没人知道」的遗产。定义与使用同一批落地。
    const defined = [...css.matchAll(/^\s*(--(?:ease|dur)-[a-z-]+):/gm)].map(
      (m) => m[1],
    );
    expect(defined.length).toBeGreaterThan(0);
    const sources = [
      css,
      ...tsxFiles().map((f) => readFileSync(join("src", f), "utf-8")),
    ].join("\n");
    const unused = defined.filter((t) => !sources.includes(`var(${t})`));
    expect(unused).toEqual([]);
  });
});


describe("main.css 计划块落位弹入", () => {
  it("落位弹入无过冲（遵守「无弹跳」纪律）", () => {
    const block = css.match(/\.plan-fresh\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(block).toMatch(/animation:\s*plan-pop var\(--dur-pop\) var\(--ease-out\)/);
    // 起点缩放比落在 0.9–0.97
    const kf = css.match(/@keyframes plan-pop\s*\{[\s\S]*?scale\(([\d.]+)\)/)?.[1];
    expect(parseFloat(kf ?? "0")).toBeGreaterThanOrEqual(0.9);
    expect(parseFloat(kf ?? "0")).toBeLessThanOrEqual(0.97);
    // 旧曲线（y2 = 1.4，过冲）不得残留
    expect(css).not.toContain("cubic-bezier(0.2, 1.4, 0.4, 1)");
  });
});

describe("main.css 拖拽的拾起语汇", () => {
  it("池内拖起有 transform 过渡，网格块有 opacity 过渡腿", () => {
    const poolLift = css.match(/\.pool-lift\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(poolLift).toMatch(/transition:[^;]*transform/);
    // box-shadow 不进过渡列表：阴影是逐帧重绘，拖起是「瞬间抬起」的语义
    expect(poolLift).not.toMatch(/transition:[^;]*box-shadow/);
    const planBlk = css.match(/\.plan-blk\s*\{([\s\S]*?)\}/)?.[1] ?? "";
    expect(planBlk).toMatch(/transition:[\s\S]*?opacity/);
  });
});

/** 扫出「不在 hover 门控媒体查询里」的 .plan-blk hover 规则 */
function barePlanBlkHover(css: string): string[] {
  const ranges: Array<[number, number]> = [];
  const gate = /@media \(hover: hover\) and \(pointer: fine\)/g;
  let m: RegExpExecArray | null;
  while ((m = gate.exec(css))) {
    let depth = 0;
    let i = css.indexOf("{", m.index);
    for (; i < css.length; i++) {
      if (css[i] === "{") depth++;
      else if (css[i] === "}") {
        depth--;
        if (depth === 0) break;
      }
    }
    ranges.push([m.index, i]);
    gate.lastIndex = i;
  }
  const out: string[] = [];
  for (const rule of css.matchAll(
    /^[ \t]*([^\n{}]*\.plan-blk[^{}]*:hover[^{}]*)\{/gm,
  )) {
    const gated = ranges.some(([a, b]) => rule.index >= a && rule.index <= b);
    if (!gated) out.push(rule[1].trim());
  }
  return out;
}

describe("main.css 计划块 hover 的触屏门控", () => {
  it("计划块 hover 规则有触屏门控", () => {
    // 触屏点一下就保持抬起、手感像卡住——hover 抬升只在真鼠标下生效
    expect(barePlanBlkHover(css)).toEqual([]);
    expect(css).toMatch(/@media \(hover: hover\) and \(pointer: fine\)/);
  });

  it("计划块过渡列表不含 box-shadow（阴影是状态不是运动），时长归 chip 档", () => {
    const block = css.match(/\.plan-blk\s*\{([\s\S]*?)\}/)?.[1] ?? "";
    expect(block).not.toMatch(/transition:[\s\S]*?box-shadow/);
    expect(block).toMatch(/transition:[\s\S]*?var\(--dur-chip\)/);
  });
});

describe("main.css nm-card 的 opacity 过渡腿", () => {
  it("nm-card 的过渡覆盖 opacity（整卡降透明度要补间）", () => {
    const block = css.match(/\.nm-card,\s*\.nm-card-hover\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(block).toMatch(/transition:[^;]*opacity/);
  });

  it("标签合并弹窗的簇行过渡覆盖 opacity/边框/背景（勾选与未勾选之间要有补间）", () => {
    const src = readFileSync("src/components/TagConsolidateModal.tsx", "utf-8");
    expect(src).toMatch(
      /transition-\[opacity,border-color,background-color\] duration-\[var\(--dur-card\)\] ease-\[var\(--ease-out\)\]/,
    );
  });
});

describe("按压反馈", () => {
  const PRESS_FILES = [
    "src/components/Toggle/Toggle.tsx",
    "src/components/ChatPanel/InputArea.tsx",
  ];

  it("Toggle 与聊天输入栏按钮有按压反馈", () => {
    for (const f of PRESS_FILES) {
      expect(readFileSync(f, "utf-8")).toContain("active:scale-[0.97]");
    }
    const input = readFileSync("src/components/ChatPanel/InputArea.tsx", "utf-8");
    // 发送键的过渡必须覆盖 transform：transition-opacity 只过渡 opacity，
    // 按压时的缩放会瞬切
    expect(input).toMatch(/transition-\[opacity,transform\]/);
  });

  it("按压缩放的元素，其过渡必须覆盖 transform（否则缩放是瞬切）", () => {
    const offenders: string[] = [];
    for (const f of PRESS_FILES) {
      const src = readFileSync(f, "utf-8");
      for (const m of src.matchAll(/"([^"]*)"|`([^`]*)`/g)) {
        const lit = m[1] ?? m[2] ?? "";
        if (!lit.includes("active:scale-[0.97]")) continue;
        if (!/transition-(transform|\[[^\]]*transform[^\]]*\])/.test(lit)) {
          offenders.push(`${f}: ${lit.trim().slice(0, 90)}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });
});

describe("拖拽幽灵的定位方式", () => {
  const TIMELINE = "src/components/TaskTimeline/TaskTimelinePage.tsx";

  it("幽灵用 transform 定位，不写 left/top（布局属性逐帧写会每帧重排）", () => {
    const src = readFileSync(TIMELINE, "utf-8");
    const ghostBlock =
      src.match(/className="drag-ghost"[\s\S]*?style=\{([\s\S]*?)\}/)?.[1] ?? "";
    expect(ghostBlock).toContain("translate3d");
    expect(ghostBlock).not.toContain("left: ghost.x");
    expect(ghostBlock).not.toContain("top: ghost.y");
  });

  it("幽灵的定位原点固定在左上角（位移只由 transform 给）", () => {
    const block = css.match(/\.drag-ghost\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(block).toMatch(/left:\s*0/);
    expect(block).toMatch(/top:\s*0/);
  });

  it("幽灵尺寸只量一次（逐帧读 offsetWidth 会强制同步布局）", () => {
    const src = readFileSync(TIMELINE, "utf-8");
    expect(src).toContain("ghostSizeRef");
    // 剥掉注释再数：注释里提到 offsetWidth 不算一次读取
    const code = stripComments(src);
    // 读取必须落在「缓存为空且元素在」的守卫里，且读完写回缓存
    expect(code).toMatch(
      /if \(![A-Za-z_$][\w$]* && ghostRef\.current\) \{[\s\S]*?offsetWidth[\s\S]*?offsetHeight[\s\S]*?ghostSizeRef\.current = [A-Za-z_$][\w$]*;/,
    );
    // 不允许每帧裸读
    expect(code).not.toMatch(/ghostRef\.current\?\.offset(Width|Height)/);
  });
});

describe("减少动效下的活动信号", () => {
  it("活动指示器在减少动效时换成透明度呼吸（信号不能消失）", () => {
    expect(css).toMatch(/\.wmsg-activity\s*\{[^}]*animation:\s*wmsg-spin/);
    expect(css).toMatch(/\.wmsg-activity-pulse\s*\{[^}]*animation:\s*wmsg-pulse/);
    // 兜底块用 !important 掐断一切动画，这里的覆盖也必须带 !important 才压得住：
    // 普通优先级会被兜底块压成 0.01ms / 1 次，动画等于冻结（Chromium 实测）
    expect(css).toMatch(
      /@media \(prefers-reduced-motion: reduce\)[\s\S]*?\.wmsg-activity,\s*\.wmsg-activity-pulse\s*\{[^}]*animation:[^;]*!important/,
    );
    expect(css).toMatch(
      /@media \(prefers-reduced-motion: reduce\)[\s\S]*?\.wmsg-activity,\s*\.wmsg-activity-pulse\s*\{[^}]*wmsg-breathe/,
    );
  });

  it("呼吸态只动 opacity，不动 transform（减少动效去掉位移）", () => {
    const breathe = css.match(/@keyframes wmsg-breathe\s*\{([\s\S]*?)\n {2}\}/)?.[1] ?? "";
    expect(breathe).toContain("opacity");
    expect(breathe).not.toContain("transform");
  });

  it("活动指示器不再用 Tailwind 的 animate-spin（转子冻结成静止图标）", () => {
    const offenders = tsxFiles().filter((f) =>
      stripComments(readFileSync(join("src", f), "utf-8")).includes("animate-spin"),
    );
    expect(offenders).toEqual([]);
  });

  it("animate-pulse 只剩必应图骨架屏（骨架本就该静态，不是活动信号）", () => {
    const users = tsxFiles().filter((f) =>
      stripComments(readFileSync(join("src", f), "utf-8")).includes("animate-pulse"),
    );
    expect(users).toEqual(["components/HomePage/BingCard.tsx"]);
  });
});

describe("main.css 共享弹层入场", () => {
  it("遮罩淡入、面板从中心缩放入场，缩放起点在 0.9–0.97", () => {
    const backdrop = css.match(/\.modal-backdrop\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(backdrop).toMatch(/animation:\s*modal-fade var\(--dur-pop\) var\(--ease-out\)/);
    const panel = css.match(/\.modal-panel\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(panel).toMatch(/animation:\s*modal-pop var\(--dur-pop\) var\(--ease-out\)/);
    // 弹窗是居中出现的，从中心长是对的（浮层才需要从触发点长）
    expect(panel).toMatch(/transform-origin:\s*center/);
    const kf = css.match(/@keyframes modal-pop\s*\{[\s\S]*?scale\(([\d.]+)\)/)?.[1];
    expect(parseFloat(kf ?? "0")).toBeGreaterThanOrEqual(0.9);
    expect(parseFloat(kf ?? "0")).toBeLessThanOrEqual(0.97);
  });

  it("入场动效只在共享壳里实现，调用点不各写各的", () => {
    const shell = readFileSync("src/ui/Modal.tsx", "utf-8");
    expect(shell).toContain("modal-backdrop");
    expect(shell).toContain("modal-panel");
    const offenders = tsxFiles()
      .filter((f) => f !== "ui/Modal.tsx")
      .filter((f) => /modal-pop|modal-fade/.test(readFileSync(join("src", f), "utf-8")));
    expect(offenders).toEqual([]);
  });
});

describe("main.css 浮层入场", () => {
  it("浮层有入场动效且缩放起点在 0.9–0.97", () => {
    const popover = css.match(/\.nm-popover\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(popover).toMatch(/animation:\s*popover-in var\(--dur-pop\) var\(--ease-out\)/);
    const kf = css.match(/@keyframes popover-in\s*\{[\s\S]*?scale\(([\d.]+)\)/)?.[1];
    expect(parseFloat(kf ?? "0")).toBeGreaterThanOrEqual(0.9);
    expect(parseFloat(kf ?? "0")).toBeLessThanOrEqual(0.97);
  });

  it("四个浮层调用点各自指定了变换原点（从触发点长出来）", () => {
    for (const [f, origin] of [
      ["src/components/SettingsPage/ApiProviderSelect.tsx", "origin-top"],
      ["src/components/SettingsPage/AvatarPicker.tsx", "origin-top-left"],
      ["src/components/SettingsPage/SettingsPage.tsx", "origin-top-right"],
    ] as const) {
      const src = readFileSync(f, "utf-8");
      expect(src).toContain(origin);
    }
    // SettingsPage 有两个浮层调用点，右对齐原点要两处都在
    const settings = readFileSync("src/components/SettingsPage/SettingsPage.tsx", "utf-8");
    expect(settings.match(/nm-popover[^"]*origin-top-right/g)?.length).toBe(2);
  });
});
