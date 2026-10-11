//! 必应每日一图（首页小部件数据源）。
//!
//! 每天拉取一次 cn.bing.com 官方 API 的当日壁纸（标题/版权/图），图片与元数据
//! 按日期落 `{data_dir}/runtime/bing/`，当天内命中缓存不再出网；缓存目录保留
//! 最近 7 天，过期文件随每次拉取清理。图片以 base64 data URL 返回——应用未开
//! asset 协议，data URL 是免配置跨平台的展示通道（单张 1920x1080 jpg 约
//! 150~400KB，IPC 一次在可接受范围）。
//!
//! 写入顺序 = 图片先、元数据后（元数据即提交标记）：中断留下的孤儿图片会在
//! 下次拉取时被覆盖/清理，不会被当成有效缓存。

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::error::CommandResult;

const BING_API: &str = "https://cn.bing.com/HPImageArchive.aspx?format=js&idx=0&n=1";
const BING_HOST: &str = "https://cn.bing.com";
/// 保留天数（含当天）：日期前缀老于 7 天的缓存文件在每次拉取时清理
const KEEP_DAYS: i64 = 7;
/// 图片下载上限：官方 1920x1080 jpg 通常 150~400KB，8MB 是宽松护栏
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BingDaily {
    pub title: String,
    pub copyright: String,
    pub data_url: String,
    /// 图片原始地址（前端点击外跳查看大图）
    pub image_url: String,
    pub date: String,
}

#[derive(Deserialize)]
struct BingResp {
    images: Vec<BingImage>,
}

#[derive(Deserialize)]
struct BingImage {
    title: String,
    copyright: String,
    url: String,
}

#[tauri::command]
pub async fn bing_daily(app: AppHandle) -> CommandResult<BingDaily> {
    let dir = crate::paths::runtime_dir(&app).join("bing");
    std::fs::create_dir_all(&dir)?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    if let Some(hit) = read_cache(&dir, &today) {
        return Ok(hit);
    }
    let fresh = fetch_today(&today).await?;
    write_cache(&dir, &today, &fresh)?;
    prune_old(&dir, &today);
    Ok(fresh)
}

/// 命中缓存读元数据 + 图片；任一缺失或元数据损坏都视为未命中（重新拉取）。
/// 读取失败不是错误——缓存的意义就是「拿不到就走网络」。
fn read_cache(dir: &std::path::Path, today: &str) -> Option<BingDaily> {
    let meta_path = dir.join(format!("{today}.json"));
    let img_path = dir.join(format!("{today}.jpg"));
    let meta = std::fs::read_to_string(&meta_path).ok()?;
    let meta: CacheMeta = match serde_json::from_str(&meta) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[bing] 缓存元数据损坏，重新拉取：{e}");
            return None;
        }
    };
    let img = std::fs::read(&img_path).ok()?;
    Some(BingDaily {
        data_url: to_data_url(&img),
        image_url: meta.image_url,
        title: meta.title,
        copyright: meta.copyright,
        date: today.to_string(),
    })
}

#[derive(Serialize, Deserialize)]
struct CacheMeta {
    title: String,
    copyright: String,
    image_url: String,
}

/// 图片先写、元数据后写（元数据 = 提交标记），中断不会产生半套有效缓存
fn write_cache(dir: &std::path::Path, today: &str, daily: &BingDaily) -> CommandResult<()> {
    let img = decode_data_url(&daily.data_url)?;
    std::fs::write(dir.join(format!("{today}.jpg")), img)?;
    let meta = CacheMeta {
        title: daily.title.clone(),
        copyright: daily.copyright.clone(),
        image_url: daily.image_url.clone(),
    };
    std::fs::write(
        dir.join(format!("{today}.json")),
        serde_json::to_vec(&meta)?,
    )?;
    Ok(())
}

/// 清理日期前缀早于 KEEP_DAYS 窗口的缓存文件（保留今天起往前 7 天）；
/// 单个删除失败不影响其余
fn prune_old(dir: &std::path::Path, today: &str) {
    let Some(cutoff) = minus_days(today, KEEP_DAYS - 1) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if file_date(name).is_some_and(|d| d < cutoff.as_str()) {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                eprintln!("[bing] 清理过期缓存失败 {name}: {e}");
            }
        }
    }
}

