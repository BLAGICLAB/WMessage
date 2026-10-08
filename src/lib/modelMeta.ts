// 内置 Rust meta 模块的 IPC 客户端，异常返回 null 由调用方降级
// （设置页回退内置预设网格）。
// 命令参数按 Tauri 约定传 camelCase（自动映射 Rust 侧 snake_case）。

import { invoke } from "@tauri-apps/api/core";

/** meta_list_providers 的单条记录（snake_case 与 Rust 侧序列化对齐） */
export type MetaProvider = {
  provider_key: string;
  provider_name: string;
  logo_url: string | null;
  fallback_color: string;
  fallback_char: string;
  default_base_url: string | null;
  timeout: number | null;
  source?: string;
};

/** meta_models_by_provider 的单条记录 */
export type MetaModel = {
  /** 形如 "openai/gpt-4o"：写入 ModelEntry.model 时去掉 "provider/" 前缀 */
  model_key: string;
  provider_key: string;
  display_name: string;
  context_length: number | null;
  temperature: number | null;
  top_p: number | null;
  max_tokens: number | null;
  default_system_prompt: string | null;
  source?: string;
};

/** 同步结果：与 Rust 侧 MetaSyncResult 的两种实际形态一一对应（判别联合）—— *  ok:true 时 providers/models 必有；ok:false 时 error 必有 */
export type MetaSyncResult =
  | { ok: true; providers: number; models: number }
  | { ok: false; error: string };

/** 全部服务商；null = meta 模块异常（调用方按降级路径处理） */
export async function fetchProviders(): Promise<MetaProvider[] | null> {
  try {
    return await invoke<MetaProvider[]>("meta_list_providers");
  } catch {
    return null;
  }
}

/** 指定服务商下全部模型；null = meta 模块异常 */
export async function fetchModelsByProvider(
  providerKey: string,
): Promise<MetaModel[] | null> {
  try {
    return await invoke<MetaModel[]>("meta_models_by_provider", { providerKey });
  } catch {
    return null;
  }
}

/** 手动触发 models.dev 同步（「更新模型库」按钮）；null = meta 模块异常 */
export async function syncModelsDev(): Promise<MetaSyncResult | null> {
  try {
    return await invoke<MetaSyncResult>("meta_sync_models_dev");
  } catch {
    return null;
  }
}
