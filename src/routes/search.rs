use actix_web::{web, HttpResponse, Responder, HttpRequest};
use serde::Serialize;
use utoipa::ToSchema;
use reqwest::Client;

#[derive(Serialize, ToSchema)]
pub struct TopVideo {
    pub title: String,
    pub author: String,
    pub video_id: String,
    pub thumbnail: String,
    pub channel_thumbnail: String,
}

#[utoipa::path(
    get,
    path = "/get_top_videos.php",
    params(
        ("count" = Option<i32>, Query, description = "Number of videos to return (default: 50)")
    ),
    responses(
        (status = 200, description = "List of top videos", body = [TopVideo]),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_top_videos(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    
    let count: i32 = req.query_string()
        .split('&')
        .find_map(|pair| {
            let mut parts = pair.split('=');
            if parts.next() == Some("count") {
                parts.next().and_then(|v| v.parse().ok())
            } else {
                None
            }
        })
        .unwrap_or(50);
    
    let count = count.min(50).max(1);
    
    let apikey = config.get_api_key();
    
    let client = Client::new();
    
    let url = format!(
        "https://www.googleapis.com/youtube/v3/videos?part=snippet&chart=mostPopular&maxResults={}&key={}",
        count,
        apikey
    );
    
    match client.get(&url).send().await {
        Ok(response) => {
            match response.json::<serde_json::Value>().await {
                Ok(json_data) => {
                    let mut top_videos: Vec<TopVideo> = Vec::new();
                    
                    if let Some(items) = json_data.get("items").and_then(|i| i.as_array()) {
                        for video in items {
                            if let (Some(video_info), Some(video_id)) = (
                                video.get("snippet"),
                                video.get("id").and_then(|id| id.as_str())
                            ) {
                                let title = video_info.get("title")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("Unknown Title")
                                    .to_string();
                                
                                let author = video_info.get("channelTitle")
                                    .and_then(|a| a.as_str())
                                    .unwrap_or("Unknown Author")
                                    .to_string();
                                
                                let thumbnail = format!("{}/thumbnail/{}", config.mainurl.trim_end_matches('/'), video_id);
                                
                                let channel_thumbnail = "".to_string();
                                
                                top_videos.push(TopVideo {
                                    title,
                                    author,
                                    video_id: video_id.to_string(),
                                    thumbnail,
                                    channel_thumbnail,
                                });
                            }
                        }
                    }
                    
                    HttpResponse::Ok().json(top_videos)
                }
                Err(e) => {
                    crate::log::info!("Error parsing YouTube API response: {}", e);
                    HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": "Failed to parse YouTube API response"
                    }))
                }
            }
        }
        Err(e) => {
            crate::log::info!("Error calling YouTube API: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to call YouTube API"
            }))
        }
    }
}