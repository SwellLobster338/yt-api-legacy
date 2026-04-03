use actix_web::http::header::{HeaderValue, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, LOCATION};
use actix_web::{web, HttpRequest, HttpResponse, Responder};
use cookie_scoop::{get_cookies, to_cookie_header};
use cookie_scoop::types::{BrowserName, GetCookiesOptions};
use futures_util::StreamExt;
use std::io::{Read, Seek};
use std::env;
use html_escape::decode_html_entities;
use image::{GenericImageView, Pixel};
use lazy_static::lazy_static;
use lru::LruCache;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio::task;
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

fn extract_ytcfg(html: &str) -> serde_json::Value {
    if let Some(cap) = regex::Regex::new(r"ytcfg\.set\((\{.*?\})\);")
        .unwrap()
        .captures(html)
    {
        if let Ok(cfg) = serde_json::from_str(&cap[1]) {
            return cfg;
        }
    }
    serde_json::Value::Object(serde_json::Map::new())
}

fn extract_initial_player_response(html: &str) -> serde_json::Value {
    let patterns = [
        r"ytInitialPlayerResponse\s*=\s*(\{.+?\});",
        r"window\['ytInitialPlayerResponse'\]\s*=\s*(\{.+?\});",
    ];
    
    for pattern in &patterns {
        if let Some(cap) = regex::Regex::new(pattern)
            .unwrap()
            .captures(html)
        {
            if let Ok(pr) = serde_json::from_str(&cap[1]) {
                return pr;
            }
        }
    }
    serde_json::Value::Object(serde_json::Map::new())
}

