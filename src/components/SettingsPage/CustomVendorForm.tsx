/** 设置页「添加厂商」里的自定义厂商表单：自拟名称 + Base URL + 首个模型名，
 *  key 可空（勾「免鉴权」= 本地模型服务，请求不带鉴权头、探活匿名转正）。
 *  常见本地服务（Ollama / LM Studio / vLLM）一键预填名称与默认端点。
 *  字段 state 留在本组件（纯表单态），提交走父级的 addCustomVendor。 */
import { useState } from "react";
import { ApiProviderSelect } from "./ApiProviderSelect";
import { API_FORMAT_OPTIONS } from "./constants";
import { LOCAL_SERVER_PRESETS } from "./types";
import type { CustomVendorDraft } from "./types";

export type CustomVendorFormProps = {
  onSubmit: (draft: CustomVendorDraft) => Promise<void>;
  busy: boolean;
};

export function CustomVendorForm({ onSubmit, busy }: CustomVendorFormProps) {
  const [name, setName] = useState("");
  const [protocol, setProtocol] = useState<"openai" | "anthropic">("openai");
  const [baseUrl, setBaseUrl] = useState("");
  const [model, setModel] = useState("");
  const [key, setKey] = useState("");
  const [noAuth, setNoAuth] = useState(false);
  const valid =
    name.trim().length > 0 && baseUrl.trim().length > 0 && model.trim().length > 0;
  const submit = () => {
    if (!valid || busy) return;
    void onSubmit({ name, protocol, baseUrl, model, key, noAuth });
  };
  return (
    <div className="nm-inset space-y-2.5 rounded-xl p-3" data-testid="custom-vendor-form">
      <div className="flex items-center gap-1.5">
        {LOCAL_SERVER_PRESETS.map((p) => (
          <button
            key={p.name}
            type="button"
            className="nm-btn px-2 py-1 text-xs text-[var(--t3)]"
            title={`预填 ${p.name} 默认端点（${p.baseUrl}）`}
            onClick={() => {
              setName((cur) => cur.trim() || p.name);
              setBaseUrl(p.baseUrl);
            }}
          >
            {p.name}
          </button>
        ))}
        <span className="ml-auto text-xs text-[var(--t6)]">
          本地服务点上面快捷预填，或全部手填
        </span>
      </div>
      <div className="grid grid-cols-[7rem_1fr] items-center gap-2">
        <label
          htmlFor="custom-vendor-name"
          className="text-xs text-[var(--t5)]"
        >
          厂商名称
        </label>
        <input
          id="custom-vendor-name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="如：我的本地模型 / 公司网关"
          className="nm-inset w-full rounded-lg px-2.5 py-1.5 text-xs text-[var(--t3)] outline-none"
        />
        <label
          htmlFor="custom-vendor-format"
          className="text-xs text-[var(--t5)]"
        >
          API 格式
        </label>
        <ApiProviderSelect
          value={protocol}
          onChange={setProtocol}
          options={API_FORMAT_OPTIONS}
        />
        <label
          htmlFor="custom-vendor-url"
          className="text-xs text-[var(--t5)]"
        >
          Base URL
        </label>
        <input
          id="custom-vendor-url"
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          placeholder="http://localhost:11434/v1"
          className="nm-inset w-full rounded-lg px-2.5 py-1.5 text-xs text-[var(--t3)] outline-none"
        />
        <label
          htmlFor="custom-vendor-model"
          className="text-xs text-[var(--t5)]"
        >
          模型名称
        </label>
        <input
          id="custom-vendor-model"
          value={model}
          onChange={(e) => setModel(e.target.value)}
          placeholder="如：qwen3:8b（添加后可继续加模型）"
          className="nm-inset w-full rounded-lg px-2.5 py-1.5 text-xs text-[var(--t3)] outline-none"
        />
      </div>
      <label className="flex cursor-pointer items-center gap-2 text-xs text-[var(--t3)]">
        <input
          type="checkbox"
          checked={noAuth}
          onChange={(e) => setNoAuth(e.target.checked)}
          className="accent-[var(--brand)]"
        />
        此服务无需 API Key（本地服务，请求不带鉴权头）
      </label>
      {!noAuth && (
        <input
          type="password"
          value={key}
          onChange={(e) => setKey(e.target.value)}
          placeholder="API Key（可留空，之后在厂商页补填）"
          aria-label="自定义厂商 API Key"
          className="nm-inset w-full rounded-lg px-2.5 py-1.5 text-xs text-[var(--t3)] outline-none"
        />
      )}
      <div className="flex items-center justify-end gap-2">
        {!valid && (
          <span className="text-xs text-[var(--t6)]">
            厂商名称、Base URL、模型名称都要填
          </span>
        )}
        <button
          type="button"
          className={`btn-sm-action ${busy || !valid ? "nm-inset" : "nm-outset"}`}
          onClick={submit}
          disabled={!valid || busy}
        >
          {busy ? "保存中…" : "添加此厂商"}
        </button>
      </div>
    </div>
  );
}
