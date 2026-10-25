//! 页面解析面：HTML 标签抽取、实体解码与 Bing/百度结果块解析。

/// 开标签精确匹配：`<a` 这类前缀 `find` 会误中 `<abbr>`/`<address>`/`<pre>`
/// 等同前缀标签——开标签后一个字符必须是空白 / `>` / `/` 才算该标签
/// （abbr 前置时标题与链接来自不同 DOM 节点）。
/// 扫描全部命中取第一个合格位置。
fn find_tag_open(s: &str, pat: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = s[from..].find(pat) {
        let idx = from + i;
        match s[idx + pat.len()..].chars().next() {
            None => return Some(idx),
            Some(c) if c.is_whitespace() || c == '>' || c == '/' => return Some(idx),
            _ => from = idx + pat.len(),
        }
    }
    None
}

/// 取第一个 `<open ...> ... </close>` 的中间内容
pub(super) fn first_between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = find_tag_open(s, open)?;
    let tail = &s[a..];
    let gt = tail.find('>')?;
    let start = gt + 1;
    let b = tail[start..].find(close)?;
    Some(&tail[start..start + b])
}

/// 取第一个 `<open ...>` 完整开标签
pub(super) fn first_open_tag<'a>(s: &'a str, open: &str) -> Option<&'a str> {
    let a = find_tag_open(s, open)?;
    let gt = s[a..].find('>')?;
    Some(&s[a..a + gt + 1])
}

/// 从 `<a ... href="X" ...>` 里提取 href
pub(super) fn href_from_tag(tag: &str) -> Option<String> {
    let h = tag.find("href=")?;
    let after = &tag[h + 5..];
    let q = after.chars().next()?;
    let inner = &after[q.len_utf8()..];
    let end = inner.find(q)?;
    Some(inner[..end].to_string())
}

/// 去 HTML 标签 + 实体解码（含数字实体 &#NNN; / &#xNN;）
pub(super) fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out).trim().to_string()
}

/// 解码命名实体 + 数字实体（&#NNN; / &#xHH;）
fn decode_entities(s: &str) -> String {
    let s = s
        .replace("&nbsp;", " ")
        .replace("&ensp;", " ")
        .replace("&emsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&rsquo;", "'")
        .replace("&lsquo;", "'")
        .replace("&hellip;", "…")
        .replace("&middot;", "·")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&ldquo;", "“")
        .replace("&rdquo;", "”");
    // 数字实体：&#NNN; 或 &#xHH;
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(pos) = rest.find("&#") {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos + 2..];
        let (digits, consumed) =
            if let Some(hex) = tail.strip_prefix('x').or_else(|| tail.strip_prefix('X')) {
                let end = hex.find(';').unwrap_or(hex.len());
                (&hex[..end], 1 + end + usize::from(end < hex.len()))
            } else {
                let end = tail.find(';').unwrap_or(tail.len());
                (&tail[..end], end + usize::from(end < tail.len()))
            };
        let radix = if tail.starts_with('x') || tail.starts_with('X') {
            16
        } else {
            10
        };
        if let Ok(n) = u32::from_str_radix(digits, radix) {
            if let Some(c) = char::from_u32(n) {
                out.push(c);
            }
        }
        rest = &tail[consumed.min(tail.len())..];
        if !tail.contains(';') {
            break;
        }
    }
    out.push_str(rest);
    out
}

// 正文提取
// 先抽正文主块再转换：
// 去 script/style/nav/footer 等噪声块 → article/main 语义标签 → 语义 class/id 的最大 div → 兜底全文。

/// 大小写不敏感的子串查找（needle 为 ASCII 标签；返回原串字节索引，不做全串 lowercase 避免变长错位）
fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack.as_bytes()[from..]
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
        .map(|i| from + i)
}

/// 从 start（`<tag` 的位置）起按嵌套深度找匹配闭合标签，返回块字节区间 [start, end)
fn tag_block_span(html: &str, start: usize, tag: &str) -> Option<(usize, usize)> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut depth = 0usize;
    let mut cur = start;
    while cur < html.len() {
        let next_open = find_ci(html, &open, cur);
        let next_close = find_ci(html, &close, cur);
        match (next_open, next_close) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                cur = o + open.len();
            }
            (_, Some(c)) => {
                depth = depth.saturating_sub(1);
                cur = c + close.len();
                if depth == 0 {
                    return Some((start, cur));
                }
            }
            _ => return None,
        }
    }
    None
}

/// 删除指定标签的完整块（含嵌套同名标签）：去 script/style/nav/footer 等噪声。
/// 未闭合的块只丢开标签本身（自闭合写法如 <iframe/> 不至于吞掉整页残余）
pub(super) fn remove_tag_blocks(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let mut out = String::with_capacity(html.len());
    let mut rest = 0usize;
    while let Some(pos) = find_ci(html, &open, rest) {
        out.push_str(&html[rest..pos]);
        rest = match tag_block_span(html, pos, tag) {
            Some((_, end)) => end,
            None => pos + open.len(),
        };
    }
    out.push_str(&html[rest.min(html.len())..]);
    out
}

