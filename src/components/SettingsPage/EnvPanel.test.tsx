import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { EnvPanel } from "./EnvPanel";
import type { EnvCheck } from "./types";

/** 造一份探测结果：Python 装 1 缺 2（其中一个是硬依赖）、Node 装 1 缺 1、启动器装 1 缺 1 */
function sampleEnv(over: Partial<EnvCheck> = {}): EnvCheck {
  return {
    failed: false,
    python: {
      available: true,
      python: "python3",
      version: "Python 3.14.7",
      packages: [
        {
          name: "openpyxl",
          available: true,
          required: true,
          purpose: "Excel 读写",
        },
        {
          name: "pypdf",
          available: false,
          required: true,
          purpose: "PDF 文本层提取",
        },
        {
          name: "pandas",
          available: false,
          required: false,
          purpose: "数据表处理",
        },
      ],
      // 安装命令由后端给（平台相关），这里直接给成品
      install: "python3 -m pip install pypdf pandas",
    },
    node: {
      available: true,
      node: "/usr/local/bin/node",
      version: "v26.5.0",
      npm: "/usr/local/bin/npm",
      npmVersion: "11.17.0",
      packages: [
        {
          name: "pptxgenjs",
          available: true,
          purpose: "PPT 生成（ppt-craft-master / ppt-generator 技能依赖）",
        },
        {
          name: "@mermaid-js/mermaid-cli",
          available: false,
          purpose: "Mermaid 图渲染（流程图/时序图出图）",
        },
      ],
      install: "npm i -g @mermaid-js/mermaid-cli",
    },
    launchers: [
      {
        name: "npx",
        available: true,
        purpose: "MCP 服务器启动器（npm；随 node 一起装）",
        install: "brew install node",
      },
      {
        name: "docker",
        available: false,
        purpose: "容器运行时",
        install: "brew install --cask docker-desktop",
      },
    ],
    ...over,
  };
}

describe("EnvPanel 本机运行环境卡", () => {
  it("没检查过时提示点右侧检查", () => {
    render(
      <EnvPanel env={null} busy={false} error="" onCheck={async () => {}} />,
    );
    expect(screen.getByText("点击右侧检查")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "检查环境" })).toBeInTheDocument();
  });

  it("检查后摘要行报解释器与运行时版本", () => {
    render(
      <EnvPanel
        env={sampleEnv()}
        busy={false}
        error=""
        onCheck={async () => {}}
      />,
    );
    expect(
      screen.getByText("Python 3.14.7 · Node v26.5.0 · npm 11.17.0"),
    ).toBeInTheDocument();
  });

  it("探测失败报异常而不是显示成什么都没装", () => {
    render(
      <EnvPanel
        env={sampleEnv({ failed: true })}
        busy={false}
        error=""
        onCheck={async () => {}}
      />,
    );
    expect(screen.getByText(/探测异常/)).toBeInTheDocument();
    expect(screen.queryByText("Python 包")).not.toBeInTheDocument();
  });

  it("命令报错时显示失败原因", () => {
    render(
      <EnvPanel
        env={null}
        busy={false}
        error="命令超时"
        onCheck={async () => {}}
      />,
    );
    expect(screen.getByText("检查失败：命令超时")).toBeInTheDocument();
  });

  it("分组默认收起，展开后列出条目与安装命令", () => {
    render(
      <EnvPanel
        env={sampleEnv()}
        busy={false}
        error=""
        onCheck={async () => {}}
      />,
    );
    // 收起时看不到条目
    expect(screen.queryByText("openpyxl")).not.toBeInTheDocument();

    fireEvent.click(screen.getByText("Python 包"));
    expect(screen.getByText("openpyxl")).toBeInTheDocument();
    expect(screen.getByText("Excel 读写")).toBeInTheDocument();
    // 缺的两个包 → 给出 pip 安装命令
    expect(
      screen.getByText("安装缺失项：python3 -m pip install pypdf pandas"),
    ).toBeInTheDocument();
  });

  it("Node 组只收与 Python 组不重复的包，缺项给 npm 安装命令", () => {
    render(
      <EnvPanel
        env={sampleEnv()}
        busy={false}
        error=""
        onCheck={async () => {}}
      />,
    );
    expect(screen.getByText("Node 全局包")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Node 全局包"));
    expect(screen.getByText("pptxgenjs")).toBeInTheDocument();
    expect(
      screen.getByText("安装缺失项：npm i -g @mermaid-js/mermaid-cli"),
    ).toBeInTheDocument();
  });

  it("硬依赖缺失单独标出来", () => {
    render(
      <EnvPanel
        env={sampleEnv()}
        busy={false}
        error=""
        onCheck={async () => {}}
      />,
    );
    fireEvent.click(screen.getByText("Python 包"));
    // pypdf 是硬依赖、pandas 不是 → 只有 1 项算「必需缺失」
    expect(screen.getByText("1 项必需缺失")).toBeInTheDocument();
  });

  it("运行时组缺项逐项给安装命令，已装的不给", () => {
    render(
      <EnvPanel
        env={sampleEnv()}
        busy={false}
        error=""
        onCheck={async () => {}}
      />,
    );
    fireEvent.click(screen.getByText("运行时与启动器"));
    expect(
      screen.getByText("docker：brew install --cask docker-desktop"),
    ).toBeInTheDocument();
    // npx 已装 → 不给安装命令
    expect(screen.queryByText(/npx：brew/)).not.toBeInTheDocument();
  });

  it("该平台没给命令的缺项只报缺，不给命令", () => {
    const env = sampleEnv();
    render(
      <EnvPanel
        env={{
          ...env,
          launchers: [
            { name: "npx", available: true, purpose: "启动器", install: null },
            { name: "bun", available: false, purpose: "Bun 运行时", install: null },
          ],
        }}
        busy={false}
        error=""
        onCheck={async () => {}}
      />,
    );
    fireEvent.click(screen.getByText("运行时与启动器"));
    expect(screen.getByText("bun")).toBeInTheDocument();
    expect(screen.queryByText(/bun：/)).not.toBeInTheDocument();
  });

  it("都装齐了就不给安装命令", () => {
    const env = sampleEnv();
    render(
      <EnvPanel
        env={{
          ...env,
          python: {
            ...env.python,
            packages: [
              {
                name: "openpyxl",
                available: true,
                required: true,
                purpose: "Excel 读写",
              },
            ],
            install: null,
          },
        }}
        busy={false}
        error=""
        onCheck={async () => {}}
      />,
    );
    fireEvent.click(screen.getByText("Python 包"));
    expect(screen.queryByText(/安装缺失项/)).not.toBeInTheDocument();
  });

  it("点检查按钮回调一次", () => {
    let called = 0;
    render(
      <EnvPanel
        env={null}
        busy={false}
        error=""
        onCheck={async () => {
          called += 1;
        }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "检查环境" }));
    expect(called).toBe(1);
  });

  it("检查中按钮禁用并显示检查中", () => {
    render(
      <EnvPanel env={null} busy error="" onCheck={async () => {}} />,
    );
    const btn = screen.getByRole("button", { name: "检查中…" });
    expect(btn).toBeDisabled();
  });
});
