// 测试专用最小 node:fs 类型声明（项目未装 @types/node，仅测试读源码文件用）
declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf-8"): string;
}
