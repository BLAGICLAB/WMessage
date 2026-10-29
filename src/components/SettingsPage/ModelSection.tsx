/** 设置页「model」分区的内容块（从 SettingsPage 主组件搬出）。
 *
 *  挂载与可见性由父组件的 <section hidden={...}> 负责——各分区面板是
 *  同时挂载的（hidden 切可见性），这里只管内容。
 */
import { handleCommandError } from "../../lib/errorHandler";
import { Toggle } from "../Toggle/Toggle";
import { ApiProviderSelect } from "./ApiProviderSelect";
import { ModelRow } from "./ModelRow";
import { ProviderLogo } from "./ProviderLogo";
import { API_FORMAT_OPTIONS, PROVIDER_PRESETS } from "./constants";
import { hostOf } from "./types";
import { emit } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Check, Eye, EyeOff, Info, MoreHorizontal, Plus } from "lucide-react";
import type { Dispatch, RefObject, SetStateAction } from "react";
import type { MetaModel, MetaProvider } from "../../lib/modelMeta";
import type { ConfirmOptions } from "../../lib/useConfirm";
import type { ModelEntry, SettingsConfig } from "./types";

export type ModelSectionProps = {
  needVendorReVerify: boolean;
  botEnabled: boolean;
  vendorGroups: { name: string; isActiveVendor: boolean; }[];
  vendorMetaOf: (name: string) => MetaProvider | null;
  activeVendor: string | null;
  setActiveVendor: Dispatch<SetStateAction<string | null>>;
  config: SettingsConfig;
  setShowProviderPicker: Dispatch<SetStateAction<boolean>>;
  showProviderPicker: boolean;
  metaProviders: MetaProvider[] | null;
  syncMetaLibrary: () => Promise<void>;
  metaSyncBusy: boolean;
  metaSyncMsg: string;
  applyProviderPreset: (p: (typeof PROVIDER_PRESETS)[number]) => Promise<void>;
  metaSearch: string;
  setMetaSearch: Dispatch<SetStateAction<string>>;
  applyMetaProvider: (p: MetaProvider) => Promise<void>;
  metaApplyBusy: boolean;
  activeVendorMeta: MetaProvider | null;
  vendorEnabled: boolean;
  toggleVendorEnabled: (next: boolean) => Promise<void>;
  vendorMenuRef: RefObject<HTMLDivElement | null>;
  vendorMenuOpen: boolean;
  setVendorMenuOpen: Dispatch<SetStateAction<boolean>>;
  activeVendorEntries: ModelEntry[];
  makeVendorActive: () => void;
  vendorOf: (name: string) => { name: string; isActiveVendor: boolean; } | null;
  confirm: (messageOrOptions: string | ConfirmOptions) => Promise<boolean>;
  deleteVendor: (name: string) => Promise<void>;
  vendorBaseUrl: string;
  setVendorBaseUrl: (url: string) => void;
  activeVendorProtocol: "openai" | "anthropic";
  setVendorProtocol: (prov: "openai" | "anthropic") => void;
  activeVendorHasKey: boolean;
  activeVendorKeyUrl: string | null;
  showKey: boolean;
  keyInput: string;
  setKeyInput: Dispatch<SetStateAction<string>>;
  setShowKey: Dispatch<SetStateAction<boolean>>;
  clearVendorKey: () => Promise<void>;
  metaPickerRef: RefObject<HTMLDivElement | null>;
  metaPickerOpen: boolean;
  toggleMetaPicker: () => Promise<void>;
  metaPickerModels: MetaModel[] | null;
  addMetaModel: (m: MetaModel) => Promise<void>;
  addModelToVendor: () => void;
  updateModel: (id: string, patch: Partial<ModelEntry>) => void;
  deleteModel: (id: string) => Promise<void>;
  loadConfig: () => Promise<void>;
  botError: string;
  vendorTestMsg: string;
  vendorTestOk: boolean | null;
  configBusy: boolean;
  vendorTestBusy: boolean;
  saveVendorPage: () => Promise<void>;
  configSaved: boolean;
};