fn get_comments_token(data: &serde_json::Value) -> Option<String> {
    if let Some(contents) = data
        .get("contents")
        .and_then(|c| c.get("twoColumnWatchNextResults"))
        .and_then(|c| c.get("results"))
        .and_then(|r| r.get("results"))
        .and_then(|r| r.get("contents"))
        .and_then(|c| c.as_array())
    {
        for item in contents {
            if let Some(item_section) = item.get("itemSectionRenderer") {
                if item_section
                    .get("sectionIdentifier")
                    .and_then(|s| s.as_str())
                    .map(|s| s == "comment-item-section")
                    .unwrap_or(false)
                {
                    if let Some(section_contents) = item_section
                        .get("contents")
                        .and_then(|c| c.as_array())
                    {
                        for content_item in section_contents {
                            if content_item.get("continuationItemRenderer").is_some() {
                                return content_item
                                    .get("continuationItemRenderer")
                                    .and_then(|c| c.get("continuationEndpoint"))
                                    .and_then(|e| e.get("continuationCommand"))
                                    .and_then(|c| c.get("token"))
                                    .and_then(|t| t.as_str())
                                    .map(|s| s.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn simplify_text(node: &serde_json::Value) -> String {
    if node.is_null() {
        return String::new();
    }
    if let Some(s) = node.as_str() {
        return s.to_string();
    }
    if let Some(simple_text) = node.get("simpleText").and_then(|t| t.as_str()) {
        return simple_text.to_string();
    }
    if let Some(runs) = node.get("runs").and_then(|r| r.as_array()) {
        let mut result = String::new();
        for run in runs {
            if let Some(text) = run.get("text").and_then(|t| t.as_str()) {
                result.push_str(text);
            }
        }
        return result;
    }
    String::new()
}

fn recursive_find(obj: &serde_json::Value, key: &str) -> Vec<serde_json::Value> {
    let mut found = Vec::new();
    if let Some(obj_map) = obj.as_object() {
        if obj_map.contains_key(key) {
            found.push(obj_map[key].clone());
        }
        for value in obj_map.values() {
            found.extend(recursive_find(value, key));
        }
    } else if let Some(arr) = obj.as_array() {
        for item in arr {
            found.extend(recursive_find(item, key));
        }
    }
    found
}

fn all_strings(obj: &serde_json::Value) -> Vec<String> {
    let mut strings = Vec::new();
    if let Some(obj_map) = obj.as_object() {
        for value in obj_map.values() {
            strings.extend(all_strings(value));
        }
    } else if let Some(arr) = obj.as_array() {
        for item in arr {
            strings.extend(all_strings(item));
        }
    } else if let Some(s) = obj.as_str() {
        strings.push(s.to_string());
    }
    strings
}

fn search_number_near(data: &serde_json::Value, words: &[&str]) -> String {
    for s in all_strings(data) {
        let sl = s.to_lowercase();
        if words.iter().any(|w| sl.contains(w)) {
            if let Some(captures) = regex::Regex::new(r"[\d][\d,. ]*").unwrap().captures(&s) {
                return captures[0].replace(" ", "").replace(",", "");
            }
        }
    }
    String::new()
}

fn find_likes(next_data: &serde_json::Value) -> String {
    if let Some(contents) = next_data
        .get("contents")
        .and_then(|c| c.get("twoColumnWatchNextResults"))
        .and_then(|c| c.get("results"))
        .and_then(|r| r.get("results"))
        .and_then(|r| r.get("contents"))
        .and_then(|c| c.as_array())
    {
        if !contents.is_empty() {
            if let Some(primary_info) = contents[0].get("videoPrimaryInfoRenderer") {
                if let Some(video_actions) = primary_info.get("videoActions") {
                    if let Some(menu_renderer) = video_actions.get("menuRenderer") {
                        if let Some(top_level_buttons) = menu_renderer.get("topLevelButtons").and_then(|btns| btns.as_array()) {
                            if !top_level_buttons.is_empty() {
                                if let Some(button) = top_level_buttons[0].get("segmentedLikeDislikeButtonViewModel") {
                                    if let Some(like_button_vm) = button.get("likeButtonViewModel") {
                                        if let Some(like_button_vm2) = like_button_vm.get("likeButtonViewModel") {
                                            if let Some(toggle_button_vm) = like_button_vm2.get("toggleButtonViewModel") {
                                                if let Some(toggle_button_vm2) = toggle_button_vm.get("toggleButtonViewModel") {
                                                    if let Some(toggled_btn) = toggle_button_vm2.get("toggledButtonViewModel") {
                                                        if let Some(button_vm) = toggled_btn.get("buttonViewModel",) {
                                                            if let Some(title) = button_vm.get("title").and_then(|t| t.as_str()) {
                                                                if !title.is_empty() && title.chars().any(|c| c.is_ascii_digit()) {
                                                                    return parse_human_number(title);
                                                                }
                                                            }
                                                            
                                                            if let Some(acc_text) = button_vm.get("accessibilityText").and_then(|t| t.as_str()) {
                                                                if !acc_text.is_empty() {
                                                                    if let Some(caps) = regex::Regex::new(r"along with ([\d, ]*) other").unwrap().captures(acc_text) {
                                                                        let num = caps[1].replace(",", "").replace(" ", "");
                                                                        return num;
                                                                    }
                                                                    if let Some(caps) = regex::Regex::new(r"(\d[\d, ]*)").unwrap().captures(acc_text) {
                                                                        return parse_human_number(&caps[1]);
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                    
                                                    if let Some(default_btn) = toggle_button_vm2.get("defaultButtonViewModel") {
                                                        if let Some(button_vm) = default_btn.get("buttonViewModel") {
                                                            if let Some(title) = button_vm.get("title").and_then(|t| t.as_str()) {
                                                                if !title.is_empty() && title.chars().any(|c| c.is_ascii_digit()) {
                                                                    return parse_human_number(title);
                                                                }
                                                            }
                                                            
                                                            if let Some(acc_text) = button_vm.get("accessibilityText").and_then(|t| t.as_str()) {
                                                                if !acc_text.is_empty() {
                                                                    if let Some(caps) = regex::Regex::new(r"along with ([\d, ]*) other").unwrap().captures(acc_text) {
                                                                        let num = caps[1].replace(",", "").replace(" ", "");
                                                                        return num;
                                                                    }
                                                                    if let Some(caps) = regex::Regex::new(r"(\d[\d, ]*)").unwrap().captures(acc_text) {
                                                                        return parse_human_number(&caps[1]);
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    if let Some(micro) = next_data
        .get("microformat")
        .and_then(|m| m.get("playerMicroformatRenderer"))
    {
        if let Some(like_count) = micro.get("likeCount").and_then(|lc| lc.as_str()) {
            return like_count.to_string();
        }
    }
    
    search_number_near(next_data, &["like", "likes", "лайк", "лайков", "лайка"])
}

fn find_dislikes(next_data: &serde_json::Value) -> String {
    // Modern web response schema: segmentedLikeDislikeButtonViewModel
    if let Some(contents) = next_data
        .get("contents")
        .and_then(|c| c.get("twoColumnWatchNextResults"))
        .and_then(|c| c.get("results"))
        .and_then(|r| r.get("results"))
        .and_then(|r| r.get("contents"))
        .and_then(|c| c.as_array())
    {
        if let Some(primary_info) = contents.first().and_then(|c| c.get("videoPrimaryInfoRenderer")) {
            if let Some(top_level_buttons) = primary_info
                .get("videoActions")
                .and_then(|va| va.get("menuRenderer"))
                .and_then(|mr| mr.get("topLevelButtons"))
                .and_then(|btns| btns.as_array())
            {
                // Dislike is typically on the same segmented control.
                for btn in top_level_buttons {
                    if let Some(seg) = btn.get("segmentedLikeDislikeButtonViewModel") {
                        if let Some(dislike_vm) = seg
                            .get("dislikeButtonViewModel")
                            .and_then(|v| v.get("dislikeButtonViewModel"))
                            .and_then(|v| v.get("toggleButtonViewModel"))
                            .and_then(|v| v.get("toggleButtonViewModel"))
                        {
                            for key in ["toggledButtonViewModel", "defaultButtonViewModel"] {
                                if let Some(button_vm) = dislike_vm
                                    .get(key)
                                    .and_then(|v| v.get("buttonViewModel"))
                                {
                                    if let Some(title) = button_vm.get("title").and_then(|t| t.as_str())
                                    {
                                        if !title.is_empty() && title.chars().any(|c| c.is_ascii_digit())
                                        {
                                            return parse_human_number(title);
                                        }
                                    }
                                    if let Some(acc_text) = button_vm
                                        .get("accessibilityText")
                                        .and_then(|t| t.as_str())
                                    {
                                        if !acc_text.is_empty() {
                                            if let Some(caps) = regex::Regex::new(r"(\d[\d, ]*)")
                                                .unwrap()
                                                .captures(acc_text)
                                            {
                                                return parse_human_number(&caps[1]);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Older schema: segmentedLikeDislikeButtonRenderer
                    if let Some(seg) = btn.get("segmentedLikeDislikeButtonRenderer") {
                        if let Some(dislike_btn) = seg
                            .get("dislikeButton")
                            .and_then(|b| b.get("toggleButtonRenderer"))
                        {
                            // Attempt: defaultText/simpleText or accessibility label.
                            if let Some(default_text) = dislike_btn.get("defaultText") {
                                let t = simplify_text(default_text);
                                if !t.is_empty() && t.chars().any(|c| c.is_ascii_digit()) {
                                    return parse_human_number(&t);
                                }
                            }
                            if let Some(acc) = dislike_btn
                                .get("accessibility")
                                .and_then(|a| a.get("accessibilityData"))
                                .and_then(|a| a.get("label"))
                                .and_then(|l| l.as_str())
                            {
                                if let Some(caps) =
                                    regex::Regex::new(r"(\d[\d, ]*)").unwrap().captures(acc)
                                {
                                    return parse_human_number(&caps[1]);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback: heuristic scan for strings mentioning dislikes.
    search_number_near(
        next_data,
        &[
            "dislike",
            "dislikes",
            "дизлайк",
            "дизлайков",
            "дизлайка",
            "не нравится",
        ],
    )
}

fn parse_human_number(s: &str) -> String {
    if s.is_empty() {
        return "0".to_string();
    }
    
    let trimmed = s.trim();
    let mut cleaned = String::with_capacity(trimmed.len());
    
    for c in trimmed.chars() {
        if c != ' ' {
            cleaned.push(c.to_ascii_uppercase());
        }
    }
    
    // If we have a compact suffix (K/M/B) and only commas (no dot), treat comma as decimal separator.
    // Example: "92,7K" => "92.7K". If both dot and comma exist, treat comma as thousands separator.
    if cleaned.len() > 1 {
        let last_char = cleaned.chars().last().unwrap();
        if matches!(last_char, 'K' | 'M' | 'B') {
            if cleaned.contains('.') && cleaned.contains(',') {
                cleaned = cleaned.replace(',', "");
            } else if cleaned.contains(',') && !cleaned.contains('.') {
                cleaned = cleaned.replace(',', ".");
            }
        }
    }

    if cleaned.len() > 1 {
        let last_char = cleaned.chars().last().unwrap();
        if last_char.is_alphabetic() {
            let num_part = &cleaned[..cleaned.len()-1];
            match last_char {
                'K' => {
                    if let Ok(num) = num_part.parse::<f64>() {
                        return ((num * 1000.0).round() as i64).to_string();
                    }
                },
                'M' => {
                    if let Ok(num) = num_part.parse::<f64>() {
                        return ((num * 1000000.0).round() as i64).to_string();
                    }
                },
                'B' => {
                    if let Ok(num) = num_part.parse::<f64>() {
                        return ((num * 1000000000.0).round() as i64).to_string();
                    }
                },
                _ => {} // Not a recognized multiplier
            }
        }
    }
    
    let mut result = String::new();
    for c in cleaned.chars() {
        if c.is_ascii_digit() {
            result.push(c);
        }
    }
    result
}

fn parse_compact_count(text: &str) -> Option<String> {
    // Intended for subscriber counts and similar compact formats.
    // Supports: "92.7K", "92,7K", "1,234", "92,7 тыс", "1,2 млн", etc.
    let raw = text.trim();
    if raw.is_empty() {
        return None;
    }
    let lower = raw.to_lowercase();

    // Remove common words around the number.
    let cleaned = lower
        .replace("subscribers", "")
        .replace("subscriber", "")
        .replace("подписчиков", "")
        .replace("подписчик", "")
        .replace("подписчика", "")
        .replace("подписки", "")
        .trim()
        .to_string();

    // Russian multipliers.
    if cleaned.contains("тыс") || cleaned.contains("млн") {
        let mult = if cleaned.contains("млн") { 1_000_000.0 } else { 1_000.0 };
        let re = regex::Regex::new(r"(\d[\d\s\.,]*)").ok()?;
        let cap = re.captures(&cleaned)?;
        let mut num = cap.get(1)?.as_str().to_string();
        num = num.replace(' ', "");
        if num.contains('.') && num.contains(',') {
            num = num.replace(',', "");
        } else if num.contains(',') && !num.contains('.') {
            num = num.replace(',', ".");
        }
        if let Ok(v) = num.parse::<f64>() {
            return Some(((v * mult).round() as u64).to_string());
        }
    }

    // Fallback: reuse K/M/B parser.
    let n = parse_human_number(&cleaned);
    if n.is_empty() {
        None
    } else {
        Some(n)
    }
}

fn find_subscriber_count(nd: &serde_json::Value) -> String {
    
    if let Some(contents) = nd.get("contents") {
        if let Some(two_col) = contents.get("twoColumnWatchNextResults") {
            if let Some(results) = two_col.get("results") {
                if let Some(results2) = results.get("results") {
                    if let Some(contents_array) = results2.get("contents").and_then(|c| c.as_array()) {
                        if contents_array.len() > 1 {
                            let item1 = &contents_array[1];
                            
                            if let Some(video_secondary) = item1.get("videoSecondaryInfoRenderer") {
                                if let Some(owner) = video_secondary.get("owner") {
                                    if let Some(video_owner) = owner.get("videoOwnerRenderer") {
                                        if let Some(sub_text) = video_owner.get("subscriberCountText") {
                                            let text = sub_text
                                                .get("simpleText")
                                                .and_then(|t| t.as_str())
                                                .or_else(|| {
                                                    sub_text.get("runs").and_then(|r| r.as_array()).and_then(|arr| arr.first()).and_then(|r| r.get("text").and_then(|t| t.as_str()))
                                                });
                                            if let Some(simple_text) = text {
                                                if let Some(n) = parse_compact_count(simple_text) {
                                                    return n;
                                                }
                                                let digits: String =
                                                    simple_text.chars().filter(|c| c.is_ascii_digit()).collect();
                                                if !digits.is_empty() {
                                                    return digits;
                                                }
                                                return simple_text.to_string();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    "0".to_string()
}

fn find_comments_count(pr: &serde_json::Value, nd: &serde_json::Value) -> String {
    if let Some(panels) = nd.get("engagementPanels").and_then(|p| p.as_array()) {
        for panel in panels {
            if let Some(panel_renderer) = panel.get("engagementPanelSectionListRenderer") {
                if let Some(identifier) = panel_renderer.get("panelIdentifier").and_then(|id| id.as_str()) {
                    if identifier == "engagement-panel-comments-section" {
                        if let Some(header) = panel_renderer.get("header") {
                            if let Some(title_header_renderer) = header.get("engagementPanelTitleHeaderRenderer") {
                                if let Some(contextual_info) = title_header_renderer.get("contextualInfo") {
                                    if let Some(runs) = contextual_info.get("runs").and_then(|r| r.as_array()) {
                                        if !runs.is_empty() {
                                            if let Some(first_run) = runs[0].get("text").and_then(|t| t.as_str()) {
                                                let result = first_run.chars().filter(|c| c.is_ascii_digit()).collect::<String>();
                                                if !result.is_empty() {
                                                    return result;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    for d in [pr, nd] {
        if d.is_null() {
            continue;
        }
        
        let comment_texts = recursive_find(d, "commentCountText");
        let count_texts = recursive_find(d, "countText");
        
        let all_texts: Vec<&serde_json::Value> = comment_texts.iter().chain(count_texts.iter()).collect();
        
        for ct in all_texts {
            let text = ct
                .get("simpleText")
                .and_then(|st| st.as_str())
                .or_else(|| {
                    ct.get("runs")
                        .and_then(|r| r.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|first_run| first_run.get("text"))
                        .and_then(|t| t.as_str())
                });
            
            if let Some(text_content) = text {
                let n = text_content.chars().filter(|c| c.is_ascii_digit()).collect::<String>();
                if !n.is_empty() {
                    return n;
                }
            }
        }
    }
    search_number_near(nd, &["comment", "comments", "коммент", "коммента"])
}

fn translate_russian_time(time_str: &str) -> String {
    let time_lower = time_str.to_lowercase();
    
    let translations = [
        ("несколько секунд назад", "a few seconds ago"),
        ("секунду назад", "a second ago"),
        (" секунд назад", " seconds ago"),
        (" секунды назад", " seconds ago"),
        (" минуту назад", " a minute ago"),
        (" минут назад", " minutes ago"),
        (" часа назад", " hours ago"),
        (" часов назад", " hours ago"),
        (" день назад", " a day ago"),
        (" дней назад", " days ago"),
        (" недели назад", " weeks ago"),
        (" недель назад", " weeks ago"),
        (" месяц назад", " a month ago"),
        (" месяцев назад", " months ago"),
        (" года назад", " years ago"),
        (" лет назад", " years ago"),
        ("только что", "just now"),
        ("сегодня", "today"),
        ("вчера", "yesterday"),
        ("позавчера", "day before yesterday"),
    ];
    
    let mut result = time_str.to_string();
    for (russian, english) in &translations {
        if time_lower.contains(russian) {
            result = result.replace(russian, english);
            let capitalized_russian = format!("{}{}", 
                russian.chars().next().unwrap().to_uppercase().collect::<String>(),
                &russian[1..]
            );
            result = result.replace(&capitalized_russian, english);
        }
    }
    
    result
}

fn extract_comments(data: &serde_json::Value, base_url: &str) -> Vec<Comment> {
    let mut comments = Vec::new();
    
    fn walk(obj: &serde_json::Value, comments: &mut Vec<Comment>, base_url: &str) {
        if let Some(obj_map) = obj.as_object() {
            if obj_map.contains_key("commentEntityPayload") {
                let p = &obj_map["commentEntityPayload"];
                let props = p.get("properties").unwrap_or(&serde_json::Value::Null);
                
                let author = p
                    .get("author")
                    .and_then(|a| a.get("displayName"))
                    .and_then(|d| d.as_str())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Unknown")
                    .to_string();
                
                let text = if let Some(content_obj) = p.get("properties").and_then(|props| props.get("content")) {
                    if let Some(content_str) = content_obj.get("content").and_then(|c| c.as_str()) {
                        content_str.to_string()
                    } else if let Some(runs) = content_obj.get("runs").and_then(|r| r.as_array()) {
                        let mut text = String::new();
                        for run in runs {
                            if let Some(run_text) = run.get("text").and_then(|t| t.as_str()) {
                                text.push_str(run_text);
                            }
                        }
                        text
                    } else {
                        String::new()
                    }
                } else {
                    let content = props.get("content").unwrap_or(&serde_json::Value::Null);
                    if let Some(runs) = content.get("runs").and_then(|r| r.as_array()) {
                        let mut text = String::new();
                        for run in runs {
                            if let Some(run_text) = run.get("text").and_then(|t| t.as_str()) {
                                text.push_str(run_text);
                            }
                        }
                        text
                    } else {
                        String::new()
                    }
                };
                
                if !text.trim().is_empty() {
                    let published_at_raw = props
                        .get("publishedTime")
                        .and_then(|p| p.as_str())
                        .unwrap_or("unknown");
                    
                    let published_at = translate_russian_time(published_at_raw);
                    
                    let author_thumbnail_raw = p
                        .get("avatar")
                        .and_then(|a| a.get("image"))
                        .and_then(|i| i.get("sources"))
                        .and_then(|s| s.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|src| src.get("url"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    
                    let author_thumbnail = if !author_thumbnail_raw.is_empty() {
                        format!("{}/channel_icon/{}", base_url, urlencoding::encode(author_thumbnail_raw))
                    } else {
                        String::new()
                    };
                    
                    comments.push(Comment {
                        author,
                        text: text.trim().to_string(),  // Only trim if necessary
                        published_at,
                        author_thumbnail,
                        author_channel_url: None,
                    });
                }
            }
            for value in obj_map.values() {
                walk(value, comments, base_url);
            }
        } else if let Some(arr) = obj.as_array() {
            for item in arr {
                walk(item, comments, base_url);
            }
        }
    }
    
    walk(data, &mut comments, base_url);
    comments
}

lazy_static! {
    static ref THUMBNAIL_CACHE: Arc<Mutex<LruCache<String, (Vec<u8>, String, u64)>>> = Arc::new(
        Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1000).unwrap()))
    );
    static ref PLAYER_RESPONSE_CACHE: Arc<Mutex<LruCache<String, (Value, u64)>>> = Arc::new(
        Mutex::new(LruCache::new(std::num::NonZeroUsize::new(512).unwrap()))
    );
    static ref BROWSER_COOKIE_CACHE: Arc<Mutex<Option<(String, u64)>>> = Arc::new(Mutex::new(None));
    static ref DIRECT_URL_CLEANUP_STARTED: AtomicBool = AtomicBool::new(false);
}

const CACHE_DURATION: u64 = 3600;
const PLAYER_CACHE_TTL: u64 = 30;
const COOKIE_CACHE_TTL: u64 = 300;

fn get_duration_from_player_response(data: &serde_json::Value) -> u64 {
    // Пытаемся достать длительность из videoDetails
    if let Some(seconds_str) = data.get("videoDetails")
        .and_then(|vd| vd.get("lengthSeconds"))
        .and_then(|v| v.as_str()) 
    {
        if let Ok(secs) = seconds_str.parse::<u64>() {
            return secs;
        }
    }
    // Фолбек: из microformat
    if let Some(seconds_str) = data.get("microformat")
        .and_then(|m| m.get("playerMicroformatRenderer"))
        .and_then(|p| p.get("lengthSeconds"))
        .and_then(|v| v.as_str())
    {
         if let Ok(secs) = seconds_str.parse::<u64>() {
            return secs;
        }
    }
    0 // Если не нашли, считаем видео коротким/потоком
}

fn ffmpeg_binary() -> String {
    let exe_name = if cfg!(target_os = "windows") { "ffmpeg.exe" } else { "ffmpeg" };
    if let Ok(cwd) = std::env::current_dir() {
        let direct_path = cwd.join(exe_name);
        if direct_path.exists() {
            return direct_path.to_string_lossy().to_string();
        }
        let assets_path = cwd.join("assets").join(exe_name);
        if assets_path.exists() {
            return assets_path.to_string_lossy().to_string();
        }
    }
    exe_name.to_string()
}

fn sanitize_text(input: &str) -> String {
    let decoded = urlencoding::decode(input)
        .unwrap_or_else(|_| input.into())
        .to_string();
    let decoded = decode_html_entities(&decoded).to_string();
    
    let mut result = String::new();
    let mut prev_was_space = false;
    
    for c in decoded.chars() {
        if c.is_whitespace() {
            if !prev_was_space && !result.is_empty() {
                result.push(' ');
                prev_was_space = true;
            }
        } else if !c.is_control() {
            result.push(c);
            prev_was_space = false;
        }
    }
    
    result
}

async fn dominant_color_from_url(url: &str) -> Option<String> {
    let client = Client::new();
    let bytes = client.get(url).send().await.ok()?.bytes().await.ok()?;
    let vec = bytes.to_vec();
    task::spawn_blocking(move || {
        let img = image::load_from_memory(&vec).ok()?;
        let mut r: u64 = 0;
        let mut g: u64 = 0;
        let mut b: u64 = 0;
        let mut count: u64 = 0;
        for pixel in img.pixels() {
            let rgb = pixel.2.to_rgb();
            r += rgb[0] as u64;
            g += rgb[1] as u64;
            b += rgb[2] as u64;
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let r = (r / count) as u8;
        let g = (g / count) as u8;
        let b = (b / count) as u8;
        Some(format!("#{:02x}{:02x}{:02x}", r, g, b))
    })
    .await
    .ok()
    .flatten()
}

fn parse_quality_height(quality: &str) -> Option<u32> {
    let s = quality.trim().to_lowercase();
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        if let Ok(h) = digits.parse::<u32>() {
            return Some(h);
        }
    }
    let aliases: std::collections::HashMap<&str, u32> = [
        ("tiny", 144),
        ("small", 240),
        ("medium", 360),
        ("large", 480),
        ("hd", 720),
        ("hd720", 720),
        ("720p", 720),
        ("hd1080", 1080),
        ("1080p", 1080),
        ("144p", 144),
        ("240p", 240),
        ("360p", 360),
        ("480p", 480),
        ("2160p", 2160),
        ("1440p", 1440),
    ]
    .into_iter()
    .collect();
    aliases.get(s.as_str()).copied()
}

/// Removes old temp files created by direct_url: `yt_api_video_*` in temp_dir (older than 1h),
/// and files in `yt_api_hls_cache` older than 24h.
fn clean_direct_url_temp_files() {
    let temp_dir = env::temp_dir();
    let now = SystemTime::now();
    let max_age_video = Duration::from_secs(3600);   // 1 hour for codec conversion temp files
    let max_age_hls = Duration::from_secs(86400);   // 24 hours for HLS cache

    if let Ok(entries) = fs::read_dir(&temp_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("yt_api_video_") && (name.ends_with(".mp4") || name.ends_with(".3gp")) {
                        if let Ok(meta) = fs::metadata(&path) {
                            if let Ok(mtime) = meta.modified() {
                                if now.duration_since(mtime).unwrap_or(Duration::MAX) > max_age_video {
                                    let _ = fs::remove_file(&path);
                                    log::debug!("direct_url cleanup: removed old temp {}", path.display());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let hls_cache = temp_dir.join("yt_api_hls_cache");
    if hls_cache.is_dir() {
        if let Ok(entries) = fs::read_dir(&hls_cache) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(meta) = fs::metadata(&path) {
                    if let Ok(mtime) = meta.modified() {
                        if now.duration_since(mtime).unwrap_or(Duration::MAX) > max_age_hls {
                            let _ = fs::remove_file(&path);
                            log::debug!("direct_url cleanup: removed old hls cache {}", path.display());
                        }
                    }
                }
            }
        }
    }
}

async fn direct_url_cleanup_loop() {
    let interval = Duration::from_secs(900); // 15 minutes
    loop {
        tokio::time::sleep(interval).await;
        let _ = task::spawn_blocking(clean_direct_url_temp_files).await;
    }
}

fn spawn_direct_url_cleanup_if_needed() {
    if DIRECT_URL_CLEANUP_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        actix_web::rt::spawn(direct_url_cleanup_loop());
    }
}

async fn resolve_direct_stream_url(
    video_id: &str,
    quality: Option<&str>,
    audio_only: bool,
    config: &crate::config::Config,
) -> Result<String, String> {
    let player = fetch_player_response(video_id, config).await?;
    let target_height = quality
        .and_then(parse_quality_height)
        .or_else(|| parse_quality_height(&config.video.default_quality));
    if audio_only {
        select_best_audio_url_from_player_response(&player)
            .ok_or_else(|| "No direct audio URL found in Innertube response".to_string())
    } else {
        select_best_video_url_from_player_response(&player, target_height)
            .ok_or_else(|| "No direct video URL found in Innertube response".to_string())
    }
}

fn format_height(format: &Value) -> u32 {
    format
        .get("height")
        .and_then(|v| v.as_u64())
        .map(|h| h as u32)
        .or_else(|| {
            format
                .get("qualityLabel")
                .and_then(|v| v.as_str())
                .and_then(parse_quality_height)
        })
        .unwrap_or(0)
}

fn format_audio_bitrate(format: &Value) -> u32 {
    format
        .get("bitrate")
        .and_then(|v| v.as_u64())
        .or_else(|| format.get("averageBitrate").and_then(|v| v.as_u64()))
        .map(|b| b as u32)
        .unwrap_or(0)
}

fn format_mime_type(format: &Value) -> String {
    format
        .get("mimeType")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase()
}

fn is_avc_mp4_video(format: &Value) -> bool {
    let mime = format_mime_type(format);
    mime.starts_with("video/mp4") && mime.contains("avc1")
}

fn is_mp4a_audio(format: &Value) -> bool {
    let mime = format_mime_type(format);
    mime.starts_with("audio/mp4") && mime.contains("mp4a")
}

fn select_best_audio_url_from_player_response(data: &Value) -> Option<String> {
    let streaming = data.get("streamingData")?;
    let mut candidates: Vec<&Value> = Vec::new();
    if let Some(arr) = streaming.get("adaptiveFormats").and_then(|v| v.as_array()) {
        candidates.extend(arr.iter());
    }

    let mut best: Option<((u8, u32), &str)> = None;
    for f in candidates {
        let mime = format_mime_type(f);
        if !mime.starts_with("audio/") {
            continue;
        }
        let url = match f.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => continue,
        };
        let is_preferred_codec = if is_mp4a_audio(f) { 1 } else { 0 };
        let bitrate = format_audio_bitrate(f);
        let score = (is_preferred_codec, bitrate);
        if best.map(|(current, _)| score > current).unwrap_or(true) {
            best = Some((score, url));
        }
    }
    best.map(|(_, url)| url.to_string())
}

fn select_best_video_url_from_player_response(data: &Value, target_height: Option<u32>) -> Option<String> {
    let streaming = data.get("streamingData")?;
    let target = target_height.unwrap_or(360);
    let mut progressive_candidates: Vec<&Value> = Vec::new();
    // `formats` are progressive streams (video+audio), preferred path without ffmpeg.
    if let Some(arr) = streaming.get("formats").and_then(|v| v.as_array()) {
        progressive_candidates.extend(arr.iter());
    }

    // Similar to yt-dlp intent: best progressive video with height <= target.
    let mut best_under_or_equal: Option<((u8, u32, u32), &str)> = None;
    let mut best_over_target: Option<((u8, u32, u32), &str)> = None;

    for f in progressive_candidates {
        let url = match f.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => continue,
        };
        let mime = format_mime_type(f);
        if !mime.starts_with("video/") {
            continue;
        }

        let has_audio = f.get("audioQuality").is_some() || f.get("audioSampleRate").is_some();
        if !has_audio {
            continue;
        }
        let is_preferred_codec = if is_avc_mp4_video(f) { 1 } else { 0 };
        let height = format_height(f);
        if height == 0 {
            continue;
        }
        let bitrate = f
            .get("bitrate")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(0);

        if height <= target {
            let score = (is_preferred_codec, height, bitrate);
            if best_under_or_equal
                .map(|(current, _)| score > current)
                .unwrap_or(true)
            {
                best_under_or_equal = Some((score, url));
            }
        } else {
            // Fallback: nearest quality above target when <=target is unavailable.
            let inverse_height = u32::MAX - height;
            let score = (is_preferred_codec, inverse_height, bitrate);
            if best_over_target
                .map(|(current, _)| score > current)
                .unwrap_or(true)
            {
                best_over_target = Some((score, url));
            }
        }
    }

    let progressive_best = best_under_or_equal
        .or(best_over_target)
        .map(|(_, url)| url.to_string());
    if progressive_best.is_some() {
        return progressive_best;
    }

    // Fallback for videos where progressive formats are missing.
    let mut adaptive_candidates: Vec<&Value> = Vec::new();
    if let Some(arr) = streaming.get("adaptiveFormats").and_then(|v| v.as_array()) {
        adaptive_candidates.extend(arr.iter());
    }
    let mut fallback_under_or_equal: Option<((u8, u32, u32), &str)> = None;
    let mut fallback_over_target: Option<((u8, u32, u32), &str)> = None;
    for f in adaptive_candidates {
        let url = match f.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => continue,
        };
        let mime = format_mime_type(f);
        if !mime.starts_with("video/") {
            continue;
        }
        let is_preferred_codec = if is_avc_mp4_video(f) { 1 } else { 0 };
        let height = format_height(f);
        if height == 0 {
            continue;
        }
        let bitrate = f
            .get("bitrate")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(0);
        if height <= target {
            let score = (is_preferred_codec, height, bitrate);
            if fallback_under_or_equal
                .map(|(current, _)| score > current)
                .unwrap_or(true)
            {
                fallback_under_or_equal = Some((score, url));
            }
        } else {
            let inverse_height = u32::MAX - height;
            let score = (is_preferred_codec, inverse_height, bitrate);
            if fallback_over_target
                .map(|(current, _)| score > current)
                .unwrap_or(true)
            {
                fallback_over_target = Some((score, url));
            }
        }
    }

    fallback_under_or_equal
        .or(fallback_over_target)
        .map(|(_, url)| url.to_string())
}

async fn download_video_to_temp_file(
    video_id: &str,
    height: u32,
    source_url: &str,
    user_agent: &str,
    cookie_header: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    let temp_dir = env::temp_dir();
    let final_file_name = format!("yt_api_video_{}_{}p.mp4", video_id, height);
    let final_path = temp_dir.join(&final_file_name);
    let partial_path = temp_dir.join(format!("{}.tmp.mp4", final_file_name));
    let lock_path = temp_dir.join(format!("yt_api_video_{}_{}p.lock", video_id, height));

    if final_path.exists() {
        return Ok(final_path);
    }

    let start = std::time::Instant::now();
    loop {
        if final_path.exists() {
            return Ok(final_path);
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(_) => break,
            Err(_) => {
                let is_stale = if let Ok(meta) = fs::metadata(&lock_path) {
                    if let Ok(modified) = meta.modified() {
                        SystemTime::now()
                            .duration_since(modified)
                            .unwrap_or(Duration::ZERO)
                            .as_secs()
                            > 300
                    } else {
                        false
                    }
                } else {
                    false
                };
                if is_stale {
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
                tokio::time::sleep(Duration::from_millis(120)).await;
                if start.elapsed().as_secs() > 300 {
                    return Err("Timeout waiting for video predownload lock".to_string());
                }
            }
        }
    }

    let client = Client::builder()
        .user_agent(user_agent.to_string())
        .build()
        .map_err(|e| e.to_string())?;
    let result = async {
        let mut request = client
            .get(source_url)
            .header("Referer", "https://www.youtube.com")
            .header("Origin", "https://www.youtube.com");
        if let Some(cookie) = cookie_header {
            request = request.header("Cookie", cookie);
        }
        let mut response = request.send().await.map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("Video source HTTP {}", response.status()));
        }

        let mut file = tokio::fs::File::create(&partial_path)
            .await
            .map_err(|e| e.to_string())?;
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        file.flush().await.map_err(|e| e.to_string())?;
        tokio::fs::rename(&partial_path, &final_path)
            .await
            .map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    }
    .await;

    let _ = fs::remove_file(&lock_path);
    if result.is_err() {
        let _ = fs::remove_file(&partial_path);
    }
    result.map(|_| final_path)
}

async fn load_youtube_cookie_header_from_browser(use_cookies: bool) -> Option<String> {
    if !use_cookies {
        return None;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    {
        let cache = BROWSER_COOKIE_CACHE.lock().await;
        if let Some((cookie, ts)) = cache.as_ref() {
            if now.saturating_sub(*ts) <= COOKIE_CACHE_TTL {
                return Some(cookie.clone());
            }
        }
    }
    let options = GetCookiesOptions {
        url: "https://www.youtube.com/".to_string(),
        origins: None,
        names: None,
        browsers: Some(vec![BrowserName::Chrome, BrowserName::Edge, BrowserName::Firefox]),
        profile: None,
        chrome_profile: None,
        edge_profile: None,
        firefox_profile: None,
        safari_cookies_file: None,
        include_expired: Some(false),
        timeout_ms: Some(7000),
        debug: Some(false),
        mode: None,
        inline_cookies_file: None,
        inline_cookies_json: None,
        inline_cookies_base64: None,
    };
    let result = get_cookies(options).await;
    if !result.warnings.is_empty() {
        log::warn!("Browser cookies warnings: {}", result.warnings.join(" | "));
    }
    if result.cookies.is_empty() {
        None
    } else {
        let header = to_cookie_header(&result.cookies, &Default::default());
        if header.trim().is_empty() {
            None
        } else {
            let mut cache = BROWSER_COOKIE_CACHE.lock().await;
            *cache = Some((header.clone(), now));
            Some(header)
        }
    }
}

#[derive(Clone, Debug)]
struct SelectedVideoStreams {
    video_url: String,
    audio_url: Option<String>,
}

fn select_video_streams_for_quality(data: &Value, target_height: Option<u32>) -> Option<SelectedVideoStreams> {
    let video_url = select_best_video_url_from_player_response(data, target_height)?;
    let audio_url = select_best_audio_url_from_player_response(data);
    Some(SelectedVideoStreams { video_url, audio_url })
}

async fn mux_video_audio_to_temp_file(
    video_id: &str,
    height: u32,
    video_url: &str,
    audio_url: &str,
    user_agent: &str,
    cookie_header: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    let temp_dir = env::temp_dir();
    let final_file_name = format!("yt_api_video_{}_{}p.mp4", video_id, height);
    let final_path = temp_dir.join(&final_file_name);
    let partial_path = temp_dir.join(format!("{}.part", final_file_name));
    let lock_path = temp_dir.join(format!("yt_api_video_{}_{}p.lock", video_id, height));

    if final_path.exists() {
        return Ok(final_path);
    }

    let start = std::time::Instant::now();
    loop {
        if final_path.exists() {
            return Ok(final_path);
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(_) => break,
            Err(_) => {
                let is_stale = if let Ok(meta) = fs::metadata(&lock_path) {
                    if let Ok(modified) = meta.modified() {
                        SystemTime::now()
                            .duration_since(modified)
                            .unwrap_or(Duration::ZERO)
                            .as_secs()
                            > 300
                    } else {
                        false
                    }
                } else {
                    false
                };
                if is_stale {
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
                tokio::time::sleep(Duration::from_millis(120)).await;
                if start.elapsed().as_secs() > 300 {
                    return Err("Timeout waiting for video mux lock".to_string());
                }
            }
        }
    }

    let ffmpeg = ffmpeg_binary();
    let output = partial_path.to_string_lossy().to_string();
    let video = video_url.to_string();
    let audio = audio_url.to_string();
    let ua = user_agent.to_string();
    let cookie = cookie_header.map(|s| s.to_string());

    let status = task::spawn_blocking(move || {
        let mut common_headers = "Referer: https://www.youtube.com\r\nOrigin: https://www.youtube.com\r\n".to_string();
        if let Some(cookie_value) = cookie.as_deref() {
            common_headers.push_str(&format!("Cookie: {}\r\n", cookie_value));
        }
        Command::new(&ffmpeg)
            .args([
                "-nostdin",
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-user_agent",
                &ua,
                "-headers",
                &common_headers,
                "-i",
                &video,
                "-user_agent",
                &ua,
                "-headers",
                &common_headers,
                "-i",
                &audio,
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-c:v",
                "copy",
                "-c:a",
                "copy",
                "-movflags",
                "+faststart",
                "-f",
                "mp4",
                &output,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("Failed to run ffmpeg: {}", e))
    })
    .await
    .map_err(|e| format!("ffmpeg task join error: {}", e))?;

    let result = match status {
        Ok(out) if out.status.success() => {
            tokio::fs::rename(&partial_path, &final_path)
                .await
                .map_err(|e| e.to_string())?;
            Ok(final_path)
        }
        Ok(out) => Err(format!(
            "ffmpeg mux failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )),
        Err(e) => Err(e),
    };

    let _ = fs::remove_file(&lock_path);
    if result.is_err() {
        let _ = fs::remove_file(&partial_path);
    }
    result
}

async fn proxy_stream_response(
    target_url: &str,
    req: &HttpRequest,
    default_content_type: &str,
) -> HttpResponse {
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
        .build()
        .unwrap();

    let mut request_builder = client.get(target_url);
    if let Some(range_header) = req.headers().get("Range") {
        request_builder = request_builder.header("Range", range_header.clone());
    }

    match request_builder.send().await {
        Ok(resp) => {
            let status = resp.status();
            let headers = resp.headers().clone();
            let content_type = headers
                .get(CONTENT_TYPE)
                .and_then(|ct| ct.to_str().ok())
                .unwrap_or(default_content_type)
                .to_string();

            let stream = resp
                .bytes_stream()
                .map(|item| item.map_err(|e| actix_web::error::ErrorBadGateway(e)));

            let mut builder = HttpResponse::build(status);
            for (key, value) in headers.iter() {
                if key == "connection" || key == "transfer-encoding" {
                    continue;
                }
                builder.insert_header((key.clone(), value.clone()));
            }
            builder.insert_header((
                CONTENT_TYPE,
                HeaderValue::from_str(&content_type)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
            ));
            builder.streaming(stream)
        }
        Err(e) => {
            log::info!("Proxy request failed: {}", e);
            HttpResponse::BadGateway().json(serde_json::json!({
                "error": "Failed to proxy request"
            }))
        }
    }
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct VideoInfoResponse {
    pub title: String,
    pub author: String,
    #[serde(rename = "subscriberCount")]
    pub subscriber_count: String,
    pub channel_custom_url: Option<String>,
    pub description: String,
    pub video_id: String,
    pub embed_url: String,
    pub duration: String,
    pub published_at: String,
    pub likes: Option<String>,
    pub dislikes: Option<String>,
    pub views: Option<String>,
    pub comment_count: Option<String>,
    pub comments: Vec<Comment>,
    pub channel_thumbnail: String,
    pub thumbnail: String,
    pub video_url: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Comment {
    pub author: String,
    pub text: String,
    pub published_at: String,
    pub author_thumbnail: String,
    pub author_channel_url: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct RelatedVideo {
    pub title: String,
    pub author: String,
    pub video_id: String,
    pub views: String,
    pub published_at: String,
    pub thumbnail: String,
    pub channel_thumbnail: String,
    pub url: String,
    pub source: String,
    pub color: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct DirectUrlResponse {
    pub video_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct HlsManifestUrlResponse {
    pub hls_manifest_url: String,
    pub video_id: String,
    pub message: Option<String>,
}

#[utoipa::path(
    get,
    path = "/thumbnail/{video_id}",
    params(
        ("video_id" = String, Path, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Thumbnail quality (default, medium, high, standard, maxres)")
    ),
    responses(
        (status = 200, description = "Thumbnail image", content_type = "image/jpeg"),
        (status = 404, description = "Thumbnail not found")
    )
)]
pub async fn thumbnail_proxy(path: web::Path<String>, req: HttpRequest) -> impl Responder {
    let video_id = path.into_inner();

    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let quality = query_params
        .get("quality")
        .map(|s| s.as_str())
        .unwrap_or("medium");

    let quality_map = [
        ("default", "default.jpg"),
        ("medium", "mqdefault.jpg"),
        ("high", "hqdefault.jpg"),
        ("standard", "sddefault.jpg"),
        ("maxres", "maxresdefault.jpg"),
    ];

    let thumbnail_type = quality_map
        .iter()
        .find(|(q, _)| *q == quality)
        .map(|(_, t)| *t)
        .unwrap_or("mqdefault.jpg");

    let cache_key = format!("{}_{}", video_id, thumbnail_type);

    {
        let mut cache = THUMBNAIL_CACHE.lock().await;
        if let Some((data, content_type, timestamp)) = cache.get(&cache_key) {
            let current_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            if current_time - timestamp < CACHE_DURATION {
                return HttpResponse::Ok()
                    .content_type(content_type.as_str())
                    .body(data.clone());
            }
        }
    }

    let url = format!("https://i.ytimg.com/vi/{}/{}", video_id, thumbnail_type);

    let client = Client::new();

    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let headers = resp.headers().clone();
            if status == 404 && thumbnail_type != "mqdefault.jpg" {
                let fallback_url = format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", video_id);
                match client.get(&fallback_url).send().await {
                    Ok(fallback_resp) => {
                        let fallback_headers = fallback_resp.headers().clone();
                        let content_type = fallback_headers
                            .get("content-type")
                            .and_then(|ct| ct.to_str().ok())
                            .unwrap_or("image/jpeg")
                            .to_string();

                        match fallback_resp.bytes().await {
                            Ok(bytes) => {
                                let current_time = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs();

                                let mut cache = THUMBNAIL_CACHE.lock().await;
                                cache.put(
                                    cache_key,
                                    (bytes.to_vec(), content_type.clone(), current_time),
                                );

                                HttpResponse::Ok()
                                    .content_type(content_type.as_str())
                                    .body(bytes)
                            }
                            Err(_) => HttpResponse::NotFound().finish(),
                        }
                    }
                    Err(_) => HttpResponse::NotFound().finish(),
                }
            } else {
                let content_type = headers
                    .get("content-type")
                    .and_then(|ct| ct.to_str().ok())
                    .unwrap_or("image/jpeg")
                    .to_string();

                match resp.bytes().await {
                    Ok(bytes) => {
                        let current_time = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs();

                        let mut cache = THUMBNAIL_CACHE.lock().await;
                        cache.put(
                            cache_key,
                            (bytes.to_vec(), content_type.clone(), current_time),
                        );

                        HttpResponse::Ok()
                            .content_type(content_type.as_str())
                            .body(bytes)
                    }
                    Err(_) => HttpResponse::NotFound().finish(),
                }
            }
        }
        Err(_) => HttpResponse::NotFound().finish(),
    }
}

#[utoipa::path(
    get,
    path = "/channel_icon/{path_video_id}",
    params(
        ("path_video_id" = String, Path, description = "Channel ID (UC...), @handle, video ID or direct image URL")
    ),
    responses(
        (status = 200, description = "Channel icon image", content_type = "image/jpeg, image/png, image/webp"),
        (status = 404, description = "Channel icon not found"),
        (status = 400, description = "Bad request")
    )
)]
pub async fn channel_icon(
    path: web::Path<String>,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let input = path.into_inner();
    let config = &data.config;

    let decoded = urlencoding::decode(&input)
        .unwrap_or_else(|_| std::borrow::Cow::Owned(input.clone()))
        .to_string();
    
    if decoded.starts_with("http://") || decoded.starts_with("https://") {
        return proxy_image(&decoded).await;
    }

    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36")
        .build()
        .unwrap();

    let innertube_key = match config.get_innertube_key() {
        Some(key) => key,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Missing innertube_key in config.yml"
            }));
        }
    };

    let ctx = serde_json::json!({
        "client": {
            "clientName": "WEB",
            "clientVersion": "2.20250130.08.00",
            "hl": "en",
            "gl": "US"
        }
    });

    let mut channel_id = String::new();

    if input.len() == 24 && input.starts_with("UC") {
        channel_id = input.clone();
    } else if input.starts_with('@') {
        let handle = &input[1..];
        let page_url = format!("https://www.youtube.com/@{}", handle);

        if let Ok(resp) = client.get(&page_url).send().await {
            if let Ok(html) = resp.text().await {
                if let Some(start) = html.find(r#""channelId":"UC"#) {
                    let slice = &html[start + 13..]; // после "channelId":"
                    if let Some(end) = slice.find('"') {
                        channel_id = slice[..end].to_string();
                    }
                }
                if channel_id.is_empty() {
                    if let Some(pos) = html.find(r#"<link rel="canonical" href="https://www.youtube.com/channel/"#) {
                        let slice = &html[pos + 47..]; // длина префикса
                        if let Some(end) = slice.find('"') {
                            channel_id = slice[..end].to_string();
                        }
                    }
                }
            }
        }
    } else {
        channel_id = get_channel_id_from_video(&client, &input, &innertube_key, &ctx).await;
    }

    if channel_id.is_empty() {
        return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "Cannot determine channel ID"}));
    }

    let avatar_url = get_channel_avatar_url(&client, &channel_id, &innertube_key, &ctx).await;

    if avatar_url.is_empty() {
        return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "Channel avatar not found"}));
    }

    proxy_image(&avatar_url).await
}

#[utoipa::path(
    get,
    path = "/get-ytvideo-info.php",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Video quality"),
        ("proxy" = Option<String>, Query, description = "Use video proxy (true/false)")
    ),
    responses(
        (status = 200, description = "Video information", body = VideoInfoResponse),
        (status = 400, description = "Missing video ID"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_ytvideo_info(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    let base = base_url(&req, config);
    let base_trimmed = base.trim_end_matches('/');

    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "ID видео не был передан."
            }));
        }
    };

    let _quality = query_params
        .get("quality")
        .map(|s| s.as_str())
        .unwrap_or(&config.video.default_quality);
    let proxy_param = query_params
        .get("proxy")
        .map(|s| s.to_lowercase())
        .unwrap_or("true".to_string());
    let _use_video_proxy = proxy_param != "false";

    let innertube_key = match config.get_innertube_key() {
        Some(key) => key,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Missing innertube_key in config.yml"
            }));
        }
    };

    let client = Client::new();

    // Speed-up: avoid fetching the watch HTML page; fetch Innertube /player + /next in parallel.
    let user_agent = config.get_innertube_user_agent();
    let cookie_header = load_youtube_cookie_header_from_browser(config.video.use_cookies).await;
    let next_url = format!("https://www.youtube.com/youtubei/v1/next?key={}", innertube_key);
    let ctx = serde_json::json!({
        "client": {
            "clientName": "WEB",
            "clientVersion": "2.20260220.00.00",
            "hl": "en",
            "gl": "US"
        }
    });
    let next_payload = serde_json::json!({
        "context": ctx,
        "videoId": video_id
    });

    let next_fut = async {
        let mut request = client
            .post(&next_url)
            .header("User-Agent", &user_agent)
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Content-Type", "application/json");
        if let Some(cookie) = cookie_header.as_deref() {
            request = request.header("Cookie", cookie);
        }
        match request.json(&next_payload).send().await {
            Ok(resp) => resp.json::<serde_json::Value>().await.unwrap_or(serde_json::Value::Null),
            Err(_) => serde_json::Value::Null,
        }
    };

    let (player_response, next_data) = tokio::join!(
        fetch_player_response(&video_id, config),
        next_fut
    );

    let pr = match player_response {
        Ok(v) => v,
        Err(e) => {
            log::info!("Error calling player endpoint: {}", e);
            serde_json::Value::Null
        }
    };

    // Comments: most real comment payloads arrive via a continuation token.
    // We still keep this bounded (single request, short timeout) to avoid big latency spikes.
    let mut comments = extract_comments(&next_data, base_trimmed);
    if comments.is_empty() {
        if let Some(token) = get_comments_token(&next_data) {
            let cont_payload = serde_json::json!({
                "context": ctx,
                "continuation": token
            });

            let cont_resp = {
                let mut request = client
                    .post(&next_url)
                    .header("User-Agent", &user_agent)
                    .header("Accept-Language", "en-US,en;q=0.9")
                    .header("Content-Type", "application/json")
                    .timeout(Duration::from_secs(8));
                if let Some(cookie) = cookie_header.as_deref() {
                    request = request.header("Cookie", cookie);
                }
                match request.json(&cont_payload).send().await {
                    Ok(resp) => resp.json::<serde_json::Value>().await.unwrap_or(serde_json::Value::Null),
                    Err(e) => {
                        log::info!("Error calling comments continuation: {}", e);
                        serde_json::Value::Null
                    }
                }
            };

            if !cont_resp.is_null() {
                comments = extract_comments(&cont_resp, base_trimmed);
            }
        }
    }

    let vd = pr.get("videoDetails").unwrap_or(&serde_json::Value::Null);
    let micro = pr
        .get("microformat")
        .and_then(|m| m.get("playerMicroformatRenderer"))
        .unwrap_or(&serde_json::Value::Null);
    
    let likes = find_likes(&next_data);
    let dislikes = find_dislikes(&next_data);
    
    let comm_cnt = find_comments_count(&pr, &next_data);
    let subscriber_count = find_subscriber_count(&next_data);
    
    let mut title = String::new();
    let mut author = String::new();
    let mut description = String::new();
    let mut published_at = String::new();
    let mut views = String::new();
    let mut channel_id = String::new();
    let mut channel_thumbnail = String::new();
    let _duration = String::new();
    
    if let Some(contents) = next_data.get("contents") {
        if let Some(two_col) = contents.get("twoColumnWatchNextResults") {
            if let Some(results) = two_col.get("results") {
                if let Some(results_inner) = results.get("results") {
                    if let Some(contents_array) = results_inner.get("contents").and_then(|c| c.as_array()) {
                        if contents_array.len() > 1 {
                            if let Some(primary_info) = contents_array[0].get("videoPrimaryInfoRenderer") {
                                if let Some(title_val) = primary_info.get("title") {
                                    title = simplify_text(title_val);
                                }
                                
                                if let Some(date_text) = primary_info.get("dateText") {
                                    published_at = simplify_text(date_text);
                                }
                                
                                if let Some(view_count) = primary_info.get("viewCount") {
                                    if let Some(video_view_count) = view_count.get("videoViewCountRenderer") {
                                        if let Some(view_count_simple) = video_view_count.get("viewCount") {
                                            views = simplify_text(view_count_simple);
                                            views.retain(|c| c.is_ascii_digit());
                                        }
                                    }
                                }
                            }
                            
                            if let Some(secondary_info) = contents_array[1].get("videoSecondaryInfoRenderer") {
                                if let Some(attr_desc) = secondary_info.get("attributedDescription") {
                                    description = attr_desc.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
                                }
                                
                                if let Some(owner) = secondary_info.get("owner").and_then(|o| o.get("videoOwnerRenderer")) {
                                    if let Some(title_val) = owner.get("title") {
                                        author = simplify_text(title_val);
                                    }
                                    
                                    if let Some(nav_endpoint) = owner.get("navigationEndpoint") {
                                        if let Some(browse_endpoint) = nav_endpoint.get("browseEndpoint") {
                                            channel_id = browse_endpoint.get("browseId").and_then(|b| b.as_str()).unwrap_or("").to_string();
                                        }
                                    }
                                    
                                    if let Some(thumbnails) = owner.get("thumbnail").and_then(|t| t.get("thumbnails")).and_then(|arr| arr.as_array()) {
                                        if !thumbnails.is_empty() {
                                            channel_thumbnail = thumbnails[0].get("url").and_then(|u| u.as_str()).unwrap_or("").to_string();
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    if title.is_empty() {
        title = vd.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string();
    }
    if author.is_empty() {
        if let Some(author_val) = vd.get("author").and_then(|a| a.as_str()) {
            author = author_val.to_string();
        } else if let Some(owner_name) = micro.get("ownerChannelName").and_then(|n| n.as_str()) {
            author = owner_name.to_string();
        }
    }
    if description.is_empty() {
        if let Some(desc_val) = vd.get("shortDescription").and_then(|d| d.as_str()) {
            description = desc_val.to_string();
        } else if let Some(desc_val) = vd.get("description").and_then(|d| d.as_str()) {
            description = desc_val.to_string();
        }
    }
    if published_at.is_empty() {
        published_at = micro.get("publishDate").and_then(|p| p.as_str()).unwrap_or("").to_string();
    }
    if views.is_empty() {
        if let Some(view_str) = vd.get("viewCount").and_then(|v| v.as_str()) {
            views = view_str.chars().filter(|c| c.is_ascii_digit()).collect();
        }
    }
    if channel_id.is_empty() {
        channel_id = vd.get("channelId").and_then(|c| c.as_str()).unwrap_or("").to_string();
    }
    
    let duration = if let Some(length_seconds) = vd.get("lengthSeconds").and_then(|l| l.as_str()) {
        if let Ok(seconds) = length_seconds.parse::<u64>() {
            format!("PT{}M{}S", seconds / 60, seconds % 60)
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    
    let final_video_url = if config.video.source == "direct" {
        format!(
            "{}/direct_url?video_id={}",
            base_trimmed, video_id
        )
    } else {
        "".to_string()
    };
    
    let _final_video_url_with_proxy = if config.proxy.video_proxy && !final_video_url.is_empty() {
        format!(
            "{}/video.proxy?url={}",
            base_trimmed,
            urlencoding::encode(&final_video_url)
        )
    } else {
        final_video_url.clone()
    };
    
    let response = VideoInfoResponse {
        title: sanitize_text(&title),
        author,
        subscriber_count,
        description,
        video_id: video_id.clone(),
        channel_custom_url: micro
            .get("ownerProfileUrl")
            .and_then(|url| url.as_str())
            .and_then(|url_str| {
                url_str.rsplit('/').next().map(|part| part.to_string())
            }),
        embed_url: format!("https://www.youtube.com/embed/{}", video_id),
        duration,
        published_at,
        likes: if !likes.is_empty() { Some(likes) } else { None },
        dislikes: if !dislikes.is_empty() {
            Some(dislikes)
        } else {
            None
        },
        views: if !views.is_empty() { Some(views) } else { None },
        comment_count: if !comm_cnt.is_empty() { 
            Some(comm_cnt) 
        } else { 
            Some(comments.len().to_string()) 
        },
        comments,
        channel_thumbnail: if !channel_thumbnail.is_empty() {
            format!("{}/channel_icon/{}", base_trimmed, urlencoding::encode(&channel_thumbnail))
        } else if !channel_id.is_empty() {
            format!("{}/channel_icon/{}", base_trimmed, channel_id)
        } else {
            "".to_string()
        },
        thumbnail: format!("{}/thumbnail/{}", base_trimmed, video_id),
        video_url: final_video_url,
    };
    
    HttpResponse::Ok().json(response)
}

#[utoipa::path(
    get,
    path = "/get_related_videos.php",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("count" = Option<i32>, Query, description = "Number of related videos to return (default: 50)"),
        ("offset" = Option<i32>, Query, description = "Offset for pagination (default: 0)"),
        ("limit" = Option<i32>, Query, description = "Limit for pagination (default: 50)"),
        ("order" = Option<String>, Query, description = "Order of results (relevance, date, rating, viewCount, title) (default: relevance)"),
        ("token" = Option<String>, Query, description = "Refresh token for InnerTube recommendations")
    ),
    responses(
        (status = 200, description = "List of related videos", body = [RelatedVideo]),
        (status = 400, description = "Missing video ID"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_related_videos(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    let base = base_url(&req, config);
    let base_trimmed = base.trim_end_matches('/');

    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "ID видео не был передан."
            }));
        }
    };

    let quality = query_params
        .get("quality")
        .map(|q| q.clone())
        .unwrap_or_else(|| config.video.default_quality.clone());

    let count_param: i32 = query_params
        .get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(config.video.default_count as i32);

    let limit: i32 = query_params
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(count_param);

    let offset: i32 = query_params
        .get("offset")
        .and_then(|o| o.parse().ok())
        .unwrap_or(0);

    let desired_count = limit.max(20).min(100); // Target more videos like in Python script

    let client = Client::new();
    
    let innertube_key = match config.get_innertube_key() {
        Some(key) => key,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Missing innertube_key in config.yml"
            }));
        }
    };
    
    let context = serde_json::json!({
        "client": {
            "clientName": "WEB",
            "clientVersion": "2.20260128.05.00"
        }
    });

    let watch_url = format!("https://www.youtube.com/watch?v={}", video_id);
    let headers_map = {
        let mut map = reqwest::header::HeaderMap::new();
        map.insert(reqwest::header::USER_AGENT, "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/121.0.0.0 Safari/537.36".parse().unwrap());
        map.insert(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9".parse().unwrap());
        map.insert(reqwest::header::CONTENT_TYPE, "application/json".parse().unwrap());
        map
    };

    let html_response = match client
        .get(&watch_url)
        .headers(headers_map.clone())
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
    {
        Ok(resp) => resp.text().await.unwrap_or_default(),
        Err(e) => {
            log::info!("Error fetching watch page: {}", e);
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to fetch video page"
            }));
        }
    };

    let ytcfg = extract_ytcfg(&html_response);
    let api_key_from_cfg = ytcfg.get("INNERTUBE_API_KEY").and_then(|v| v.as_str()).unwrap_or(innertube_key);
    let context_from_cfg = ytcfg.get("INNERTUBE_CONTEXT").cloned().unwrap_or(context);

    let next_url = format!("https://www.youtube.com/youtubei/v1/next?key={}", api_key_from_cfg);
    let body = serde_json::json!({
        "context": context_from_cfg,
        "videoId": video_id
    });

    let next_response = match client
        .post(&next_url)
        .headers(headers_map.clone())
        .json(&body)
        .timeout(std::time::Duration::from_secs(25))
        .send()
        .await
    {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(json) => json,
            Err(e) => {
                log::info!("Error parsing next response: {}", e);
                return HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": "Failed to parse response"
                }));
            }
        },
        Err(e) => {
            log::info!("Error making next request: {}", e);
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to fetch related videos"
            }));
        }
    };

    let mut related_videos = extract_related_videos_from_response(&next_response);
    let mut continuation = get_related_continuation(&next_response);
    
    let mut page = 1;
    while let Some(cont_token) = continuation {
        if related_videos.len() >= desired_count as usize || page >= 6 {
            break;
        }
        
        page += 1;
        tokio::time::sleep(tokio::time::Duration::from_millis(1200 + (page as u64 * 300))).await;
        
        let cont_body = serde_json::json!({
            "context": context_from_cfg,
            "continuation": cont_token
        });
        
        let cont_response = match client
            .post(&next_url)
            .headers(headers_map.clone())
            .json(&cont_body)
            .timeout(std::time::Duration::from_secs(25))
            .send()
            .await
        {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(json) => json,
                Err(_) => break,
            },
            Err(_) => break,
        };
        
        let new_videos = extract_related_videos_from_response(&cont_response);
        if new_videos.is_empty() {
            break;
        }
        
        related_videos.extend(new_videos);
        continuation = get_related_continuation(&cont_response);
    }

    let mut seen = std::collections::HashSet::new();
    let unique_videos: Vec<_> = related_videos
        .into_iter()
        .filter(|v| {
            if v.video_id == video_id || seen.contains(&v.video_id) {
                false
            } else {
                seen.insert(v.video_id.clone());
                true
            }
        })
        .collect();

    let start_index = offset as usize;
    let end_index = (offset + limit) as usize;
    let paginated_videos = if start_index < unique_videos.len() {
        let actual_end = std::cmp::min(end_index, unique_videos.len());
        &unique_videos[start_index..actual_end]
    } else {
        &[][..]
    };

    let mut result_videos: Vec<RelatedVideo> = Vec::new();
    for video in paginated_videos {
        let thumbnail = format!("{}/thumbnail/{}", base_trimmed, video.video_id);
        let color = dominant_color_from_url(&format!("{}/thumbnail/{}", base_trimmed, video.video_id)).await;
        let channel_thumbnail = format!("{}/channel_icon/{}", base_trimmed, video.video_id);
        
        let video_url = format!("{}/get-ytvideo-info.php?video_id={}&quality={}", 
            base_trimmed, video.video_id, quality);
        
        let final_url = if config.proxy.video_proxy {
            format!("{}/video.proxy?url={}", 
                base_trimmed, urlencoding::encode(&video_url))
        } else {
            video_url
        };

        result_videos.push(RelatedVideo {
            title: video.title.clone(),
            author: video.channel.clone(),
            video_id: video.video_id.clone(),
            views: video.views.clone(),
            published_at: video.published.clone(),
            thumbnail,
            channel_thumbnail,
            url: final_url,
            source: "innertube".to_string(),
            color,
        });
    }

    HttpResponse::Ok().json(result_videos)
}

#[utoipa::path(
    get,
    path = "/get-direct-video-url.php",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Preferred quality")
    ),
    responses(
        (status = 200, description = "Direct URL for the video", body = DirectUrlResponse),
        (status = 400, description = "Missing video_id")
    )
)]
pub async fn get_direct_video_url(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "ID параметр обязателен"
            }));
        }
    };

    let quality = query_params.get("quality").map(|q| q.as_str());
    match resolve_direct_stream_url(&video_id, quality, false, &data.config).await {
        Ok(url) => HttpResponse::Ok().json(DirectUrlResponse { video_url: url }),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({
            "error": "Failed to resolve direct url",
            "details": e
        })),
    }
}

#[utoipa::path(
    get,
    path = "/direct_url",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Preferred quality"),
        ("proxy" = Option<String>, Query, description = "Pass-through proxy (true/false)"),
        ("codec" = Option<String>, Query, description = "Video codec for optional conversion: mpeg4 or h263. If passed, quality will be 360p")
    ),
    responses(
        (status = 200, description = "Video stream"),
        (status = 400, description = "Missing video_id or invalid codec")
    )
)]
pub async fn direct_url(req: HttpRequest, data: web::Data<crate::AppState>) -> impl Responder {
    spawn_direct_url_cleanup_if_needed();

    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "video_id parameter is required"
            }));
        }
    };

    // 1. Старые кодеки отключены: конвертация через ffmpeg убрана.
    let codec = query_params.get("codec").map(|c| c.as_str());
    if codec.is_some() {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "codec conversion is disabled",
            "details": "This build serves videos without ffmpeg transcoding"
        }));
    }

    // 2. HLS
    let hls_only = query_params.get("hls").map(|v| v == "true").unwrap_or(false);
    if hls_only {
        match get_hls_manifest_url(&video_id, &data.config).await {
            Ok(manifest_url) => {
                return HttpResponse::Ok().json(serde_json::json!({
                    "hls_manifest_url": manifest_url,
                    "video_id": video_id,
                    "message": "HLS Manifest URL ready"
                }));
            },
            Err(e) => {
                return HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": "Failed to get HLS manifest URL",
                    "details": e
                }));
            }
        }
    } 

    let proxy_param = query_params.get("proxy").map(|p| p.to_lowercase()).unwrap_or_else(|| "true".to_string());
    let use_proxy = proxy_param != "false";

    // Получаем инфо о видео
    let player_response = match fetch_player_response(&video_id, &data.config).await {
        Ok(data) => data,
        Err(e) => {
             return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to fetch player response",
                "details": e
            }));
        }
    };

    let duration_seconds = get_duration_from_player_response(&player_response);
    let requested_quality = query_params.get("quality").map(|q| q.as_str());
    
    let mut target_height = requested_quality
        .and_then(|q| parse_quality_height(q))
        .unwrap_or_else(|| parse_quality_height(&data.config.video.default_quality).unwrap_or(360));
    let cookie_header = load_youtube_cookie_header_from_browser(data.config.video.use_cookies).await;

    // --- ЛОГИКА КАЧЕСТВА ---

    // 1. Длинные видео (> 30 мин) и высокое качество -> Форсируем 360p
    if target_height > 360 && duration_seconds > 1800 {
        log::info!("Video > 30m ({}s). Forcing 360p for stability.", duration_seconds);
        target_height = 360;
    }

    // 2. Подбираем потоки как в логике yt-dlp: видео по quality + лучший аудио.
    let selected_streams = match select_video_streams_for_quality(&player_response, Some(target_height)) {
        Some(streams) => streams,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to resolve video url",
                "details": "No direct stream URLs in Innertube response"
            }));
        }
    };

    if req.method() == actix_web::http::Method::HEAD {
        let client = Client::new();
        match client.head(&selected_streams.video_url).send().await {
            Ok(resp) => {
                let mut builder = HttpResponse::build(resp.status());
                if let Some(len) = resp.headers().get(CONTENT_LENGTH) {
                    builder.insert_header((CONTENT_LENGTH, len.clone()));
                }
                if let Some(range) = resp.headers().get(CONTENT_RANGE) {
                    builder.insert_header((CONTENT_RANGE, range.clone()));
                }
                builder.insert_header((CONTENT_TYPE, HeaderValue::from_static("video/mp4")));
                builder.finish()
            }
            Err(_) => HttpResponse::Ok().finish(),
        }
    } else if use_proxy {
        let user_agent = data.config.get_innertube_user_agent();
        let prepared_file = if let Some(audio_url) = selected_streams.audio_url.as_deref() {
            mux_video_audio_to_temp_file(
                &video_id,
                target_height,
                &selected_streams.video_url,
                audio_url,
                &user_agent,
                cookie_header.as_deref(),
            )
            .await
        } else {
            download_video_to_temp_file(
                &video_id,
                target_height,
                &selected_streams.video_url,
                &user_agent,
                cookie_header.as_deref(),
            )
            .await
        };
        match prepared_file {
            Ok(path) => serve_mp4_from_cache(&path, &req, Some(duration_seconds)),
            Err(e) => {
                HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": "Failed to download video before streaming",
                    "details": e
                }))
            }
        }
    } else if !use_proxy {
        HttpResponse::Found()
            .insert_header((LOCATION, selected_streams.video_url))
            .finish()
    } else {
        HttpResponse::InternalServerError().finish()
    }
}

#[utoipa::path(
    get,
    path = "/hls_manifest_url",
    params(
        ("video_id" = String, Query, description = "YouTube video ID")
    ),
    responses(
        (status = 200, description = "HLS Manifest URL", body = HlsManifestUrlResponse),
        (status = 400, description = "Missing video_id"),
        (status = 500, description = "Failed to get manifest URL")
    )
)]
pub async fn hls_manifest_url(req: HttpRequest, data: web::Data<crate::AppState>) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "video_id parameter is required"
            }));
        }
    };

    match get_hls_manifest_url(&video_id, &data.config).await {
        Ok(manifest_url) => {
            HttpResponse::Ok().json(HlsManifestUrlResponse {
                hls_manifest_url: manifest_url,
                video_id,
                message: Some("HLS Master Manifest URL - use this for streams without quality selection".to_string()),
            })
        },
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to get HLS manifest URL",
                "details": e
            }))
        }
    }
}

#[utoipa::path(
    get,
    path = "/direct_audio_url",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("proxy" = Option<String>, Query, description = "Pass-through proxy (true/false)")
    ),
    responses(
        (status = 200, description = "Audio stream"),
        (status = 400, description = "Missing video_id")
    )
)]
pub async fn direct_audio_url(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "ID параметр обязателен"
            }));
        }
    };

    let proxy_param = query_params
        .get("proxy")
        .map(|p| p.to_lowercase())
        .unwrap_or_else(|| "true".to_string());
    let use_proxy = proxy_param != "false";

    let direct_url = match resolve_direct_stream_url(&video_id, None, true, &data.config).await {
        Ok(url) => url,
        Err(e) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to resolve audio url",
                "details": e
            }));
        }
    };

    if req.method() == actix_web::http::Method::HEAD {
        let client = Client::new();
        match client.head(&direct_url).send().await {
            Ok(resp) => {
                let mut builder = HttpResponse::build(resp.status());
                if let Some(len) = resp.headers().get(CONTENT_LENGTH) {
                    builder.insert_header((CONTENT_LENGTH, len.clone()));
                }
                if let Some(range) = resp.headers().get(CONTENT_RANGE) {
                    builder.insert_header((CONTENT_RANGE, range.clone()));
                }
                builder.insert_header((CONTENT_TYPE, HeaderValue::from_static("audio/m4a")));
                builder.finish()
            }
            Err(_) => HttpResponse::Ok().finish(),
        }
    } else if !use_proxy {
        HttpResponse::Found()
            .insert_header((LOCATION, direct_url))
            .finish()
    } else {
        proxy_stream_response(&direct_url, &req, "audio/m4a").await
    }
}

