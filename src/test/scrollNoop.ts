import { afterEach, beforeEach } from "vitest";

/**
 * jsdom 不实现 Element.scrollTo / scrollIntoView；渲染 ChatPanel / WidgetApp 的
 * suite 在顶层调用一次本函数即可（scoped polyfill：beforeEach 装、afterEach 只删
 * 本套件装的，不再全局常驻）。
 *
 * 注意：这里有意在 **jsdom 测试环境** 内临时补 Element.prototype（仓规「禁止改原生
 * 原型」针对运行时代码；jsdom 根本没实现这两方法，spyOn 无的可 spy，scoped polyfill
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
  afterEach(() => {
    while (patched.length) {
      const name = patched.pop()!;
      delete (Element.prototype as unknown as Record<string, unknown>)[name];
    }
  });
}
