/// <reference types="node" />
// GuardKeywordsPanel（设置页「机器人」分区 · 工具调用触发器关键词卡）单测。
//
// 盯住三件容易出事的：① 当前生效词表回填到输入框；② 改完保存走
// bot_set_config 且**带上** guardClaimKeywords（整体替换写，漏传=清空）；
// ③ 空列表会显红警告（这是用户唯一能看见「关掉触发器」后果的地方）。

import { useState } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { GuardKeywordsPanel } from "./GuardKeywordsPanel";

const DEFAULTS = ["执行完毕", "跑完了", "已执行", "实测约"];

/** 受控组件：真实用法里 value 由父组件 state 持有，这里用 wrapper 复现，
 *  否则 fireEvent.change 只会打到 onChange mock，界面不会跟着变。 */
function setup(over?: Partial<React.ComponentProps<typeof GuardKeywordsPanel>>) {
  const onChange = vi.fn();
  const onSave = vi.fn();
  const props = {
    value: DEFAULTS.join("\n"),
    defaults: DEFAULTS,
    busy: false,
    ...over,
  };
  function Harness() {
    const [value, setValue] = useState(props.value);
    return (
      <GuardKeywordsPanel
        {...props}
        value={value}
        onChange={(next) => {
          onChange(next);
          setValue(next);
        }}
        onSave={onSave}
      />
    );
  }
  render(<Harness />);
  return { onChange, onSave };
}

const textarea = () =>
  screen.getByPlaceholderText(/执行完毕/) as HTMLTextAreaElement;

describe("GuardKeywordsPanel", () => {
  beforeEach(cleanup);

  it("回填当前生效词表，并标出与内置默认一致", () => {
    setup();
    expect(textarea().value).toBe(DEFAULTS.join("\n"));
    expect(screen.getByText(/当前生效（4 个）/)).toBeInTheDocument();
    expect(screen.getByText("与内置默认一致。")).toBeInTheDocument();
  });

  it("删词后偏离内置默认，保存回调触发", () => {
    const { onChange, onSave } = setup();
    fireEvent.change(textarea(), { target: { value: "执行完毕\n搞定" } });
    expect(onChange).toHaveBeenCalledWith("执行完毕\n搞定");
    expect(screen.getByText("已偏离内置默认。")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    expect(onSave).toHaveBeenCalledTimes(1);
  });

  it("清空列表时警告：关掉触发器只剩两档核查", () => {
    setup({ value: "" });
    expect(screen.getByText(/列表为空 = 关闭这道触发器/)).toBeInTheDocument();
    expect(screen.getByText(/变更未调工具/)).toBeInTheDocument();
  });

  it("恢复默认只回填输入框、不直接保存", () => {
    const { onChange, onSave } = setup({ value: "我自己写的词" });
    fireEvent.click(screen.getByRole("button", { name: /恢复默认/ }));
    expect(onChange).toHaveBeenCalledWith(DEFAULTS.join("\n"));
    expect(onSave).not.toHaveBeenCalled();
  });

  it("busy 时保存按钮禁用", () => {
    setup({ busy: true });
    expect(screen.getByRole("button", { name: "保存中…" })).toBeDisabled();
  });
});