/// 提取正文主块：去噪声块 → article/main 标签 → 语义 class/id 的最大 div → 兜底整页
pub(super) fn extract_main_content(html: &str) -> String {
    let mut cleaned = html.to_string();
    for tag in [
        "script", "style", "noscript", "iframe", "form", "nav", "footer", "header", "aside",
    ] {
        cleaned = remove_tag_blocks(&cleaned, tag);
    }
    // HTML 注释
    while let Some(s) = cleaned.find("<!--") {
        match cleaned[s..].find("-->") {
            Some(e) => cleaned.replace_range(s..s + e + 3, ""),
            None => break,
        }
    }
    for tag in ["article", "main"] {
        if let Some(pos) = find_ci(&cleaned, &format!("<{tag}"), 0) {
            if let Some((s, e)) = tag_block_span(&cleaned, pos, tag) {
                if strip_tags(&cleaned[s..e]).chars().count() >= 200 {
                    return cleaned[s..e].to_string();
                }
            }
        }
    }
    // 语义 div：class/id 含 article|content|post|entry|main（排除 comment），取可见文本最多者
    let mut best: Option<(usize, usize, usize)> = None;
    let mut cur = 0;
    while let Some(pos) = find_ci(&cleaned, "<div", cur) {
        let Some(gt) = cleaned[pos..].find('>') else {
            break;
        };
        let attrs = cleaned[pos..pos + gt].to_lowercase();
        cur = pos + 4;
        let hit = ["article", "content", "post", "entry", "main"]
            .iter()
            .any(|k| attrs.contains(k));
        if !hit || attrs.contains("comment") {
            continue;
        }
        if let Some((s, e)) = tag_block_span(&cleaned, pos, "div") {
            let text_len = strip_tags(&cleaned[s..e]).chars().count();
            if text_len > best.map(|b| b.0).unwrap_or(0) {
                best = Some((text_len, s, e));
            }
        }
    }
    if let Some((len, s, e)) = best {
        if len >= 200 {
            return cleaned[s..e].to_string();
        }
    }
    cleaned
}

/// 解析 Bing 结果页：`<li class="b_algo">` 块 → (标题, 链接, 摘要)
pub(super) fn parse_bing(html: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("<li class=\"b_algo\"") {
        rest = &rest[pos..];
        let end = rest
            .find("</li>")
            .map(|i| i + "</li>".len())
            .unwrap_or(rest.len());
        let block = &rest[..end];
        rest = &rest[end..];
        let Some(h2) = first_between(block, "<h2", "</h2>") else {
            continue;
        };
        let Some(a_tag) = first_open_tag(h2, "<a") else {
            continue;
        };
        let Some(a_text) = first_between(h2, "<a", "</a>") else {
            continue;
        };
        let title = strip_tags(a_text);
        if title.is_empty() {
            continue;
        }
        let link = href_from_tag(a_tag).unwrap_or_default();
        let snip = first_between(block, "<p", "</p>")
            .map(strip_tags)
            .unwrap_or_default();
        out.push((title, link, snip));
    }
    out
}

/// 解析百度结果页：逐个 h3 里带 /link?url 的标题 → (标题, 跳转链接, 摘要)
pub(super) fn parse_baidu(html: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("<h3") {
        let seg = &rest[pos..];
        let Some(h3_close) = seg.find("</h3>") else {
            break;
        };
        let h3end = h3_close + "</h3>".len();
        let h3 = &seg[..h3end];
        let Some(a_tag) = first_open_tag(h3, "<a") else {
            rest = &seg[h3end..];
            continue;
        };
        let Some(link) = href_from_tag(a_tag) else {
            rest = &seg[h3end..];
            continue;
        };
        if !link.contains("baidu.com/link") {
            rest = &seg[h3end..];
            continue;
        }
        let Some(a_text) = first_between(h3, "<a", "</a>") else {
            rest = &seg[h3end..];
            continue;
        };
        let title = strip_tags(a_text);
        if title.is_empty() {
            rest = &seg[h3end..];
            continue;
        }
        // 摘要：标题后面第一个有实质内容的 span（滤掉 JSON 垃圾）
        let mut tail_end = seg.len().min(h3end + 6000);
        while !seg.is_char_boundary(tail_end) {
            tail_end += 1;
        }
        let tail = &seg[h3end..tail_end];
        let snip = extract_baidu_snippet(tail);
        out.push((title, link, snip));
        rest = &seg[h3end..];
    }
    out
}

/// 从 h3 之后的文本里抽摘要：找第一个 ≥12 字且不含 JSON 垃圾的 span 文本，截 200 字
fn extract_baidu_snippet(tail: &str) -> String {
    let mut s = tail;
    while let Some(p) = s.find("<span") {
        let after = &s[p..];
        let Some(gt) = after.find('>') else {
            break;
        };
        let start = gt + 1;
        let Some(close) = after[start..].find("</span>") else {
            break;
        };
        let text = strip_tags(&after[start..start + close]);
        if !text.is_empty()
            && text.chars().count() >= 12
            && !text.contains("clamp")
            && !text.contains("isSingleLine")
            && !text.contains("sutil")
        {
            return text.chars().take(200).collect();
        }
        s = &after[start + close + "</span>".len()..];
    }
    String::new()
}