#[utoipa::path(
    get,
    path = "/video.proxy",
    params(
        ("url" = String, Query, description = "Target URL to proxy")
    ),
    responses(
        (status = 200, description = "Proxied response")
    )
)]
pub async fn video_proxy(req: HttpRequest) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let url = match query_params.get("url") {
        Some(u) => urlencoding::decode(u)
            .unwrap_or_else(|_| u.into())
            .to_string(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Missing url parameter"
            }));
        }
    };

    if req.method() == actix_web::http::Method::HEAD {
        let client = Client::new();
        match client.head(&url).send().await {
            Ok(resp) => {
                let mut builder = HttpResponse::build(resp.status());
                if let Some(len) = resp.headers().get(CONTENT_LENGTH) {
                    builder.insert_header((CONTENT_LENGTH, len.clone()));
                }
                if let Some(ct) = resp.headers().get(CONTENT_TYPE) {
                    builder.insert_header((CONTENT_TYPE, ct.clone()));
                }
                builder.finish()
            }
            Err(_) => HttpResponse::Ok().finish(),
        }
    } else {
        proxy_stream_response(&url, &req, "application/octet-stream").await
    }
}

#[utoipa::path(
    get,
    path = "/download",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Preferred quality")
    ),
    responses(
        (status = 302, description = "Redirect to downloadable stream")
    )
)]
pub async fn download_video(req: HttpRequest, data: web::Data<crate::AppState>) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "ID параметр обязателен"
            }));
        }
    };

    let quality = query_params.get("quality").map(|q| q.as_str());
    let direct_url = match resolve_direct_stream_url(&video_id, quality, false, &data.config).await
    {
        Ok(url) => url,
        Err(e) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to resolve video url",
                "details": e
            }));
        }
    };

    if req.method() == actix_web::http::Method::HEAD {
        HttpResponse::Ok().finish()
    } else {
        HttpResponse::Found()
            .insert_header((LOCATION, direct_url))
            .insert_header((
                "Content-Disposition",
                format!("attachment; filename=\"{}.mp4\"", video_id),
            ))
            .finish()
    }
}


