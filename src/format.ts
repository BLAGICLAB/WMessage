// 主窗口 TodoCard 与挂件 TaskCardContent 共用的展示格式化（保持两处显示一致）

const pad = (n: number) => String(n).padStart(2, "0");

export function basename(p: string): string {
  const parts = p.split(/[\\/]/);
  return parts[parts.length - 1] || p;
}

// due 格式兼容两种："YYYY-MM-DD"（旧数据）与 "YYYY-MM-DDTHH:mm"
// 输出含年份：与完成时间 2026-09-08 老板拍板对齐（避免跨年任务识别不清）
export function formatDue(due: string): string {
  if (due.includes("T")) {
    const [d, t] = due.slice(0, 16).split("T");
    return `截止 ${d} ${t}`;
  }
  return `截止 ${due}`;
}

// 截止日期（"YYYY-MM-DD" 或 "YYYY-MM-DDTHH:mm"）的日期部分是否为今天（本地时区）
export function isDueToday(due?: string): boolean {
  if (!due) return false;
  const d = new Date();
  return (
    due.slice(0, 10) ===
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
  );
}

// completedAt(ms) → 「完成 YYYY-MM-DD HH:mm」；无效回空串
// （2026-09-08 老板拍板：加年份，避免跨年任务识别不出是哪一年的完成时间；
// 显示在截止日期下方；取消完成即清除）
export function formatCompletedAt(ms: number): string {
  const d = new Date(ms);
  if (isNaN(d.getTime())) return "";
  return `完成 ${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(
    d.getMinutes()
  )}`;
}

// datetime-local 值校验：格式完整 + 时分在界 + Date 往返一致
// （防 2026-02-31 被 Date 静默进位、防 25:99、防不完整输入产生坏数据——
// 沿用定时面板 NaN 事故教训：无效值一律不写库）
export function isValidDateTimeLocal(v: string): boolean {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(v)) return false;
  const h = Number(v.slice(11, 13));
  const m = Number(v.slice(14, 16));
  if (h > 23 || m > 59) return false;
  const d = new Date(v);
  if (isNaN(d.getTime())) return false;
  const pad2 = (n: number) => String(n).padStart(2, "0");
  return (
    `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}` === v.slice(0, 10)
  );
}

// 把已有 schedule 转成 datetime-local 输入框默认值（主窗口/挂件共用）
// 全程防御：任何分支算出无效日期都回退「今天 09:00」，防止 NaN 字符串扩散
// （历史事故：Invalid Date → NaN 写进 schedule → 面板回填死循环卡死 App）
export function scheduleToDatetime(s?: string | null): string {
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  const fallback = () =>
    `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}T09:00`;
  const fmt = (d: Date) => {
    if (isNaN(d.getTime())) return fallback();
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(
      d.getHours()
    )}:${pad(d.getMinutes())}`;
  };
  if (!s) return fallback();
  if (s.startsWith("at:")) {
    const t = s.slice(3, 19);
    return isNaN(new Date(t).getTime()) ? fallback() : t;
  }
  if (s.startsWith("daily:")) {
    const [h, m] = s.slice(6).split(":");
    return fmt(
      new Date(now.getFullYear(), now.getMonth(), now.getDate(), Number(h), Number(m))
    );
  }
  if (s.startsWith("weekly:")) {
    // weekly:D:HH:MM —— 三段解构（此前 t 只拿到小时，分钟 NaN 导致回退）
    const [d, h, m] = s.slice(7).split(":");
    const target = Number(d);
    if (!Number.isInteger(target) || target < 1 || target > 7) return fallback();
    const cur = ((now.getDay() + 6) % 7) + 1;
    const diff = (target + 7 - cur) % 7;
    return fmt(
      new Date(now.getFullYear(), now.getMonth(), now.getDate() + diff, Number(h), Number(m))
    );
  }
  if (s.startsWith("monthly:")) {
    // monthly:DD:HH:MM —— 三段解构
    const [dd, h, m] = s.slice(8).split(":");
    const day = Number(dd);
    if (!Number.isInteger(day) || day < 1 || day > 31) return fallback();
    let date = new Date(now.getFullYear(), now.getMonth(), day, Number(h), Number(m));
    if (date.getTime() < now.getTime()) {
      date = new Date(now.getFullYear(), now.getMonth() + 1, day, Number(h), Number(m));
    }
    // 下月无该日（如 2 月 31 日）→ 逐月顺延，上限 12 个月（防死循环）
    for (let i = 0; i < 12 && date.getDate() !== day; i++) {
      date = new Date(date.getFullYear(), date.getMonth() + 1, day, Number(h), Number(m));
    }
    return fmt(date);
  }
  return fallback();
}

// 定时执行规则 → 可读文案（主窗口/挂件共用）
export function formatSchedule(s: string): string {
  if (s.startsWith("daily:")) {
    const [h, m] = s.slice(6).split(":");
    return `每天 ${h}:${m}`;
  }
  if (s.startsWith("weekly:")) {
    // weekly:D:HH:MM —— 直接三段解构（此前两段拆分 t 只拿到小时，分钟丢成 undefined）
    const [d, h, m] = s.slice(7).split(":");
    const names = ["一", "二", "三", "四", "五", "六", "日"];
    const n = names[Number(d) - 1] ?? "?";
    return `每周${n} ${h}:${m}`;
  }
  if (s.startsWith("monthly:")) {
    // monthly:DD:HH:MM —— 三段解构
    const [dd, h, m] = s.slice(8).split(":");
    const day = Number(dd);
    // 坏数据（非整数或超出 1-31）原样显示，不再展开 NaN / 每月0日
    if (!Number.isInteger(day) || day < 1 || day > 31) return s;
    return `每月${day}日 ${h}:${m}`;
  }
  if (s.startsWith("at:")) {
    const t = s.slice(3, 19); // YYYY-MM-DDTHH:mm（此前 slice(3,16) 把分钟切没了）
    const [d, hm] = t.split("T");
    return `${d.slice(5)} ${hm} 一次`;
  }
  return s;
}
