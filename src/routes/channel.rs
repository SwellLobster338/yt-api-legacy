use actix_web::{web, HttpRequest, HttpResponse, Responder};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use urlencoding;
use utoipa::ToSchema;

fn base_url(req: &HttpRequest, config: &crate::config::Config) -> String {
    if !config.server.main_url.is_empty() {
        return config.server.main_url.clone();
    }
    let info = req.connection_info();
    let scheme = info.scheme();
    let host = info.host();
    format!("{}://{}/", scheme, host.trim_end_matches('/'))
}

fn parse_number(text: &str) -> String {
    let lower_text = text.trim().to_lowercase();
    let mut multiplier = 1.0;
    let clean_text = if lower_text.contains('k') {
        multiplier = 1000.0;
        lower_text.replace('k', "")
    } else if lower_text.contains('m') {
        multiplier = 1000000.0;
        lower_text.replace('m', "")
    } else if lower_text.contains('b') {
        multiplier = 1000000000.0;
        lower_text.replace('b', "")
    } else {
        lower_text
    };
    
    // Extract digits and decimal points
    let num_str: String = clean_text.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    
    match num_str.parse::<f64>() {
        Ok(num) => ((num * multiplier) as u64).to_string(),
        Err(_) => "0".to_string(),
    }
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ChannelInfo {
    pub title: String,
    pub description: String,
    pub thumbnail: String,
    pub banner: String,
    pub subscriber_count: String,
    pub video_count: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ChannelVideo {
    pub title: String,
    pub author: String,
    pub video_id: String,
    pub thumbnail: String,
    pub channel_thumbnail: String,
    pub views: String,
    pub published_at: String,
    pub duration: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ChannelVideosResponse {
    pub channel_info: ChannelInfo,
    pub videos: Vec<ChannelVideo>,
}

#[utoipa::path(
    get,
    path = "/get_author_videos.php",
    params(
        ("author" = String, Query, description = "Channel username/search query"),
        ("count" = Option<i32>, Query, description = "Number of videos to return (default: 50)")
    ),
    responses(
        (status = 200, description = "Videos for the author", body = ChannelVideosResponse),
        (status = 400, description = "Missing author parameter")
    )
)]
pub async fn get_author_videos(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    let base = base_url(&req, config);
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let author_raw = match query_params.get("author") {
        Some(a) => a.clone(),
        None => return HttpResponse::BadRequest().json(serde_json::json!({"error": "Author parameter is required"})),
    };

    // --- ИСПРАВЛЕНИЕ: Декодируем дважды, чтобы убрать %25E6... и получить чистые иероглифы ---
    let decoded_once = urlencoding::decode(&author_raw).unwrap_or(std::borrow::Cow::Borrowed(&author_raw)).to_string();
    let author = urlencoding::decode(&decoded_once).unwrap_or(std::borrow::Cow::Borrowed(&decoded_once)).to_string();

    let count: i32 = query_params
        .get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(config.video.default_count as i32);

    let innertube_key = match config.get_innertube_key() {
        Some(key) => key,
        None => return HttpResponse::InternalServerError().json(serde_json::json!({"error": "Missing innertube_key in config.yml"})),
    };

    let client = Client::new();

    let channel_id = if author.starts_with("UC") && author.len() == 24 {
        Some(author.clone())
    } else {
        resolve_handle_to_channel_id(&author, &client, &innertube_key, &base).await
    };

    let channel_id = match channel_id {
        Some(id) => id,
        None => return HttpResponse::BadRequest().json(serde_json::json!({"error": "Channel not found"})),
    };

    get_author_videos_by_id_internal(&channel_id, count, config, &base).await
}

#[utoipa::path(
    get,
    path = "/get_author_videos_by_id.php",
    params(
        ("channel_id" = String, Query, description = "YouTube channel ID"),
        ("count" = Option<i32>, Query, description = "Number of videos to return (default: 50)")
    ),
    responses(
        (status = 200, description = "Videos for channel", body = ChannelVideosResponse),
        (status = 400, description = "Missing channel_id parameter")
    )
)]
pub async fn get_author_videos_by_id(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    let base = base_url(&req, config);
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let channel_id = match query_params.get("channel_id") {
        Some(id) => id.clone(),
        None => return HttpResponse::BadRequest().json(serde_json::json!({"error": "Channel ID parameter is required"})),
    };

    let count: i32 = query_params
        .get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(config.video.default_count as i32);

    get_author_videos_by_id_internal(&channel_id, count, config, &base).await
}

