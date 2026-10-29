/// <reference types="node" />
// 任务模式三档选择（设置页「Agent 运行参数」卡顶部）单测。
//
// 盯住三件容易出事的：① 三个档位都渲染且当前档位高亮；② 切档位时
// **清掉逐项覆盖**（覆盖优先于档位——留着旧覆盖会让「换了档位却没变化」，
// 用户看不出是覆盖卡住了）；③ 切档位要落盘 taskMode。

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { BotCoreSection } from "./BotCoreSection";
import type { SettingsConfig } from "./types";

function baseConfig(over?: Partial<SettingsConfig>): SettingsConfig {
  return {
    apiProvider: "openai",
    modelsByProvider: { openai: [], anthropic: [] },
    activeModelId: { openai: null, anthropic: null },
    disabledVendors: [],
    verifiedVendors: [],
    uiFontSize: "small",
    hasApiKey: false,
    vendorKeys: [],
    bypassLlmOnPreStepHit: true,
    allowedDirs: "",
    guardClaimKeywords: "",
    guardClaimKeywordsDefault: [],
    hasTavilyKey: false,
    tavilyEnabled: false,
    hasBraveKey: false,
    braveEnabled: false,
    pythonTimeoutSecs: "",
    maxFunctionCalls: "",
    archiveAfterDays: "",
    permMode: "ask",
    toolRules: [],
    maxTokens: "",
    reasoningEffort: "medium",
    memoryConsolidation: {
      enabled: true,
      interval: "",
      lastRunAt: null,
    },
    memoryControl: {
      injectionEnabled: true,
      autoWriteEnabled: true,
      autoExtract: "off",
    },
    memoryTuning: null,
    taskMode: "balanced",
    maxRounds: "",
    historyBudgetChars: "",
    historyTruncateTargetPct: "",
    reasoningEchoEnabled: "",
    inTurnContextEdit: "",
    subagentMaxTurns: "",
    subagentMaxToolCalls: "",
    searchMaxResults: "",
    maxToolOutputBytes: "",
    askBudget: "",
    ...over,
  };
}

/** 渲染 BotCoreSection，只关心档位选择器那部分。 */
function setup(config: SettingsConfig, saveConfig = vi.fn()) {
  const props = {
    botEnabled: true,
    toggleBot: vi.fn(),
    botBusy: false,
    botError: "",
    pyEnabled: false,
    togglePy: vi.fn(),
    pyBusy: false,
    servicesEnabled: false,
    toggleServices: vi.fn(),
    servicesBusy: false,
    exposeNetAllowed: false,
    toggleExposeNet: vi.fn(),
    exposeNetBusy: false,
    uiEnabled: false,
    toggleUi: vi.fn(),
    uiBusy: false,
    envErr: "",
    env: null,
    envBusy: false,
    checkEnv: vi.fn(),
    pyTimeoutView: undefined,
    config,
    setConfig: vi.fn(),
    renderSaveButton: () => <button type="button">保存</button>,
    paramMaxText: (k: string) => k,
    configBusy: false,
    saveConfig,
    paramViews: null,
  };
  render(
    <BotCoreSection
      {...props}
      config={config}
      saveConfig={saveConfig as never}
    />,
  );
  return { saveConfig };
}

describe("任务模式档位选择", () => {
  beforeEach(() => cleanup());

  it("三个档位都渲染出来", () => {
    setup(baseConfig());
    expect(screen.getByRole("button", { name: "轻量" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "均衡" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "深度" })).toBeTruthy();
  });

  it("点档位时落盘 taskMode 并清掉逐项覆盖", () => {
    const { saveConfig } = setup(
      baseConfig({
        taskMode: "balanced",
        maxRounds: "77",
        maxFunctionCalls: "88",
        subagentMaxTurns: "9",
        subagentMaxToolCalls: "7",
        historyBudgetChars: "200000",
        maxTokens: "4096",
        maxToolOutputBytes: "100000",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "深度" }));
    expect(saveConfig).toHaveBeenCalledTimes(1);
    const arg = saveConfig.mock.calls[0][0] as (c: SettingsConfig) => SettingsConfig;
    const next = arg(baseConfig());
    expect(next.taskMode).toBe("deep");
    // 覆盖必须被清空——留着旧覆盖，档位切换看起来「没生效」
    expect(next.maxRounds).toBe("");
    expect(next.maxFunctionCalls).toBe("");
    expect(next.subagentMaxTurns).toBe("");
    expect(next.subagentMaxToolCalls).toBe("");
    expect(next.historyBudgetChars).toBe("");
    expect(next.maxTokens).toBe("");
    expect(next.maxToolOutputBytes).toBe("");
  });

  it("当前档位带高亮标记（ring）", () => {
    setup(baseConfig({ taskMode: "light" }));
    const light = screen.getByRole("button", { name: "轻量" });
    const deep = screen.getByRole("button", { name: "深度" });
    expect(light.className).toContain("ring-1");
    expect(deep.className).not.toContain("ring-1");
  });
});