async fn fetch_today(today: &str) -> CommandResult<BingDaily> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| crate::error::CommandError::from(format!("HTTP client 构建失败：{e}")))?;
    let resp: BingResp = client
        .get(BING_API)
        .header(reqwest::header::USER_AGENT, "wmessage/0.1")
        .send()
        .await
        .map_err(req_err("必应每日一图接口请求失败"))?
        .error_for_status()
        .map_err(req_err("必应每日一图接口返回错误状态"))?
        .json()
        .await
        .map_err(req_err("必应每日一图接口响应解析失败"))?;
    let img = resp
        .images
        .first()
        .ok_or_else(|| crate::error::CommandError::from("必应接口 images 列表为空"))?;
    let image_url = if img.url.starts_with("http") {
        img.url.clone()
    } else {
        format!("{BING_HOST}{}", img.url)
    };
    let bytes = client
        .get(&image_url)
        .header(reqwest::header::USER_AGENT, "wmessage/0.1")
        .send()
        .await
        .map_err(req_err("壁纸图片下载失败"))?
        .error_for_status()
        .map_err(req_err("壁纸图片下载返回错误状态"))?
        .bytes()
        .await
        .map_err(req_err("壁纸图片读取失败"))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(crate::error::CommandError::from(format!(
            "壁纸图片超过大小上限（{} > {MAX_IMAGE_BYTES}）",
            bytes.len()
        )));
    }
    Ok(BingDaily {
        title: img.title.clone(),
        copyright: img.copyright.clone(),
        data_url: to_data_url(&bytes),
        image_url,
        date: today.to_string(),
    })
}

fn req_err(prefix: &'static str) -> impl Fn(reqwest::Error) -> crate::error::CommandError {
    move |e| crate::error::CommandError::from(format!("{prefix}：{e}"))
}

fn to_data_url(jpg: &[u8]) -> String {
    format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpg)
    )
}

fn decode_data_url(data_url: &str) -> CommandResult<Vec<u8>> {
    const PREFIX: &str = "data:image/jpeg;base64,";
    let b64 = data_url
        .strip_prefix(PREFIX)
        .ok_or_else(|| crate::error::CommandError::from("data URL 前缀异常"))?;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| crate::error::CommandError::from(format!("缓存图片 base64 解码失败：{e}")))
}

/// 文件名前 10 位形如 YYYY-MM-DD 才视为日期缓存文件
fn file_date(name: &str) -> Option<&str> {
    let head = name.get(..10)?;
    let ok = head.as_bytes().iter().enumerate().all(|(i, b)| match i {
        4 | 7 => *b == b'-',
        _ => b.is_ascii_digit(),
    });
    ok.then_some(head)
}

fn minus_days(today: &str, days: i64) -> Option<String> {
    chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d")
        .ok()?
        .checked_sub_signed(chrono::Duration::days(days))
        .map(|d| d.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wm-bing-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const TODAY: &str = "2026-10-11";

    fn sample(img: &[u8]) -> BingDaily {
        BingDaily {
            title: "秋冬菜单静物".into(),
            copyright: "© 摄影师（Bing 中国）".into(),
            data_url: to_data_url(img),
            image_url: "https://cn.bing.com/th?id=OHR.X".into(),
            date: TODAY.into(),
        }
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tmp_dir("roundtrip");
        let daily = sample(b"jpegbytes");
        write_cache(&dir, TODAY, &daily).unwrap();
        let hit = read_cache(&dir, TODAY).unwrap();
        assert_eq!(hit.title, "秋冬菜单静物");
        assert_eq!(hit.data_url, daily.data_url);
        assert_eq!(hit.image_url, daily.image_url);
    }

    #[test]
    fn missing_img_is_miss() {
        let dir = tmp_dir("miss-img");
        let daily = sample(b"jpegbytes");
        write_cache(&dir, TODAY, &daily).unwrap();
        std::fs::remove_file(dir.join(format!("{TODAY}.jpg"))).unwrap();
        assert!(read_cache(&dir, TODAY).is_none());
    }

    #[test]
    fn corrupt_meta_is_miss() {
        let dir = tmp_dir("corrupt");
        std::fs::write(dir.join(format!("{TODAY}.json")), b"not-json").unwrap();
        std::fs::write(dir.join(format!("{TODAY}.jpg")), b"jpegbytes").unwrap();
        assert!(read_cache(&dir, TODAY).is_none());
    }

    #[test]
    fn prune_keeps_recent_and_today() {
        let dir = tmp_dir("prune");
        let daily = sample(b"jpegbytes");
        write_cache(&dir, TODAY, &daily).unwrap();
        for d in ["2026-10-03", "2026-10-04", "2026-10-05"] {
            write_cache(&dir, d, &daily).unwrap();
        }
        prune_old(&dir, TODAY);
        assert!(!dir.join("2026-10-03.jpg").exists());
        assert!(!dir.join("2026-10-03.json").exists());
        assert!(!dir.join("2026-10-04.jpg").exists());
        assert!(dir.join("2026-10-05.jpg").exists());
        assert!(dir.join(format!("{TODAY}.jpg")).exists());
        assert!(dir.join(format!("{TODAY}.json")).exists());
    }

    #[test]
    fn file_date_requires_shape() {
        assert_eq!(file_date("2026-10-11.jpg"), Some("2026-10-11"));
        assert_eq!(file_date("not-a-date.jpg"), None);
        assert_eq!(file_date("abc"), None);
    }

    #[test]
    fn data_url_roundtrip() {
        let url = to_data_url(b"abc");
        assert_eq!(decode_data_url(&url).unwrap(), b"abc");
    }
}
