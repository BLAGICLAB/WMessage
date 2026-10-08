// SettingsPage 子模块：厂商 → lobehub 图标解析（vendor 模式，资产见 src/assets/providers/lobe/，
// 由 scripts/sync-lobe-icons.mjs 从 @lobehub/icons-static-svg 同步）。
// 解析链：别名归一（models.dev provider_key / 厂商显示名 → lobehub slug）→ -color 优先 → 单色次之 →
// null（自定义/长尾厂商由 ProviderLogo 回退色块+首字母）。

const modules = import.meta.glob("../../assets/providers/lobe/*.svg", {
  eager: true,
  query: "?url",
  import: "default",
}) as Record<string, string>;

/** slug（不含 -color 后缀）→ 资产 url 表 */
const LOBE_ICONS: Record<string, string> = {};
for (const [path, url] of Object.entries(modules)) {
  LOBE_ICONS[path.split("/").pop()!.replace(/\.svg$/, "")] = url;
}

/** models.dev provider_key / 常见厂商显示名（小写）→ lobehub slug。 *  仅登记命名不一致的项；key 本身即 slug 的靠直接命中，不入表。 */
const PROVIDER_ALIASES: Record<string, string> = {
  // ── models.dev provider_key 差异 ──
  moonshotai: "kimi",
  "moonshotai-cn": "kimi",
  "kimi-code-plan-cn": "kimi",
  "kimi-code-plan-global": "kimi",
  zhipuai: "zhipu",
  "zhipuai-coding-plan": "zhipu",
  alibaba: "bailian",
  "alibaba-cn": "bailian",
  "alibaba-coding-plan": "bailian",
  "alibaba-coding-plan-cn": "bailian",
  "alibaba-token-plan": "bailian",
  "alibaba-token-plan-cn": "bailian",
  "amazon-bedrock": "bedrock",
  "azure-cognitive-services": "azureai",
  "google-vertex": "vertexai",
  "google-vertex-anthropic": "vertexai",
  "cloudflare-workers-ai": "workersai",
  "cloudflare-ai-gateway": "cloudflare",
  siliconflow: "siliconcloud",
  "siliconflow-cn": "siliconcloud",
  "fireworks-ai": "fireworks",
  togetherai: "together",
  "novita-ai": "novita",
  "302ai": "ai302",
  "stepfun-ai": "stepfun",
  "stepfun-step-plan": "stepfun",
  "stepfun-ai-step-plan": "stepfun",
  "minimax-cn": "minimax",
  "minimax-coding-plan": "minimax",
  "minimax-cn-coding-plan": "minimax",
  "zai-coding-plan": "zai",
  "github-copilot": "githubcopilot",
  "volcengine-coding-plan": "volcengine",
  "tencent-tokenhub": "hunyuan",
  "tencent-token-plan": "hunyuan",
  "tencent-coding-plan": "hunyuan",
  tencent: "hunyuan",
  xiaomi: "xiaomimimo",
  "xiaomi-token-plan-cn": "xiaomimimo",
  "xiaomi-token-plan-ams": "xiaomimimo",
  "xiaomi-token-plan-sgp": "xiaomimimo",
  "qiniu-ai": "qiniu",
  "ollama-cloud": "ollama",
  "io-net": "ionet",
  llama: "metaai",
  "snowflake-cortex": "snowflake",
  databricks: "dbrx",
  watsonx: "ibm",
  kilo: "kilocode",
  "cline-pass": "cline",
  "opencode-go": "opencode",
  "wafer.ai": "wafer",
  agnes: "agnesai",
  bailing: "antgroup",
  "perplexity-agent": "perplexity",
  // ── 常见厂商显示名（预设/手输 vendor 名）──
  moonshot: "kimi",
  阿里云百炼: "bailian",
  百炼: "bailian",
  智谱: "zhipu",
  智谱ai: "zhipu",
  glm: "chatglm",
  gemini: "google",
  "azure openai": "azureai",
  "vertex ai": "vertexai",
  vertex: "vertexai",
  豆包: "doubao",
  字节跳动: "bytedance",
  火山引擎: "volcengine",
  硅基流动: "siliconcloud",
  腾讯混元: "hunyuan",
  混元: "hunyuan",
  讯飞星火: "spark",
  文心一言: "wenxin",
  通义千问: "qwen",
  百川智能: "baichuan",
  零一万物: "yi",
  阶跃星辰: "stepfun",
  商汤: "sensenova",
};

function slugIcon(slug: string): string | null {
  return LOBE_ICONS[`${slug}-color`] ?? LOBE_ICONS[slug] ?? null;
}

/** 厂商名归一化：小写 + 循环剥尾部「括号备注 / 套餐后缀」直到稳定—— *  两种后缀的先后组合都能剥净（"MiniMax Token Plan (minimax.cn)" 先括号后套餐、
 *  "Alibaba (China) Token Plan" 先套餐后括号，单遍任一顺序都会漏一种） */
export function normalizeVendorName(s: string): string {
  let out = s.trim().toLowerCase();
  for (;;) {
    const next = out
      .replace(/\s*[（(][^)）]*[)）]\s*$/, "")
      .replace(/\s+(token plan|coding plan|step plan)$/, "");
    if (next === out) return out;
    out = next;
  }
}

/** 厂商图标资产 URL；providerKey（models.dev key）优先，name 次之；未命中 → null */
export function resolveLobeIcon(providerKey?: string, name?: string): string | null {
  for (const raw of [providerKey, name]) {
    const key = raw ? normalizeVendorName(raw) : "";
    if (!key) continue;
    const hit = slugIcon(PROVIDER_ALIASES[key] ?? key);
    if (hit) return hit;
  }
  return null;
}
