/** 消息身份生成（真源化契约 §8.2）：用户行出生即有 id，随 bot_chat 传给后端
 *  落库（client_msg_id 即行身份），bot-msg-upserted 事件回显同 id 对账。
 *
 *  优先 crypto.randomUUID（Tauri WKWebView / WebView2 均支持），异常环境退化
 *  时间戳+随机数。单点封装：出生点不直接摸 crypto，格式与后端 uuid v4 同为
 *  不透明串，两端各生成互不冲突。
 */
export function newMsgId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `msg-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}
