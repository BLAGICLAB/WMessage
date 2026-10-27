import { describe, it, expect } from "vitest";
import {
  keepExisting,
  mergeDeliverables,
  pickDeliverableShaped,
  type PathProbe,
} from "./deliverables";

describe("pickDeliverableShaped（交付物形态过滤）", () => {
  it("保留文档/表格/演示/图片/文本形态", () => {
    const paths = [
      "D:\\a\\绩效宣贯.pptx",
      "D:\\a\\设计说明.md",
      "D:\\a\\台账.xlsx",
      "D:\\a\\纪要.docx",
      "D:\\a\\截图.png",
      "D:\\a\\手册.pdf",
    ];
    expect(pickDeliverableShaped(paths)).toEqual(paths);
  });

  it("挡掉脚本形态与无扩展名路径", () => {
    expect(
      pickDeliverableShaped([
        "D:\\a\\生成脚本.py",
        "D:\\a\\run.js",
        "D:\\a\\build.sh",
        "D:\\a\\资料",
      ]),
    ).toEqual([]);
  });

  it("json/log 属白名单形态——交付物可以是一份数据导出", () => {
    expect(
      pickDeliverableShaped(["D:\\a\\参数.json", "D:\\a\\日志.log"]),
    ).toEqual(["D:\\a\\参数.json", "D:\\a\\日志.log"]);
  });

  it("大小写不敏感（.PPTX 也要认）", () => {
    expect(pickDeliverableShaped(["D:\\a\\A.PPTX"])).toEqual([
      "D:\\a\\A.PPTX",
    ]);
  });
});

describe("mergeDeliverables（两个来源合并）", () => {
  it("declared 在前、produced 追加在后", () => {
    const out = mergeDeliverables(
      ["D:\\a\\宣贯.pptx"],
      ["D:\\a\\生成脚本.py", "D:\\a\\台账.xlsx"],
    );
    expect(out.map((d) => d.path)).toEqual([
      "D:\\a\\宣贯.pptx",
      "D:\\a\\台账.xlsx",
    ]);
    expect(out[0].source).toBe("declared");
    expect(out[1].source).toBe("produced");
  });

  it("同路径只留一条，declared 优先（大小写归一后也算同一条）", () => {
    const out = mergeDeliverables(
      ["D:\\a\\宣贯.PPTX"],
      ["D:\\a\\宣贯.pptx", "D:\\a\\其他.pptx"],
    );
    expect(out).toHaveLength(2);
    expect(out[0].path).toBe("D:\\a\\宣贯.PPTX");
    expect(out[0].source).toBe("declared");
  });

  it("空串与纯空白不进清单", () => {
    expect(mergeDeliverables(["", "   "], [])).toEqual([]);
  });
});

describe("keepExisting（存在性收敛）", () => {
  const probes: PathProbe[] = [
    { path: "D:\\a\\宣贯.pptx", exists: true, isDir: false, size: 1024 },
    { path: "D:\\a\\缺失.md", exists: false, isDir: false, size: 0 },
  ];

  it("剔掉磁盘上没有的路径", () => {
    const items = mergeDeliverables(
      ["D:\\a\\宣贯.pptx", "D:\\a\\缺失.md"],
      [],
    );
    expect(keepExisting(items, probes).map((d) => d.path)).toEqual([
      "D:\\a\\宣贯.pptx",
    ]);
  });

  it("探测结果里没有的路径一并剔掉（探测失败不等于存在）", () => {
    const items = mergeDeliverables(["D:\\a\\未探测.md"], []);
    expect(keepExisting(items, probes)).toEqual([]);
  });
});