fn get_related_continuation(data: &serde_json::Value) -> Option<String> {
    if let Some(contents) = data.get("contents")
        .and_then(|c| c.get("twoColumnWatchNextResults"))
        .and_then(|c| c.get("secondaryResults"))
        .and_then(|c| c.get("secondaryResults"))
        .and_then(|c| c.get("results"))
        .and_then(|c| c.as_array())
    {
        for item in contents {
            if let Some(item_section) = item.get("itemSectionRenderer") {
                if let Some(contents_arr) = item_section.get("contents").and_then(|c| c.as_array()) {
                    for content in contents_arr {
                        if let Some(cont_renderer) = content.get("continuationItemRenderer") {
                            if let Some(cont_endpoint) = cont_renderer
                                .get("continuationEndpoint")
                                .and_then(|ce| ce.get("continuationCommand"))
                            {
                                if let Some(token) = cont_endpoint.get("token").and_then(|t| t.as_str()) {
                                    return Some(token.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
struct RelatedVideoInfo {
    video_id: String,
    title: String,
    channel: String,
    views: String,
    duration: String,
    thumbnail: String,
    published: String,
}

fn extract_related_videos_from_response(data: &serde_json::Value) -> Vec<RelatedVideoInfo> {
    let mut videos = Vec::new();
    
    walk_json_for_videos(data, &mut videos);
    
    videos
}

fn walk_json_for_videos(obj: &serde_json::Value, videos: &mut Vec<RelatedVideoInfo>) {
    match obj {
        serde_json::Value::Object(map) => {
            if let Some(lockup_view_model) = map.get("lockupViewModel") {
                if let Some(video_info) = extract_video_from_lockup(lockup_view_model) {
                    videos.push(video_info);
                }
            }
            
            for (_, value) in map {
                walk_json_for_videos(value, videos);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                walk_json_for_videos(item, videos);
            }
        }
        _ => {}
    }
}

fn extract_video_from_lockup(lockup: &serde_json::Value) -> Option<RelatedVideoInfo> {
    let renderer_context = lockup.get("rendererContext")?.as_object()?;
    let command_context = renderer_context.get("commandContext")?.as_object()?;
    let on_tap = command_context.get("onTap")?.as_object()?;
    let innertube_command = on_tap.get("innertubeCommand")?.as_object()?;
    let watch_endpoint = innertube_command.get("watchEndpoint")?.as_object()?;
    
    let video_id = watch_endpoint.get("videoId")?.as_str()?.to_string();
    
    let metadata = lockup.get("metadata")?.as_object()?;
    let lockup_metadata = metadata.get("lockupMetadataViewModel")?.as_object()?;
    let title = lockup_metadata.get("title")?
        .as_object()?
        .get("content")?
        .as_str()?
        .to_string();
    
    let metadata_rows = metadata
        .get("lockupMetadataViewModel")?
        .as_object()?
        .get("metadata")?
        .as_object()?
        .get("contentMetadataViewModel")?
        .as_object()?
        .get("metadataRows")?
        .as_array()?
        .to_vec();
    
    let mut channel = "—".to_string();
    let mut views = "".to_string();
    let mut published = "—".to_string();
    
    if !metadata_rows.is_empty() {
        if let Some(first_row) = metadata_rows.first() {
            if let Some(metadata_parts) = first_row.as_object()
                .and_then(|r| r.get("metadataParts"))
                .and_then(|p| p.as_array()) 
            {
                if let Some(first_part) = metadata_parts.first() {
                    if let Some(text_content) = first_part.as_object()
                        .and_then(|p| p.get("text"))
                        .and_then(|t| t.as_object())
                        .and_then(|t| t.get("content"))
                        .and_then(|c| c.as_str())
                    {
                        channel = text_content.trim().to_string();
                    }
                }
            }
        }
        
        if metadata_rows.len() > 1 {
            if let Some(second_row) = metadata_rows.get(1) {
                if let Some(metadata_parts) = second_row.as_object()
                    .and_then(|r| r.get("metadataParts"))
                    .and_then(|p| p.as_array())
                {
                    if metadata_parts.len() >= 1 {
                        if let Some(views_raw) = metadata_parts[0].as_object()
                            .and_then(|p| p.get("text"))
                            .and_then(|t| t.as_object())
                            .and_then(|t| t.get("content"))
                            .and_then(|c| c.as_str())
                        {
                            views = clean_views_string(views_raw);
                        }
                    }
                    
                    if metadata_parts.len() >= 2 {
                        if let Some(published_raw) = metadata_parts[1].as_object()
                            .and_then(|p| p.get("text"))
                            .and_then(|t| t.as_object())
                            .and_then(|t| t.get("content"))
                            .and_then(|c| c.as_str())
                        {
                            published = published_raw.trim().to_string();
                        }
                    }
                }
            }
        }
    }
    
    let mut duration = "—".to_string();
    if let Some(content_image) = lockup.get("contentImage").and_then(|ci| ci.as_object()) {
        if let Some(thumbnail_vm) = content_image.get("thumbnailViewModel").and_then(|tvm| tvm.as_object()) {
            if let Some(overlays) = thumbnail_vm.get("overlays").and_then(|o| o.as_array()) {
                for overlay in overlays {
                    if let Some(badge_vm) = overlay.as_object()
                        .and_then(|o| o.get("thumbnailOverlayBadgeViewModel"))
                        .and_then(|bvm| bvm.as_object())
                    {
                        if let Some(thumbnail_badges) = badge_vm.get("thumbnailBadges").and_then(|tb| tb.as_array()) {
                            if let Some(first_badge) = thumbnail_badges.first() {
                                if let Some(badge_text) = first_badge.as_object()
                                    .and_then(|b| b.get("thumbnailBadgeViewModel"))
                                    .and_then(|tbm| tbm.as_object())
                                    .and_then(|tbm| tbm.get("text"))
                                    .and_then(|t| t.as_str())
                                {
                                    duration = badge_text.to_string();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    let thumbnail = String::new();
    
    Some(RelatedVideoInfo {
        video_id,
        title,
        channel,
        views,
        duration,
        thumbnail,
        published,
    })
}


async fn fetch_player_response(
    video_id: &str,
    config: &crate::config::Config,
) -> Result<Value, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    {
        let mut cache = PLAYER_RESPONSE_CACHE.lock().await;
        if let Some((cached, ts)) = cache.get(video_id) {
            if now.saturating_sub(*ts) <= PLAYER_CACHE_TTL {
                return Ok(cached.clone());
            }
            let _ = cache.pop(video_id);
        }
    }
    let api_key = config
        .get_innertube_key()
        .ok_or("innertube api key не задан в config.yml (api.innertube.key)")?;
    let client = Client::new();
    let user_agent = config.get_innertube_user_agent();
    let cookie_header = load_youtube_cookie_header_from_browser(config.video.use_cookies).await;
    let player_client = config.get_innertube_player_client();
    let json_data = serde_json::json!({
        "context": {
            "client": player_client.to_player_context_value()
        },
        "videoId": video_id
    });
    let url = format!("https://www.youtube.com/youtubei/v1/player?key={}", api_key);
    let mut request = client
        .post(&url)
        .header("User-Agent", &user_agent)
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Content-Type", "application/json");
    if let Some(cookie) = cookie_header.as_deref() {
        request = request.header("Cookie", cookie);
    }
    let resp = request
        .json(&json_data)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("player API HTTP {}", resp.status()));
    }
    let json = resp.json::<Value>().await.map_err(|e| e.to_string())?;
    {
        let mut cache = PLAYER_RESPONSE_CACHE.lock().await;
        cache.put(video_id.to_string(), (json.clone(), now));
    }
    Ok(json)
}

async fn get_hls_manifest_url(video_id: &str, config: &crate::config::Config) -> Result<String, String> {
    let data = fetch_player_response(video_id, config).await?;
    get_hls_manifest_url_from_player(&data)
}

fn get_hls_manifest_url_and_duration_from_player(data: &Value) -> Result<(String, Option<u64>), String> {
    let streaming_data = data
        .get("streamingData")
        .ok_or("streamingData отсутствует")?;
    let hls = streaming_data
        .get("hlsManifestUrl")
        .and_then(|v| v.as_str())
        .ok_or("hlsManifestUrl отсутствует (приватное/возраст/регион)")?;

    let duration_seconds = streaming_data
        .get("formats")
        .and_then(|a| a.get(0))
        .and_then(|f| f.get("approxDurationMs"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .map(|ms| (ms + 999) / 1000)
        .or_else(|| {
            streaming_data
                .get("adaptiveFormats")
                .and_then(|a| a.get(0))
                .and_then(|f| f.get("approxDurationMs"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<u64>().ok())
                .map(|ms| (ms + 999) / 1000)
        })
        .or_else(|| {
            data.get("videoDetails")
                .and_then(|vd| vd.get("lengthSeconds"))
                .and_then(|l| l.as_str())
                .and_then(|s| s.parse::<u64>().ok())
        });

    Ok((hls.to_string(), duration_seconds))
}

fn get_hls_manifest_url_from_player(data: &Value) -> Result<String, String> {
    get_hls_manifest_url_and_duration_from_player(data).map(|(url, _)| url)
}

fn get_direct_stream_url_from_player_response(data: &Value) -> Option<String> {
    select_best_video_url_from_player_response(data, None)
}

fn serve_mp4_from_cache(
    path: &Path,
    req: &HttpRequest,
    duration_seconds: Option<u64>,
) -> HttpResponse {
    let file_size = match fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return HttpResponse::NotFound().finish(),
    };
    if req.method() == actix_web::http::Method::HEAD {
        let mut builder = HttpResponse::Ok();
        builder
            .insert_header((CONTENT_TYPE, HeaderValue::from_static("video/mp4")))
            .insert_header(("Accept-Ranges", "bytes"))
            .insert_header((CONTENT_LENGTH, file_size.to_string()));
        if let Some(secs) = duration_seconds {
            let s = secs.to_string();
            builder
                .insert_header(("X-Content-Duration", s.as_str()))
                .insert_header(("Content-Duration", s.as_str()))
                .insert_header(("X-Video-Duration", s.as_str()))
                .insert_header(("X-Duration-Seconds", s.as_str()));
        }
        return builder.finish();
    }
    let range_header = req.headers().get("Range").and_then(|v| v.to_str().ok());
    let (start, end, status, content_range) = if let Some(range) = range_header {
        let mut start = 0u64;
        let mut end = file_size.saturating_sub(1);
        if let Some(cap) = regex::Regex::new(r"bytes=(\d+)-(\d*)").ok().and_then(|r| r.captures(range)) {
            if let Some(s) = cap.get(1).and_then(|m| m.as_str().parse::<u64>().ok()) {
                start = s.min(file_size.saturating_sub(1));
            }
            if let Some(m) = cap.get(2).map(|m| m.as_str()) {
                if !m.is_empty() {
                    if let Ok(e) = m.parse::<u64>() {
                        end = e.min(file_size.saturating_sub(1));
                    }
                }
            }
        }
        let content_range_val = format!("bytes {}-{}/{}", start, end, file_size);
        (start, end, actix_web::http::StatusCode::PARTIAL_CONTENT, Some(content_range_val))
    } else {
        (0, file_size.saturating_sub(1), actix_web::http::StatusCode::OK, None)
    };
    let start = start;
    let end = end;
    let content_range = content_range;
    let body = match fs::File::open(path) {
        Ok(mut f) => {
            let _ = f.seek(std::io::SeekFrom::Start(start));
            let len = end.saturating_sub(start) + 1;
            let mut buf = vec![0u8; len as usize];
            if let Ok(n) = f.read(&mut buf) {
                buf.truncate(n);
            }
            buf
        }
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };
    let mut builder = HttpResponse::build(status);
    builder
        .insert_header((CONTENT_TYPE, HeaderValue::from_static("video/mp4")))
        .insert_header(("Accept-Ranges", "bytes"))
        .insert_header((CONTENT_LENGTH, body.len()));
    if let Some(cr) = content_range {
        builder.insert_header((CONTENT_RANGE, cr));
    }
    if let Some(secs) = duration_seconds {
        let s = secs.to_string();
        builder
            .insert_header(("X-Content-Duration", s.as_str()))
            .insert_header(("Content-Duration", s.as_str()))
            .insert_header(("X-Video-Duration", s.as_str()))
            .insert_header(("X-Duration-Seconds", s.as_str()));
    }
    builder.body(body)
}

async fn get_channel_id_from_video(
    client: &Client,
    video_id: &str,
    key: &str,
    ctx: &serde_json::Value,
) -> String {
    let url = format!("https://www.youtube.com/youtubei/v1/player?key={}", key);

    let payload = serde_json::json!({
        "context": ctx,
        "videoId": video_id
    });

    match client
        .post(&url)
        .json(&payload)
        .header("Content-Type", "application/json")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                json.pointer("/videoDetails/channelId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

async fn get_channel_avatar_url(
    client: &Client,
    channel_id: &str,
    key: &str,
    ctx: &serde_json::Value,
) -> String {
    let url = format!("https://www.youtube.com/youtubei/v1/browse?key={}", key);

    let payload = serde_json::json!({
        "context": ctx,
        "browseId": channel_id
    });

    match client
        .post(&url)
        .json(&payload)
        .header("Content-Type", "application/json")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(header) = json.pointer("/header/c4TabbedHeaderRenderer") {
                    if let Some(thumbs) = header
                        .pointer("/avatar/thumbnails")
                        .and_then(|arr| arr.as_array())
                    {
                        if let Some(best) = thumbs.iter().max_by_key(|t| {
                            let w = t.pointer("/width").and_then(|w| w.as_u64()).unwrap_or(0);
                            w
                        }) {
                            if let Some(u) = best.pointer("/url").and_then(|u| u.as_str()) {
                                let mut url = u.to_string();
                                if url.contains("yt3.ggpht.com") {
                                    url = url.replace("yt3.ggpht.com", "yt3.googleusercontent.com");
                                }
                                return url;
                            }
                        }
                    }
                }

                json.pointer("/metadata/channelMetadataRenderer/avatar/thumbnails")
                    .and_then(|arr| arr.as_array())
                    .and_then(|thumbs| thumbs.last())
                    .and_then(|t| t.get("url").and_then(|u| u.as_str()))
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

async fn proxy_image(url: &str) -> HttpResponse {
    let processed_url = url.replace("s900", "s88");
    
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36")
        .build()
        .unwrap();

    match client.get(&processed_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("image/jpeg")
                .to_string();

            match resp.bytes().await {
                Ok(bytes) => HttpResponse::Ok()
                    .content_type(content_type)
                    .insert_header(("Cache-Control", "public, max-age=86400"))
                    .body(bytes),
                Err(_) => HttpResponse::NotFound().finish(),
            }
        }
        _ => HttpResponse::NotFound().finish(),
    }
}

fn clean_views_string(views_raw: &str) -> String {
    let cleaned = views_raw.replace(|c: char| !c.is_ascii_digit() && c != 'K' && c != 'M' && c != '.', "");
    cleaned.replace("K", "000").replace("M", "000000").replace(".", "")
}