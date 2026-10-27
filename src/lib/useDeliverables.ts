import { useEffect, useRef, useState } from "react";
import { keepExisting, probePaths, type Deliverable } from "./deliverables";

/** 探测结果：交付物清单 + 「探完了没」 */
export type ProbedDeliverables = {
  /** 探测通过的交付物（磁盘上真在） */
  items: Deliverable[];
  /**
   * 探测是否已回来。
   *
   * 这个位不能省：探测是异步的，没回来之前清单是空的——空清单这时既可能
   * 是「真没交付物」，也可能是「还没探完」。两者在 UI 上要分开：前者才
   * 能说「这轮没产出」，后者只能先不渲染。
   */
  ready: boolean;
};

/**
 * 交付物清单的异步收敛（存在性校验）。
 *
 * 交付物是**磁盘上的实体**：路径在、文件不在，挂上去就是个点不开的条目。
 * 存在性判定要走 IPC（前端没有 stat 能力），所以清单分两段——
 * 探测没回来之前返回空列表：宁可晚一拍出现，也别先挂一个打不开的。
 *
 * enabled=false（消息还在流式输出）时不探：正文每个 token 都在变，
 * 探一次就等于每个 token 触发一次 IPC。
 *
 * 重新探测的触发条件是**路径集合变了**，不是「清单数组换了身份」：会话级
 * 清单每来一个 token 就重算一次数组，按身份判会每个 token 探一遍全量路径。
 */
export function useProbedDeliverables(
  items: Deliverable[],
  enabled: boolean,
): ProbedDeliverables {
  /** 探测结果按清单键打标：清单换了就返回空，不闪上一份的结果 */
  const key = items.map((d) => d.path).join("\u0000");
  /** 清单与路径的最新值：探测回调里要用，但不该进依赖数组 */
  const itemsRef = useRef(items);
  itemsRef.current = items;
  const [probed, setProbed] = useState<{ key: string; items: Deliverable[] }>({
    key: "",
    items: [],
  });
  useEffect(() => {
    if (!enabled || itemsRef.current.length === 0) return;
    // 快照：探测回来时用**这一批**的清单做收敛，不用最新的——
    // 最新的可能已经是另一批路径了（key 已变），拿它收敛就串了
    const snapshot = itemsRef.current;
    let live = true;
    probePaths(snapshot.map((d) => d.path))
      .then((probes) => {
        if (live) setProbed({ key, items: keepExisting(snapshot, probes) });
      })
      .catch(() => {
        // 探测失败按「没有可确认存在的交付物」处理：不挂可能打不开的条目
        if (live) setProbed({ key, items: [] });
      });
    return () => {
      live = false;
    };
  }, [key, enabled]);
  if (!enabled) return { items: [], ready: false };
  if (items.length === 0) return { items: [], ready: true };
  return {
    items: probed.key === key ? probed.items : [],
    ready: probed.key === key,
  };
}
