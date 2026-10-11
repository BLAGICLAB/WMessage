// 必应每日一图（首页小部件）：后端当天缓存，命令见 src-tauri/src/bing.rs。

import { invoke } from "@tauri-apps/api/core";

export interface BingDaily {
  title: string;
  copyright: string;
  /** data:image/jpeg;base64（应用未开 asset 协议，走 data URL 展示） */
  dataUrl: string;
  /** 图片原始地址（点击外跳查看大图） */
  imageUrl: string;
  date: string;
}

export function fetchBingDaily(): Promise<BingDaily> {
  return invoke<BingDaily>("bing_daily");
}