async fn get_author_videos_by_id_internal(
    channel_id: &str,
    count: i32,
    config: &crate::config::Config,
    base: &str,
) -> HttpResponse {
    let innertube_key = match config.get_innertube_key() {
        Some(key) => key,
        None => return HttpResponse::InternalServerError().json(serde_json::json!({"error": "Missing innertube_key"})),
    };

    let (videos, channel_info) = fetch_channel_videos_inner_tube(channel_id, count, &innertube_key, base).await;

    let response = ChannelVideosResponse { channel_info, videos };
    HttpResponse::Ok().json(response)
}

async fn resolve_handle_to_channel_id(handle: &str, client: &Client, innertube_key: &str, _base: &str) -> Option<String> {
    let clean_handle = handle.trim().trim_start_matches('@');
    
    // ИСПРАВЛЕНИЕ: Был пустой URL, вставил правильный эндопоинт resolve_url
    let url = format!("https://www.youtube.com/youtubei/v1/navigation/resolve_url?key={}", innertube_key);
    
    let context = serde_json::json!({
        "client": {
            "clientName": "WEB",
            "clientVersion": "2.20260220.00.00",
            "hl": "en",
            "gl": "US"
        }
    });
    
    // В InnerTube не нужно энкодить URL здесь, он справляется с Unicode сам
    let payload = serde_json::json!({
        "context": context,
        "url": format!("https://www.youtube.com/@{}", clean_handle),
    });
    
    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
    {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(data) => {
                data.get("endpoint")
                    .and_then(|endpoint| endpoint.get("browseEndpoint"))
                    .and_then(|browse_endpoint| browse_endpoint.get("browseId"))
                    .and_then(|browse_id| browse_id.as_str())
                    .map(|s| s.to_string())
            },
            Err(_) => None,
        },
        Err(_) => None,
    }
}

async fn fetch_channel_videos_inner_tube(
    channel_id: &str,
    count: i32,
    innertube_key: &str,
    base: &str,
) -> (Vec<ChannelVideo>, ChannelInfo) {
    let client = Client::new();
    
    let url = format!("https://www.youtube.com/youtubei/v1/browse?key={}", innertube_key);
    
    let context = serde_json::json!({
        "client": {
            "clientName": "WEB",
            "clientVersion": "2.20260220.00.00",
            "hl": "en",
            "gl": "US"
        }
    });
    
    let payload = serde_json::json!({
        "context": context,
        "browseId": channel_id,
    });
    
    let response = match client.post(&url).header("Content-Type", "application/json").json(&payload).send().await {
        Ok(resp) => resp,
        Err(_) => return (Vec::new(), ChannelInfo {
            title: "Unknown".to_string(), description: "".to_string(), thumbnail: "".to_string(), banner: "".to_string(), subscriber_count: "0".to_string(), video_count: "0".to_string(),
        }),
    };
    
    let data: serde_json::Value = match response.json().await {
        Ok(json) => json,
        Err(_) => return (Vec::new(), ChannelInfo {
            title: "Unknown".to_string(), description: "".to_string(), thumbnail: "".to_string(), banner: "".to_string(), subscriber_count: "0".to_string(), video_count: "0".to_string(),
        }),
    };
    
    let channel_info = extract_channel_info(&data, base, channel_id).await;
    
    let tabs_array: Vec<serde_json::Value> = data.pointer("/contents/twoColumnBrowseResultsRenderer/tabs")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    
    let mut videos_content = None;
    for tab in &tabs_array {
        if let Some(tr) = tab.get("tabRenderer") {
            if let Some(title) = tr.get("title").and_then(|t| t.as_str()) {
                if title == "Videos" {
                    videos_content = tr.get("content");
                    break;
                }
            }
        }
    }
    
    if videos_content.is_none() {
        for tab in &tabs_array {
            if let Some(tr) = tab.get("tabRenderer") {
                if tr.get("selected").and_then(|s| s.as_bool()).unwrap_or(false) {
                    videos_content = tr.get("content");
                    break;
                }
            }
        }
    }
    
    let mut videos = Vec::new();
    let base_trimmed = base.trim_end_matches('/');
    
    // --- ИСПОЛЬЗУЕМ УМНЫЙ РЕКУРСИВНЫЙ ПАРСЕР ---
    if let Some(content) = videos_content {
        let mut seen = HashSet::new();
        extract_videos_recursively(content, &mut videos, &channel_info, base_trimmed, &mut seen);
    }
    
    videos.truncate(count as usize);
    (videos, channel_info)
}

