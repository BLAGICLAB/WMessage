import { afterAll, beforeEach } from "vitest";

/**
 * jsdom 不实现 Element.scrollTo / scrollIntoView；渲染 ChatPanel 的 suite 在顶层
 * 调用一次本函数即可（文件级 polyfill：beforeEach 装、afterAll 删）。
 *
 * 为什么不按用例摘除：补丁是原型上的方法，摘掉之后到下一个用例装上之前有一段
 * 真空期——React 的 passive effect 若在这段里 flush，组件调 x.current?.scrollTo()
 * 会抛 TypeError（可选链只挡 ref 为空，挡不住方法缺失），红的是随机某个用例，
 * 与出错的那段代码毫无关系。文件内常驻、文件末清掉，空档就没有了。
 *
 * 注意：这里有意在 **jsdom 测试环境** 内补 Element.prototype（仓规「禁止改原生
 * 原型」针对运行时代码；jsdom 根本没实现这两方法，spyOn 无的可 spy，polyfill
 * 是 finding 认可的形态）。勿复制到 src 运行时代码。
 */
export function stubScrollNoop(): void {
  const patched: string[] = [];
  beforeEach(() => {
    for (const name of ["scrollTo", "scrollIntoView"] as const) {
      // truthy 判定（非 in）：属性存在但值为 undefined 的半成品 polyfill 也要覆盖
      if (Element.prototype[name]) continue;
      Object.defineProperty(Element.prototype, name, {
        configurable: true,
        writable: true,
        value: () => {},
      });
      patched.push(name);
    }
  });
  afterAll(() => {
    while (patched.length) {
      const name = patched.pop()!;
      delete (Element.prototype as unknown as Record<string, unknown>)[name];
    }
  });
}
