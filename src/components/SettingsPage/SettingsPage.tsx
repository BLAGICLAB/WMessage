// SettingsPage 主组件（orchestrator）。
// 1846 → 1340 行：types / constants / SkillsPanel / ApiProviderSelect / ProfileRow / ModelRow
// 拆到同目录子文件；后来 8 个分区区块拆成 *Section.tsx，状态与 handler 又按域拆进
// useSettingsActions / useSettingsVendors / useSettingsOps——本文件现在只剩门面：
// 分区导航状态、三个 hook 的挂载骨架、JSX render。
//
// 公开 import 路径保持稳定：外部仍 `import { SettingsPage } from "./components/SettingsPage"`，
// Vite 解析到 `./SettingsPage/index.tsx` → 透传 `./SettingsPage`。

import { useRef, useState } from "react";
import { useConfirm } from "../../lib/useConfirm";
import { useDismissable } from "../../lib/useDismissable";
import { ArrowLeft, BarChart3, Bot, Brain, Cpu, DatabaseBackup, FolderOutput, History, Plug, ScrollText, Server, Settings2, Sparkles, Waypoints, LucideIcon } from "lucide-react";
import { NAV_W } from "../../lib/navWidth";
import type { ThemeSetting } from "../../theme";
import { MigrationPanel } from "../MigrationPanel";
import type { SectionKey } from "./types";
import { SkillsPanel } from "./SkillsPanel";
import { OnionsPanel } from "./OnionsPanel";
import { WordTemplatePanel } from "./WordTemplatePanel";
import { McpPanel } from "./McpPanel";
import { ServicesPanel } from "./ServicesPanel";
import { LogsPanel } from "./LogsPanel";
import GraphSettingsPanel from "./GraphSettingsPanel";
import { UsageStatsCard } from "./UsageStatsCard";
import { OperationHistoryTab } from "./OperationHistoryTab";
import { GeneralSection } from "./GeneralSection";
import { DataSection } from "./DataSection";
import { BotCoreSection } from "./BotCoreSection";
import { MemorySection } from "./MemorySection";
import { ModelSection } from "./ModelSection";
import { BotApiSection } from "./BotApiSection";
import { McpSection } from "./McpSection";
import { BotToolsSection } from "./BotToolsSection";
import { useSettingsActions } from "./useSettingsActions";
import { useSettingsVendors } from "./useSettingsVendors";
import { useSettingsOps } from "./useSettingsOps";

type Props = {
  theme: ThemeSetting;
  onThemeChange: (t: ThemeSetting) => void;
  onExportTasks: () => Promise<void>;
  onImportTasks: () => Promise<void>;
  /** 可选：未传时按钮置 disabled（App.tsx 已传；测试可选） */
  onExportWorkspace?: () => Promise<void>;
  onImportWorkspace?: () => Promise<void>;
  /** 返回入口（设置壳）：未传时按钮 disabled（App 恒传；测试可省） */
  onBack?: () => void;
};

const SECTIONS: { key: SectionKey; label: string; icon: LucideIcon }[] = [
  { key: "general", label: "基础设置", icon: Settings2 },
  { key: "data", label: "数据管理", icon: DatabaseBackup },
  { key: "bot", label: "机器人", icon: Bot },
  { key: "model", label: "模型设置", icon: Cpu },
  { key: "memory", label: "记忆", icon: Brain },
  { key: "skills", label: "技能", icon: Sparkles },
  { key: "graph", label: "任务图谱", icon: Waypoints },
  { key: "mcp", label: "MCP 服务", icon: Plug },
  { key: "services", label: "本地服务", icon: Server },
  { key: "desk", label: "文件整理", icon: FolderOutput },
  { key: "tokens", label: "词元统计", icon: BarChart3 },
  { key: "opsHistory", label: "操作历史", icon: History },
  { key: "logs", label: "日志", icon: ScrollText },
];

/** 设置壳（重设计）：左侧分类导航 + 右侧分类内容（大标题 + 卡片流）。
 *  面板惰性挂载：分类首次激活才挂载，挂后保留（hidden 切换）——
 *  首屏不跑未访问面板的加载 invoke，未保存输入跨分类保留。 */
