import { useMemo, useRef } from "react";

/** 异步竞态令牌：「只认最新一拍」——慢的旧响应晚到时 isCurrent(seq) 已为假，
 *  调用方丢弃过期结果（防旧数据覆盖新状态）。
 *  next() 领新号并作废此前所有在途号；只作废不领号的场景（配置变更等）
 *  直接调 next() 不接返回值。 */
export function useRaceSeq(): {
  next: () => number;
  isCurrent: (seq: number) => boolean;
} {
  const seqRef = useRef(0);
  return useMemo(
    () => ({
      next: () => ++seqRef.current,
      isCurrent: (seq: number) => seq === seqRef.current,
    }),
    [],
  );
}
