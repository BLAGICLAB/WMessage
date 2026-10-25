/** 聊天面板的「models」逻辑块（从 ChatPanel 主组件搬出）。
 *
 *  模型下拉：模型列表、当前协议与当前模型。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useRef, useState } from "react";
import type { ModelItem } from "./types";

export function useModelPicker() {
  const [modelMenuOpen, setModelMenuOpen] = useState(false);
  const [models, setModels] = useState<ModelItem[]>([]);
  const [apiProvider, setApiProvider] = useState<"openai" | "anthropic">(
    "openai",
  );
  const [activeModelId, setActiveModelId] = useState<string | null>(null);
  const modelBtnRef = useRef<HTMLButtonElement>(null);
  const modelDropdownRef = useRef<HTMLDivElement>(null);
  const [modelLabel, setModelLabel] = useState("…");

  return {
    activeModelId,
    apiProvider,
    modelBtnRef,
    modelDropdownRef,
    modelLabel,
    modelMenuOpen,
    models,
    setActiveModelId,
    setApiProvider,
    setModelLabel,
    setModelMenuOpen,
    setModels,
  };
}
