use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_mainurl")]
    pub mainurl: String,
    pub secretkey: String,
    pub frontend_url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct ApiConfig {
    pub api_keys: Vec<String>,
    #[serde(default)]
    pub dontworkedkeys: Vec<String>,
    #[serde(default)]
    pub innertube_key: Option<String>,
    pub oauth_client_id: String,
    pub oauth_client_secret: String,
    pub request_timeout: u64,
    #[serde(default)]
    pub redirect_uri: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct VideoConfig {
    pub default_quality: String,
    pub available_qualities: Vec<String>,
    pub video_source: String,
    pub use_cookies: bool,
    #[serde(default = "default_count")]
    pub default_count: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct ProxyConfig {
    pub use_thumbnail_proxy: bool,
    pub use_channel_thumbnail_proxy: bool,
    pub use_video_proxy: bool,
    pub fetch_channel_thumbnails: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct CacheConfig {
    #[serde(default = "temp_folder_max_size_mb")]
    pub temp_folder_max_size_mb: u32,
    #[serde(default = "cache_cleanup_threshold_mb")]
    pub cache_cleanup_threshold_mb: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct InstantInstance {
    pub url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct Config {
    pub server: ServerConfig,
    pub api: ApiConfig,
    pub video: VideoConfig,
    pub proxy: ProxyConfig,
    pub cache: CacheConfig,
    #[serde(default)]
    pub instants: Vec<InstantInstance>,
}

static API_KEY_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn default_port() -> u16 {
    2823
}

fn default_mainurl() -> String {
    String::new()
}

fn default_count() -> u32 {
    50
}

fn temp_folder_max_size_mb() -> u32 {
    5120
}

fn cache_cleanup_threshold_mb() -> u32 {
    100
}

impl Config {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&contents)?;
        Ok(config)
    }

    pub fn get_api_key_rotated(&self) -> &str {
        let bad: HashSet<&str> = self.api.dontworkedkeys.iter().map(|s| s.as_str()).collect();
        let good_keys: Vec<&str> = self
            .api
            .api_keys
            .iter()
            .map(|s| s.as_str())
            .filter(|k| !k.is_empty() && !bad.contains(k))
            .collect();
        let index = API_KEY_COUNTER.fetch_add(1, Ordering::Relaxed) % good_keys.len();
        good_keys[index]
    }

    pub fn get_innertube_key(&self) -> Option<&str> {
        self.api
            .innertube_key
            .as_deref()
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
    }
}