export function ModelSection({
  needVendorReVerify,
  botEnabled,
  vendorGroups,
  vendorMetaOf,
  activeVendor,
  setActiveVendor,
  config,
  setShowProviderPicker,
  showProviderPicker,
  metaProviders,
  syncMetaLibrary,
  metaSyncBusy,
  metaSyncMsg,
  applyProviderPreset,
  metaSearch,
  setMetaSearch,
  applyMetaProvider,
  metaApplyBusy,
  activeVendorMeta,
  vendorEnabled,
  toggleVendorEnabled,
  vendorMenuRef,
  vendorMenuOpen,
  setVendorMenuOpen,
  activeVendorEntries,
  makeVendorActive,
  vendorOf,
  confirm,
  deleteVendor,
  vendorBaseUrl,
  setVendorBaseUrl,
  activeVendorProtocol,
  setVendorProtocol,
  activeVendorHasKey,
  activeVendorKeyUrl,
  showKey,
  keyInput,
  setKeyInput,
  setShowKey,
  clearVendorKey,
  metaPickerRef,
  metaPickerOpen,
  toggleMetaPicker,
  metaPickerModels,
  addMetaModel,
  addModelToVendor,
  updateModel,
  deleteModel,
  loadConfig,
  botError,
  vendorTestMsg,
  vendorTestOk,
  configBusy,
  vendorTestBusy,
  saveVendorPage,
  configSaved,
}: ModelSectionProps) {
  return (
    <>
      {/* 页首：说明（原「刷新」按钮删除——bot_get_config 只合顶层字段，
  厂商列表不跟随刷新，点了像没反应，属于误导性按钮） */}
      <p className="text-xs text-[var(--t5)]">
        管理模型供应商，配置后可在聊天时选择使用。
      </p>
      {/* 收尾引导：有带 vendor 的条目但 verifiedVendors 为空（可用性状态无自动迁移） */}
      {needVendorReVerify && (
        <p
          className="flex items-center gap-1.5 text-xs text-[var(--t3)]"
          role="note"
          aria-label="厂商可用性引导"
        >
          <Info
            size={13}
            className="shrink-0 text-[var(--brand)]"
            aria-hidden
          />
          检测到已有厂商模型但均未通过连接测试：升级后需逐厂商点一次插头恢复可用。
        </p>
      )}
      <div className="nm-card p-5">
        {/* 大模型 API 配置（开关开启后显示） */}
        {botEnabled && (
          <div className="space-y-3">
            {/*  厂商中心：左厂商列表 + 右厂商页；高度跟随窗口（视口高 - 页头占用），两视图对齐 */}
            <div className="flex h-[calc(100vh-14rem)] min-h-[320px] gap-4">
              {/* 左栏：厂商列表（绿点 = 该厂商含全局 active 模型） */}
              <div className="w-44 shrink-0 space-y-1 overflow-y-auto border-r border-[var(--edge)] pr-3">
                <p className="text-xs text-[var(--t5)]">厂商</p>
                {vendorGroups.map((v) => {
                  const meta = vendorMetaOf(v.name);
                  return (
                    <button
                      key={v.name}
                      type="button"
                      aria-current={
                        activeVendor === v.name || undefined
                      }
                      onClick={() => setActiveVendor(v.name)}
                      className={`flex w-full items-center gap-2 rounded-[var(--r-sm)] px-2.5 py-2 text-left text-xs transition-colors duration-[var(--dur-chip)] ease-[var(--ease-out)] ${
                        activeVendor === v.name
                          ? "nm-inset text-[var(--t1)]"
                          : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                      }`}
                    >
                      <ProviderLogo
                        name={v.name}
                        className="h-4 w-4"
                        providerKey={meta?.provider_key}
                        fallbackColor={meta?.fallback_color}
                        fallbackChar={meta?.fallback_char}
                      />
                      <span className="flex-1 truncate">{v.name}</span>
                      {/* 可用性提示点：绿 = 连接测试通过（key/URL/格式变了会自动失效需重测） */}
                      <span
                        className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                          config.verifiedVendors.includes(v.name)
                            ? "bg-[var(--success)]"
                            : "bg-[var(--t6)]"
                        }`}
                        title={
                          config.verifiedVendors.includes(v.name)
                            ? "连接测试已通过，模型可在聊天窗口选用"
                            : "未通过连接测试：点模型行的插头图标测试，通过后模型才进聊天窗口"
                        }
                      />
                    </button>
                  );
                })}
                <button
                  type="button"
                  onClick={() => {
                    setActiveVendor(null);
                    setShowProviderPicker(true);
                  }}
                  className={`flex w-full items-center gap-2 rounded-[var(--r-sm)] px-2.5 py-2 text-left text-xs text-[var(--brand)] hover:bg-[var(--hover-bg)] ${
                    showProviderPicker && activeVendor === null
                      ? "nm-inset"
                      : ""
                  }`}
                >
                  <Plus size={12} aria-hidden />
                  添加厂商
                </button>{" "}
              </div>
              {/* 右栏：厂商页 / 添加厂商网格 / 引导（flex 列：列表区 flex-1 填满剩余高度） */}
              <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
                {showProviderPicker ? (
                  <>
                    <div className="flex items-center gap-2">
                      <button
                        type="button"
                        className="nm-btn px-2 py-1 text-xs text-[var(--t3)]"
                        onClick={() => setShowProviderPicker(false)}
                      >
                        ← 返回
                      </button>
                      <p className="text-sm font-medium text-[var(--t1)]">
                        添加厂商
                      </p>
                    </div>
                    <p className="text-xs text-[var(--t5)]">
                      点选厂商即添加（同名厂商重新选择会覆盖其 Base URL
                      与模型列表）；添加后可改 Base URL、API
                      格式与模型列表。
                    </p>
                    {metaProviders === null ||
                    metaProviders.length === 0 ? (
                      <>
                        <div className="flex items-center gap-2">
                          <p className="min-w-0 flex-1 text-xs text-[var(--t5)]">
                            模型库为空或同步中——启动时自动同步
                            models.dev，也可手动同步
                          </p>
                          <button
                            type="button"
                            className="nm-btn shrink-0 px-2.5 py-1.5 text-xs text-[var(--t3)]"
                            title="从 models.dev 回源同步最新厂商与模型数据"
                            onClick={syncMetaLibrary}
                            disabled={metaSyncBusy}
                          >
                            {metaSyncBusy ? "更新中…" : "更新模型库"}
                          </button>
                        </div>
                        {metaSyncMsg && (
                          <p className="text-xs text-[var(--t5)]">
                            {metaSyncMsg}
                          </p>
                        )}
                        <div className="grid grid-cols-2 gap-2">
                          {PROVIDER_PRESETS.map((p) => (
                            <button
                              key={p.name}
                              type="button"
                              onClick={() => applyProviderPreset(p)}
                              className="nm-btn flex items-center gap-2 px-3 py-2.5 text-left text-xs text-[var(--t2)]"
                            >
                              <ProviderLogo name={p.name} />
                              <span className="flex-1 truncate">
                                {p.name}
                              </span>
                              <span
                                aria-hidden
                                className="text-xs text-[var(--t5)]"
                              >
                                {p.protocol === "anthropic"
                                  ? "Anthropic 格式"
                                  : "OpenAI 格式"}
                              </span>
                            </button>
                          ))}
                        </div>
                      </>
                    ) : (
                      <>
                        <div className="flex items-center gap-2">
                          <input
                            value={metaSearch}
                            onChange={(e) =>
                              setMetaSearch(e.target.value)
                            }
                            placeholder="搜索厂商…"
                            aria-label="搜索厂商"
                            className="nm-inset min-w-0 flex-1 rounded-lg px-2.5 py-1.5 text-xs text-[var(--t3)] outline-none"
                          />
                          <button
                            type="button"
                            className="nm-btn shrink-0 px-2.5 py-1.5 text-xs text-[var(--t3)]"
                            title="从 models.dev 回源同步最新厂商与模型数据"
                            onClick={syncMetaLibrary}
                            disabled={metaSyncBusy}
                          >
                            {metaSyncBusy ? "更新中…" : "更新模型库"}
                          </button>
                        </div>
                        {metaSyncMsg && (
                          <p className="text-xs text-[var(--t5)]">
                            {metaSyncMsg}
                          </p>
                        )}
                        {/* 列数随宽度自适应；flex-1 填满右栏剩余高度，内部滚动保持搜索栏可见 */}
                        <div className="grid min-h-0 flex-1 grid-cols-[repeat(auto-fill,minmax(13rem,1fr))] content-start gap-2 overflow-y-auto">
                          {metaProviders
                            .filter((p) => {
                              const q = metaSearch.trim().toLowerCase();
                              if (!q) return true;
                              return (
                                p.provider_name
                                  .toLowerCase()
                                  .includes(q) ||
                                p.provider_key.toLowerCase().includes(q)
                              );
                            })
                            .map((p) => (
                              <button
                                key={p.provider_key}
                                type="button"
                                onClick={() => applyMetaProvider(p)}
                                disabled={metaApplyBusy}
                                className="nm-btn flex items-center gap-2 px-3 py-2.5 text-left text-xs text-[var(--t2)]"
                              >
                                <ProviderLogo
                                  name={p.provider_name}
                                  providerKey={p.provider_key}
                                  fallbackColor={p.fallback_color}
                                  fallbackChar={p.fallback_char}
                                  className="h-7 w-7"
                                />
                                <span className="flex-1 truncate">
                                  {p.provider_name}
                                </span>
                                <span
                                  aria-hidden
                                  className="truncate text-xs text-[var(--t5)]"
                                >
                                  {hostOf(p.default_base_url)}
                                </span>
                              </button>
                            ))}
                        </div>
                        {metaProviders.length > 0 &&
                          metaSearch.trim() &&
                          !metaProviders.some((p) => {
                            const q = metaSearch.trim().toLowerCase();
                            return (
                              p.provider_name
                                .toLowerCase()
                                .includes(q) ||
                              p.provider_key.toLowerCase().includes(q)
                            );
                          }) && (
                            <p className="text-xs text-[var(--t5)]">
                              无匹配厂商，换个关键词试试
                            </p>
                          )}
                      </>
                    )}
                  </>
                ) : !activeVendor ? (
                  <p className="m-auto text-center text-xs text-[var(--t5)]">
                    从左侧选择一个厂商，或点「添加厂商」接入新的模型供应商
                  </p>
                ) : (
                  <>
                    {/* 厂商标题行：logo + 名称 ｜ 右侧厂商总开关 +「⋯」菜单（设为当前使用/删除厂商） */}
                    <div className="flex items-center justify-between gap-3">
                      <div className="flex min-w-0 items-center gap-2">
                        <ProviderLogo
                          name={activeVendor}
                          className="h-6 w-6"
                          providerKey={activeVendorMeta?.provider_key}
                          fallbackColor={
                            activeVendorMeta?.fallback_color
                          }
                          fallbackChar={activeVendorMeta?.fallback_char}
                        />
                        <h3 className="truncate text-base font-semibold text-[var(--t1)]">
                          {activeVendor}
                        </h3>
                      </div>
                      <div className="flex shrink-0 items-center gap-2">
                        <Toggle
                          checked={vendorEnabled}
                          onChange={toggleVendorEnabled}
                          ariaLabel={`启用厂商 ${activeVendor}`}
                          size="sm"
                        />
                        <div ref={vendorMenuRef} className="relative">
                          <button
                            type="button"
                            aria-label={`厂商 ${activeVendor} 更多操作`}
                            aria-haspopup="menu"
                            aria-expanded={vendorMenuOpen}
                            className="nm-icon-btn text-[var(--t5)] hover:text-[var(--t2)]"
                            onClick={() => setVendorMenuOpen((v) => !v)}
                          >
                            <MoreHorizontal size={15} aria-hidden />
                          </button>
                          {vendorMenuOpen && (
                            <div
                              role="menu"
                              className="nm-popover absolute right-0 z-50 mt-1 w-40 p-1 space-y-0.5 origin-top-right"
                            >
                              <button
                                type="button"
                                role="menuitem"
                                className="w-full text-left px-3 py-1.5 text-xs rounded-lg text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                                disabled={
                                  !activeVendorEntries.length ||
                                  !config.verifiedVendors.includes(
                                    activeVendor,
                                  )
                                }
                                title={
                                  config.verifiedVendors.includes(
                                    activeVendor,
                                  )
                                    ? "把该厂商的 active 模型设为聊天当前使用的模型"
                                    : "先点模型行插头通过「测试连接」，再设为当前使用"
                                }
                                onClick={() => {
                                  setVendorMenuOpen(false);
                                  makeVendorActive();
                                }}
                              >
                                {vendorOf(activeVendor)?.isActiveVendor
                                  ? "当前使用 ✓"
                                  : "设为当前使用"}
                              </button>
                              <button
                                type="button"
                                role="menuitem"
                                className="w-full text-left px-3 py-1.5 text-xs rounded-lg text-[var(--danger)] hover:bg-[var(--hover-bg)]"
                                onClick={async () => {
                                  setVendorMenuOpen(false);
                                  if (
                                    await confirm(
                                      `删除厂商「${activeVendor}」？其全部模型条目将一并移除。`,
                                    )
                                  ) {
                                    deleteVendor(activeVendor);
                                  }
                                }}
                              >
                                删除厂商
                              </button>
                            </div>
                          )}
                        </div>
                      </div>
                    </div>
                    {/* Base URL：厂商条目共用一个（编辑同步全部条目） */}
                    <div className="space-y-1">
                      <p className="text-xs text-[var(--t5)]">
                        Base URL
                      </p>
                      <input
                        value={vendorBaseUrl}
                        onChange={(e) =>
                          setVendorBaseUrl(e.target.value)
                        }
                        placeholder="https://api.example.com/v1"
                        className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
                      />
                    </div>
                    {/* API 格式：该厂商条目所在的协议列表（切换 = 条目整体搬移）；自绘下拉同 ApiProviderSelect 模式 */}
                    <div className="space-y-1">
                      <p className="text-xs text-[var(--t5)]">
                        API 格式
                      </p>
                      <ApiProviderSelect
                        value={activeVendorProtocol}
                        onChange={setVendorProtocol}
                        options={API_FORMAT_OPTIONS}
                      />
                    </div>
                    {/* API Key：按厂商分存（keyring 条目按厂商名），各厂商独立——挂账记录 */}
                    <div className="space-y-1">
                      <div className="flex items-center justify-between">
                        <p className="text-xs text-[var(--t5)]">
                          API Key
                          {activeVendorHasKey && (
                            <span className="text-[var(--success)]">
                              {" "}
                              · 已存入系统凭据存储 ✓
                            </span>
                          )}
                        </p>
                        {activeVendorKeyUrl && (
                          <button
                            type="button"
                            className="text-xs text-[var(--brand)] hover:underline"
                            onClick={() => {
                              openUrl(activeVendorKeyUrl).catch((e) =>
                                handleCommandError(e, "open url"),
                              );
                            }}
                          >
                            获取 API Key
                          </button>
                        )}
                      </div>
                      <div className="relative">
                        <input
                          type={showKey ? "text" : "password"}
                          value={keyInput}
                          onChange={(e) => setKeyInput(e.target.value)}
                          placeholder={
                            activeVendorHasKey
                              ? "已保存（输入新 Key 可覆盖）"
                              : "sk-…"
                          }
                          className="nm-inset w-full rounded-xl px-3 py-2 pr-9 text-xs text-[var(--t3)] outline-none"
                        />
                        <button
                          type="button"
                          aria-label={
                            showKey ? "隐藏 API Key" : "显示 API Key"
                          }
                          className="absolute right-2 top-1/2 -translate-y-1/2 text-[var(--t5)] hover:text-[var(--t2)]"
                          onClick={() => setShowKey((v) => !v)}
                        >
                          {showKey ? (
                            <EyeOff size={13} aria-hidden />
                          ) : (
                            <Eye size={13} aria-hidden />
                          )}
                        </button>
                      </div>
                      {activeVendorHasKey && (
                        <button
                          className="text-xs text-[var(--danger)] hover:underline"
                          onClick={clearVendorKey}
                        >
                          清除已保存的 Key
                        </button>
                      )}
                    </div>
                    {/* 模型列表：行 = 名称 + 上下文徽标 + 能力徽标 + 插头（连接测试）+ 铅笔编辑 + 开关；flex-1 与添加厂商网格同高对齐 */}
                    <div className="flex min-h-0 flex-1 flex-col gap-1">
                      <div className="flex items-center justify-between">
                        <p className="text-xs text-[var(--t5)]">
                          模型列表
                        </p>
                        <div className="flex items-center gap-1.5">
                          {activeVendorMeta && (
                            <div
                              ref={metaPickerRef}
                              className="relative"
                            >
                              <button
                                type="button"
                                aria-haspopup="listbox"
                                aria-expanded={metaPickerOpen}
                                className="nm-btn px-2.5 py-1 text-xs text-[var(--t3)]"
                                title="从模型库挑选该厂商的模型，自动填充上下文与推理参数；新模型默认禁用"
                                onClick={toggleMetaPicker}
                              >
                                从模型库添加 ▾
                              </button>
                              {metaPickerOpen && (
                                <div
                                  role="listbox"
                                  aria-label="从模型库添加"
                                  className="nm-popover absolute right-0 z-50 mt-1 max-h-56 w-64 space-y-0.5 overflow-y-auto p-1 origin-top-right"
                                >
                                  {(() => {
                                    const added = new Set(
                                      activeVendorEntries.map(
                                        (m) => m.model,
                                      ),
                                    );
                                    const options = (
                                      metaPickerModels ?? []
                                    ).filter(
                                      (m) =>
                                        !added.has(
                                          m.model_key.includes("/")
                                            ? m.model_key.slice(
                                                m.model_key.indexOf(
                                                  "/",
                                                ) + 1,
                                              )
                                            : m.model_key,
                                        ),
                                    );
                                    if (metaPickerModels === null)
                                      return (
                                        <p className="px-3 py-1.5 text-xs text-[var(--t5)]">
                                          模型库服务不可用
                                        </p>
                                      );
                                    if (options.length === 0)
                                      return (
                                        <p className="px-3 py-1.5 text-xs text-[var(--t5)]">
                                          模型库中的模型均已添加
                                        </p>
                                      );
                                    return options.map((m) => (
                                      <button
                                        key={m.model_key}
                                        type="button"
                                        role="option"
                                        aria-selected={false}
                                        className="flex w-full items-center gap-2 rounded-lg px-3 py-1.5 text-left text-xs text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                                        onClick={() => addMetaModel(m)}
                                      >
                                        <span className="min-w-0 flex-1 truncate">
                                          {m.display_name ||
                                            m.model_key}
                                        </span>
                                        {m.context_length ? (
                                          <span
                                            aria-hidden
                                            className="shrink-0 font-mono text-xs text-[var(--t5)]"
                                          >
                                            {Math.round(
                                              m.context_length / 100,
                                            ) / 10}
                                            K
                                          </span>
                                        ) : null}
                                      </button>
                                    ));
                                  })()}
                                </div>
                              )}
                            </div>
                          )}
                          <button
                            type="button"
                            className="nm-btn px-2.5 py-1 text-xs text-[var(--t3)]"
                            onClick={addModelToVendor}
                          >
                            ＋ 添加模型
                          </button>
                        </div>
                      </div>
                      {activeVendorEntries.length === 0 ? (
                        <p className="py-1 text-xs text-[var(--t5)]">
                          暂无模型，点「＋ 添加模型」开始添加
                        </p>
                      ) : (
                        <div className="min-h-0 flex-1 space-y-1.5 overflow-y-auto">
                          {activeVendorEntries.map((m) => (
                            <ModelRow
                              key={m.id}
                              model={m}
                              apiProvider={activeVendorProtocol}
                              vendorDisabled={!vendorEnabled}
                              vendorVerified={config.verifiedVendors.includes(
                                activeVendor ?? "",
                              )}
                              onChange={(patch) =>
                                updateModel(m.id, patch)
                              }
                              onDelete={() => deleteModel(m.id)}
                              onTested={async () => {
                                // 可用性已落盘：await reload 拿到最新 verifiedVendors 再广播——
                                // 否则旧 state 的下一次整写会把刚落盘的验证状态覆盖掉
                                await loadConfig();
                                emit("bot-config-changed", null).catch(
                                  () => {},
                                );
                              }}
                            />
                          ))}
                        </div>
                      )}
                      <p className="text-xs text-[var(--t6)] leading-snug">
                        插头 =
                        连接测试：通过后厂商点亮绿点、开启的模型才进聊天窗口下拉（key/URL/格式变更后需重新测试）。当前使用的模型在聊天窗口
                        🧠 下拉切换；max_tokens 在各模型的编辑里设置（仅
                        Anthropic 格式生效）。
                      </p>
                    </div>
                    {/* 全局 max_tokens 已随 2026-10-29 参数全常量化退役（条目级 max_tokens > 内置
        默认 16384 两层）；保存不再透传盘上旧全局值，它会随下次保存清空 */}
                    {/* 保存 + 自动连接测试：保存成功后厂商下有开启的模型即自动探测，
        结果决定左栏绿点与聊天下拉可见性 */}
                    <div className="mt-3 flex items-center justify-end gap-3">
                      {botError && (
                        <p className="text-xs text-[var(--danger)]">
                          {botError}
                        </p>
                      )}
                      {vendorTestMsg && (
                        <p
                          className={`text-xs ${
                            vendorTestOk
                              ? "text-[var(--success)]"
                              : "text-[var(--danger)]"
                          }`}
                        >
                          {vendorTestMsg}
                        </p>
                      )}
                      <button
                        aria-label="保存配置"
                        className={`btn-sm-action ${
                          configBusy || vendorTestBusy
                            ? "nm-inset"
                            : "nm-outset"
                        }`}
                        onClick={saveVendorPage}
                        disabled={configBusy || vendorTestBusy}
                      >
                        {configSaved ? (
                          <>
                            <Check size={12} aria-hidden /> 已保存
                          </>
                        ) : configBusy ? (
                          "保存中…"
                        ) : vendorTestBusy ? (
                          "测试连接中…"
                        ) : (
                          "保存配置"
                        )}
                      </button>
                    </div>
                  </>
                )}
              </div>
            </div>
          </div>
        )}
      </div>
    </>
  );
}