export function SettingsPage({
  theme,
  onThemeChange,
  onExportTasks,
  onImportTasks,
  onExportWorkspace,
  onImportWorkspace,
  onBack,
}: Props) {
  const confirm = useConfirm();
  const [activeSection, setActiveSection] = useState<SectionKey>("general");
  const {
    autostartBusy,
    autostartEnabled,
    autostartError,
    botBusy,
    botEnabled,
    botError,
    braveKeyInput,
    checkPyEnv,
    clearTraces,
    config,
    configBusy,
    configSaved,
    deleteNonSelfBusy,
    deleteNonSelfDone,
    exposeNetAllowed,
    exposeNetBusy,
    extractIntervalText,
    loadConfig,
    loadParamViews,
    paramMaxText,
    paramNumber,
    paramViews,
    pyBusy,
    pyEnabled,
    pyEnv,
    pyEnvBusy,
    pyEnvErr,
    pyTimeoutView,
    runDeleteNonSelf,
    servicesBusy,
    servicesEnabled,
    setAutostartBusy,
    setAutostartEnabled,
    setAutostartError,
    setBotError,
    setBraveKeyInput,
    setConfig,
    setConfigBusy,
    setConfigSaved,
    setTavilyKeyInput,
    setTraceRetainDays,
    tavilyKeyInput,
    toggleBot,
    toggleExposeNet,
    togglePy,
    toggleServices,
    toggleUi,
    traceClearBusy,
    traceClearMsg,
    traceRetainDays,
    traceRetentionDays,
    traceRetentionTitle,
    uiBusy,
    uiEnabled,
  } = useSettingsActions({
    confirm,
  });
  const [vendorMenuOpen, setVendorMenuOpen] = useState(false);
  const vendorMenuRef = useRef<HTMLDivElement>(null);
  useDismissable(vendorMenuOpen, vendorMenuRef, () => setVendorMenuOpen(false));
  const {
    activeVendor,
    activeVendorEntries,
    activeVendorHasKey,
    activeVendorKeyUrl,
    activeVendorMeta,
    activeVendorProtocol,
    addMetaModel,
    addModelToVendor,
    applyMetaProvider,
    applyProviderPreset,
    deleteModel,
    deleteVendor,
    keyInput,
    makeVendorActive,
    metaApplyBusy,
    metaPickerModels,
    metaPickerOpen,
    metaProviders,
    metaSearch,
    metaSyncBusy,
    metaSyncMsg,
    mountedSections,
    needVendorReVerify,
    openSection,
    renderSaveButton,
    saveConfig,
    saveVendorPage,
    setActiveVendor,
    setKeyInput,
    setMetaPickerOpen,
    setMetaSearch,
    setShowKey,
    setShowProviderPicker,
    setVendorBaseUrl,
    setVendorProtocol,
    showKey,
    showProviderPicker,
    syncMetaLibrary,
    toggleMetaPicker,
    toggleVendorEnabled,
    updateModel,
    vendorBaseUrl,
    vendorEnabled,
    vendorGroups,
    vendorMetaOf,
    vendorOf,
    vendorTestBusy,
    vendorTestMsg,
    vendorTestOk,
  } = useSettingsVendors({
    braveKeyInput,
    config,
    configBusy,
    configSaved,
    confirm,
    loadConfig,
    loadParamViews,
    paramNumber,
    setActiveSection,
    setBotError,
    setBraveKeyInput,
    setConfig,
    setConfigBusy,
    setConfigSaved,
    setTavilyKeyInput,
    tavilyKeyInput,
  });
  const metaPickerRef = useRef<HTMLDivElement>(null);
  useDismissable(metaPickerOpen, metaPickerRef, () => setMetaPickerOpen(false));
  const {
    busy,
    clearVendorKey,
    consolidateBusy,
    consolidateMsg,
    consolidateNow,
    copied,
    copyToken,
    error,
    exporting,
    exportingWs,
    importing,
    importingWs,
    renderMemoryToggle,
    rotateToken,
    runExport,
    runExportWs,
    runImport,
    runImportWs,
    setConsolidation,
    setMemoryControl,
    setPermMode,
    status,
    toggle,
    toggleAutostart,
    toggleBrave,
    toggleTavily,
  } = useSettingsOps({
    activeVendor,
    autostartBusy,
    autostartEnabled,
    config,
    configBusy,
    confirm,
    loadConfig,
    onExportTasks,
    onExportWorkspace,
    onImportTasks,
    onImportWorkspace,
    saveConfig,
    setAutostartBusy,
    setAutostartEnabled,
    setAutostartError,
    setBotError,
    setConfig,
    setConfigBusy,
  });
  const activeMeta =
    SECTIONS.find((s) => s.key === activeSection) ?? SECTIONS[0];
  return (
    <div className="flex h-full min-h-0 bg-[var(--bg)]">
      {/* 左侧分类导航（设置壳）：返回 + 分类项（选中 nm-inset 语义）。
          与主导航共用 NAV_W：同一个壳、同一套字号，取宽的那档给它只会
          多余不会截断；分类项本身不带快捷键提示条 */}
      <aside
        aria-label="设置分类"
        style={{ width: NAV_W }}
        className="flex shrink-0 flex-col gap-1 overflow-y-auto border-r border-[var(--edge)] p-3"
      >
        <button
          type="button"
          className="nm-btn mb-2 flex h-8 items-center gap-2 px-2.5 text-sm text-[var(--t3)]"
          onClick={() => onBack?.()}
          disabled={!onBack}
          title="返回主界面"
        >
          <ArrowLeft size={15} aria-hidden />
          <span className="min-w-0 truncate">返回</span>
        </button>
        {SECTIONS.map((s) => (
          <button
            key={s.key}
            type="button"
            aria-current={activeSection === s.key || undefined}
            onClick={() => openSection(s.key)}
            className={`flex h-8 w-full items-center gap-2 rounded-[var(--r-sm)] px-2.5 text-sm transition-colors duration-[var(--dur-chip)] ease-[var(--ease-out)] ${
              activeSection === s.key
                ? "nm-inset rounded-[var(--r-sm)] text-[var(--t1)]"
                : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
            }`}
          >
            <s.icon size={15} aria-hidden />
            {/* truncate + min-w-0：特大系统字号下省略而不是换行 */}
            <span className="min-w-0 flex-1 truncate text-left">
              {s.label}
            </span>
          </button>
        ))}
      </aside>
      {/* 右侧分类内容：大标题 + 卡片流（max-w 收行宽，截图节奏） */}
      <div className="min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl px-8 py-6">
          <h1 className="text-xl font-semibold text-[var(--t1)]">
            {activeMeta.label}
          </h1>
          {mountedSections.has("general") && (
            <section
              hidden={activeSection !== "general"}
              className="space-y-4 pt-4"
            >
              <GeneralSection
                theme={theme}
                onThemeChange={onThemeChange}
                config={config}
                setConfig={setConfig}
                saveConfig={saveConfig}
                autostartError={autostartError}
                autostartEnabled={autostartEnabled}
                toggleAutostart={toggleAutostart}
                autostartBusy={autostartBusy}
              />
            </section>
          )}
          {mountedSections.has("data") && (
            <section
              hidden={activeSection !== "data"}
              className="space-y-4 pt-4"
            >
              <DataSection
                exporting={exporting}
                runExport={runExport}
                importing={importing}
                runImport={runImport}
                deleteNonSelfBusy={deleteNonSelfBusy}
                deleteNonSelfDone={deleteNonSelfDone}
                runDeleteNonSelf={runDeleteNonSelf}
                config={config}
                setConfig={setConfig}
                renderSaveButton={renderSaveButton}
                traceRetentionDays={traceRetentionDays}
                traceRetainDays={traceRetainDays}
                setTraceRetainDays={setTraceRetainDays}
                traceRetentionTitle={traceRetentionTitle}
                traceClearBusy={traceClearBusy}
                clearTraces={clearTraces}
                traceClearMsg={traceClearMsg}
                exportingWs={exportingWs}
                runExportWs={runExportWs}
                importingWs={importingWs}
                runImportWs={runImportWs}
              />
            </section>
          )}
          {mountedSections.has("bot") && (
            <section
              hidden={activeSection !== "bot"}
              className="space-y-4 pt-4"
            >
              <BotCoreSection
                botEnabled={botEnabled}
                toggleBot={toggleBot}
                botBusy={botBusy}
                botError={botError}
                pyEnabled={pyEnabled}
                togglePy={togglePy}
                pyBusy={pyBusy}
                servicesEnabled={servicesEnabled}
                toggleServices={toggleServices}
                servicesBusy={servicesBusy}
                exposeNetAllowed={exposeNetAllowed}
                toggleExposeNet={toggleExposeNet}
                exposeNetBusy={exposeNetBusy}
                uiEnabled={uiEnabled}
                toggleUi={toggleUi}
                uiBusy={uiBusy}
                pyEnvErr={pyEnvErr}
                pyEnv={pyEnv}
                pyEnvBusy={pyEnvBusy}
                checkPyEnv={checkPyEnv}
                pyTimeoutView={pyTimeoutView}
                config={config}
                setConfig={setConfig}
                renderSaveButton={renderSaveButton}
                paramMaxText={paramMaxText}
                configBusy={configBusy}
                saveConfig={saveConfig}
                paramViews={paramViews}
              />
            </section>
          )}
          {mountedSections.has("memory") && (
            <section
              hidden={activeSection !== "memory"}
              className="space-y-4 pt-4"
            >
              <MemorySection
                config={config}
                setConsolidation={setConsolidation}
                configBusy={configBusy}
                consolidateBusy={consolidateBusy}
                consolidateNow={consolidateNow}
                consolidateMsg={consolidateMsg}
                renderMemoryToggle={renderMemoryToggle}
                extractIntervalText={extractIntervalText}
                setMemoryControl={setMemoryControl}
              />
            </section>
          )}
          {mountedSections.has("model") && (
            <section
              hidden={activeSection !== "model"}
              className="space-y-4 pt-4"
            >
              <ModelSection
                needVendorReVerify={needVendorReVerify}
                botEnabled={botEnabled}
                vendorGroups={vendorGroups}
                vendorMetaOf={vendorMetaOf}
                activeVendor={activeVendor}
                setActiveVendor={setActiveVendor}
                config={config}
                setShowProviderPicker={setShowProviderPicker}
                showProviderPicker={showProviderPicker}
                metaProviders={metaProviders}
                syncMetaLibrary={syncMetaLibrary}
                metaSyncBusy={metaSyncBusy}
                metaSyncMsg={metaSyncMsg}
                applyProviderPreset={applyProviderPreset}
                metaSearch={metaSearch}
                setMetaSearch={setMetaSearch}
                applyMetaProvider={applyMetaProvider}
                metaApplyBusy={metaApplyBusy}
                activeVendorMeta={activeVendorMeta}
                vendorEnabled={vendorEnabled}
                toggleVendorEnabled={toggleVendorEnabled}
                vendorMenuRef={vendorMenuRef}
                vendorMenuOpen={vendorMenuOpen}
                setVendorMenuOpen={setVendorMenuOpen}
                activeVendorEntries={activeVendorEntries}
                makeVendorActive={makeVendorActive}
                vendorOf={vendorOf}
                confirm={confirm}
                deleteVendor={deleteVendor}
                vendorBaseUrl={vendorBaseUrl}
                setVendorBaseUrl={setVendorBaseUrl}
                activeVendorProtocol={activeVendorProtocol}
                setVendorProtocol={setVendorProtocol}
                activeVendorHasKey={activeVendorHasKey}
                activeVendorKeyUrl={activeVendorKeyUrl}
                showKey={showKey}
                keyInput={keyInput}
                setKeyInput={setKeyInput}
                setShowKey={setShowKey}
                clearVendorKey={clearVendorKey}
                metaPickerRef={metaPickerRef}
                metaPickerOpen={metaPickerOpen}
                toggleMetaPicker={toggleMetaPicker}
                metaPickerModels={metaPickerModels}
                addMetaModel={addMetaModel}
                addModelToVendor={addModelToVendor}
                updateModel={updateModel}
                deleteModel={deleteModel}
                loadConfig={loadConfig}
                botError={botError}
                vendorTestMsg={vendorTestMsg}
                vendorTestOk={vendorTestOk}
                configBusy={configBusy}
                vendorTestBusy={vendorTestBusy}
                saveVendorPage={saveVendorPage}
                configSaved={configSaved}
              />
            </section>
          )}
          {mountedSections.has("bot") && (
            <section
              hidden={activeSection !== "bot"}
              className="space-y-4 pt-4"
            >
              <BotApiSection
                config={config}
                setPermMode={setPermMode}
                configBusy={configBusy}
                setConfig={setConfig}
                renderSaveButton={renderSaveButton}
              />
            </section>
          )}
          {mountedSections.has("mcp") && (
            <section
              hidden={activeSection !== "mcp"}
              className="space-y-4 pt-4"
            >
              <McpSection
                config={config}
                toggleTavily={toggleTavily}
                configBusy={configBusy}
                tavilyKeyInput={tavilyKeyInput}
                setTavilyKeyInput={setTavilyKeyInput}
                toggleBrave={toggleBrave}
                braveKeyInput={braveKeyInput}
                setBraveKeyInput={setBraveKeyInput}
                renderSaveButton={renderSaveButton}
              />
            </section>
          )}
          {mountedSections.has("bot") && (
            <section
              hidden={activeSection !== "bot"}
              className="space-y-4 pt-4"
            >
              <BotToolsSection
                config={config}
                setConfig={setConfig}
                configBusy={configBusy}
                status={status}
                toggle={toggle}
                busy={busy}
                error={error}
                copyToken={copyToken}
                copied={copied}
                rotateToken={rotateToken}
                renderSaveButton={renderSaveButton}
              />
            </section>
          )}
          {mountedSections.has("skills") && (
            <section
              hidden={activeSection !== "skills"}
              className="space-y-4 pt-4"
            >
              <SkillsPanel />
              <OnionsPanel />
              <WordTemplatePanel />
            </section>
          )}
          {mountedSections.has("mcp") && (
            <section
              hidden={activeSection !== "mcp"}
              className="space-y-4 pt-4"
            >
              {/* active：分类隐藏时面板不卸载（惰性挂载），可见性要传下去，
                  否则它在后台一直 5s 一次地拍 ps / lsof */}
              <McpPanel active={activeSection === "mcp"} />
            </section>
          )}
          {mountedSections.has("services") && (
            <section
              hidden={activeSection !== "services"}
              className="space-y-4 pt-4"
            >
              <ServicesPanel />
            </section>
          )}
          {mountedSections.has("graph") && (
            <section
              hidden={activeSection !== "graph"}
              className="space-y-4 pt-4"
            >
              <GraphSettingsPanel />
            </section>
          )}
          {mountedSections.has("desk") && (
            <section
              hidden={activeSection !== "desk"}
              className="space-y-4 pt-4"
            >
              <MigrationPanel />
            </section>
          )}
          {mountedSections.has("tokens") && (
            <section
              hidden={activeSection !== "tokens"}
              className="space-y-4 pt-4"
            >
              {/* 词元统计实装（原「规划中」占位）——exec_traces 按日聚合，纯本地 */}
              <UsageStatsCard />
            </section>
          )}

          {mountedSections.has("opsHistory") && (
            <section
              hidden={activeSection !== "opsHistory"}
              className="space-y-4 pt-4"
            >
              {/* 标签整理 / 未来其他破坏性操作的可回滚历史；列最近 50 条，
                每条带「撤销」按钮（按 run_id 调 rollback 命令） */}
              <OperationHistoryTab />
            </section>
          )}
          {mountedSections.has("logs") && (
            <section
              hidden={activeSection !== "logs"}
              className="space-y-4 pt-4"
            >
              {/* 错误 / 机器人审计 / 文件迁移三份日志 + 一键整包复制 */}
              <LogsPanel />
            </section>
          )}
        </div>
      </div>
    </div>
  );
}