// --- УМНЫЙ РЕКУРСИВНЫЙ ПАРСЕР (заменяет старые функции) ---
fn extract_videos_recursively(
    obj: &serde_json::Value,
    videos: &mut Vec<ChannelVideo>,
    channel_info: &ChannelInfo,
    base_trimmed: &str,
    seen: &mut HashSet<String>,
) {
    if let Some(map) = obj.as_object() {
        // Ищем видео (поддерживает и старый, и новый дизайн YouTube)
        if let Some(vr) = map.get("videoRenderer").or_else(|| map.get("gridVideoRenderer")) {
            if let Some(video_id) = vr.get("videoId").and_then(|id| id.as_str()) {
                if !seen.contains(video_id) {
                    seen.insert(video_id.to_string());
                    
                    let title = vr.pointer("/title/simpleText")
                        .or_else(|| vr.pointer("/title/runs/0/text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("No title")
                        .to_string();
                        
                    let views_raw = vr.pointer("/viewCountText/simpleText")
                        .or_else(|| vr.pointer("/shortViewCountText/simpleText"))
                        .or_else(|| vr.pointer("/viewCountText/runs/0/text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("0");
                        
                    let duration = vr.pointer("/lengthText/simpleText")
                        .or_else(|| vr.pointer("/lengthText/runs/0/text"))
                        .or_else(|| vr.pointer("/thumbnailOverlays/0/thumbnailOverlayTimeStatusRenderer/text/simpleText"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("0:00")
                        .to_string();
                        
                    let published_at = vr.pointer("/publishedTimeText/simpleText")
                        .or_else(|| vr.pointer("/publishedTimeText/runs/0/text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();

                    videos.push(ChannelVideo {
                        title,
                        author: channel_info.title.clone(),
                        video_id: video_id.to_string(),
                        thumbnail: format!("{}/thumbnail/{}", base_trimmed, video_id),
                        channel_thumbnail: channel_info.thumbnail.clone(),
                        views: parse_number(views_raw),
                        published_at,
                        duration,
                    });
                }
            }
        } 
        // Ищем шортсы на канале
        else if let Some(rr) = map.get("reelItemRenderer") {
            if let Some(video_id) = rr.get("videoId").and_then(|id| id.as_str()) {
                if !seen.contains(video_id) {
                    seen.insert(video_id.to_string());
                    
                    let title = rr.pointer("/headline/simpleText")
                        .or_else(|| rr.pointer("/headline/runs/0/text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("Short")
                        .to_string();
                        
                    let views_raw = rr.pointer("/viewCountText/simpleText")
                        .or_else(|| rr.pointer("/viewCountText/runs/0/text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("0");
                        
                    videos.push(ChannelVideo {
                        title,
                        author: channel_info.title.clone(),
                        video_id: video_id.to_string(),
                        thumbnail: format!("{}/thumbnail/{}", base_trimmed, video_id),
                        channel_thumbnail: channel_info.thumbnail.clone(),
                        views: parse_number(views_raw),
                        published_at: String::new(),
                        duration: "Short".to_string(),
                    });
                }
            }
        }
        
        for value in map.values() {
            extract_videos_recursively(value, videos, channel_info, base_trimmed, seen);
        }
    } else if let Some(arr) = obj.as_array() {
        for item in arr {
            extract_videos_recursively(item, videos, channel_info, base_trimmed, seen);
        }
    }
}

async fn extract_channel_info(data: &serde_json::Value, base: &str, channel_id: &str) -> ChannelInfo {
    let metadata = data.pointer("/metadata/channelMetadataRenderer").unwrap_or(&serde_json::Value::Null);
    
    let title = metadata.get("title").and_then(|t| t.as_str()).unwrap_or("No title").to_string();
    let description = metadata.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string();
    let external_id = metadata.get("externalId").and_then(|id| id.as_str()).unwrap_or(channel_id);
    
    let banner_url = data.pointer("/header/pageHeaderRenderer/content/pageHeaderViewModel/banner/imageBannerViewModel/image/sources")
        .and_then(|s| s.as_array())
        .and_then(|arr| arr.last())
        .and_then(|last| last.get("url"))
        .and_then(|url| url.as_str())
        .unwrap_or("");
    
    let banner_escaped = if !banner_url.is_empty() { urlencoding::encode(banner_url).to_string() } else { "".to_string() };
    let channel_icon = format!("{}/channel_icon/{}", base.trim_end_matches('/'), external_id);
    let banner = if !banner_escaped.is_empty() { format!("{}/channel_icon/{}", base.trim_end_matches('/'), banner_escaped) } else { "".to_string() };
    
    let mut subscriber_count = "0".to_string();
    let mut video_count = "0".to_string();
    
    if let Some(metadata_rows) = data.pointer("/header/pageHeaderRenderer/content/pageHeaderViewModel/metadata/contentMetadataViewModel/metadataRows").and_then(|mr| mr.as_array()) {
        if metadata_rows.len() > 1 {
            if let Some(parts) = metadata_rows[1].get("metadataParts").and_then(|mp| mp.as_array()) {
                if let Some(content) = parts.first().and_then(|p| p.pointer("/text/content")).and_then(|c| c.as_str()) {
                    subscriber_count = parse_number(content);
                }
                if parts.len() > 1 {
                    if let Some(content) = parts[1].pointer("/text/content").and_then(|c| c.as_str()) {
                        video_count = parse_number(content);
                    }
                }
            }
        }
    }
    
    ChannelInfo { title, description, thumbnail: channel_icon, banner, subscriber_count, video_count }
}

#[utoipa::path(
    get,
    path = "/get_channel_thumbnail.php",
    params(("video_id" = String, Query, description = "YouTube video ID")),
    responses((status = 200, description = "Channel thumbnail for video", body = serde_json::Value), (status = 400, description = "Missing video_id"))
)]
pub async fn get_channel_thumbnail_api(req: HttpRequest, data: web::Data<crate::AppState>) -> impl Responder {
    let query_params: HashMap<String, String> = web::Query::<HashMap<String, String>>::from_query(req.query_string()).map(|q| q.into_inner()).unwrap_or_default();
    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => return HttpResponse::BadRequest().json(serde_json::json!({"error": "ID параметр обязателен"})),
    };
    let channel_thumbnail_url = format!("{}/channel_icon/{}", base_url(&req, &data.config).trim_end_matches('/'), video_id);
    HttpResponse::Ok().json(serde_json::json!({"channel_thumbnail": channel_thumbnail_url}))
}