
use anyhow::bail;
use lofty::prelude::*;
use lofty::probe::Probe;
use clap::Parser;
use ignore::{DirEntry, WalkBuilder};
use serde::{Serialize, Deserialize};
use tokio::{fs::File, io::AsyncWriteExt};

#[derive(Parser, Debug)]
#[command(version = env!("CARGO_PKG_VERSION"), about = "pulls lrc files for songs in the current directory, try it on your music collection", long_about = None)]
pub struct CliConfig {
    #[arg(short = 'u', long = "lrclib-url", default_value = "https://lrclib.net")]
    pub lrclib_url: String,
    #[arg(short = 'a', long = "hidden", default_value_t = false)]
    pub hidden: bool,
    #[arg(short = 'f', long = "force", default_value_t = false, help = "overwrite existing lrc files")]
    pub force: bool,
    #[arg(short = 'i', long = "ignore", value_parser, value_delimiter = ',', help = "ignore the follow properties when searching lrclib by not sending them, comma seperated")]
    pub ignore: Vec<String>,
    #[arg(short = 's', long = "search", default_value_t = false, help = "use searching on lrclib as a fallback")]
    pub search: bool,
    #[arg(short = 't', long = "tolerance", default_value_t = 5.0, help = "tolerance in seconds for searching lrclib")]
    pub tolerance: f32,
    #[arg(short = 'd', long = "delay", default_value_t = 0, help = "delay in milliseconds between lrclib requests")]
    pub delay: u64,
}

static DEFAULT_USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    "/",
    env!("CARGO_PKG_VERSION"),
    " (",
    // github repo
    env!("CARGO_PKG_HOMEPAGE"),
    ")"
);

pub struct LrcLibClient {
    pub url: String,
    pub client: reqwest::Client,
    pub delay_ms: u64,
    has_requested: std::sync::atomic::AtomicBool,
}

pub struct LrclibQuery {
    pub track_name: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub duration: Option<f32>,
}

impl LrclibQuery {
    // old method
    pub fn to_get_query_string(&self) -> String {
        let mut query = String::new();
        query.push_str("track_name=");
        query.push_str(&self.track_name);
        query.push_str("&artist_name=");
        query.push_str(&self.artist_name);
        if let Some(album_name) = &self.album_name {
            query.push_str("&album_name=");
            query.push_str(album_name);
        }
        if let Some(duration) = &self.duration {
            query.push_str("&duration=");
            query.push_str(&duration.to_string());
        }
        query
    }

    pub fn can_use_get(&self) -> bool {
        !self.track_name.trim().is_empty() && !self.artist_name.trim().is_empty()
    }

    pub fn to_get_query(&self) -> Vec<(String, String)> {
        let mut query = self.to_search_query();
        if let Some(duration) = &self.duration {
            if duration.is_finite() && (1.0..=3600.0).contains(duration) {
                query.push(("duration".to_string(), duration.to_string()));
            }
        }
        query
    }

    pub fn to_search_query(&self) -> Vec<(String, String)> {
        let mut query = Vec::new();
        query.push(("track_name".to_string(), self.track_name.trim().to_string()));
        if !self.artist_name.trim().is_empty() {
            query.push(("artist_name".to_string(), self.artist_name.trim().to_string()));
        }
        if let Some(album_name) = &self.album_name {
            if !album_name.trim().is_empty() {
                query.push(("album_name".to_string(), album_name.trim().to_string()));
            }
        }
        query
    }

    pub fn remove_duration(&mut self) {
        self.duration = None;
    }

    pub fn remove_album_name(&mut self) {
        self.album_name = None;
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LrclibItem {
    pub id: i64,
    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub album_name: Option<String>,
    pub duration: Option<f32>,
    #[serde(default)]
    pub instrumental: bool,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
}

impl LrcLibClient {
    pub fn new(url: &str) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Lrclib-Client", DEFAULT_USER_AGENT.parse().unwrap());
        Self {
            url: url.to_string(),
            client: reqwest::Client::builder().default_headers(headers).user_agent(DEFAULT_USER_AGENT).build().expect("Failed to create reqwest client"),
            delay_ms: 0,
            has_requested: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn set_url(&mut self, url: &str) {
        self.url = url.to_string();
    }

    pub fn set_delay_ms(&mut self, delay_ms: u64) {
        self.delay_ms = delay_ms;
    }

    async fn throttle(&self) {
        if self.delay_ms > 0 && self.has_requested.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
        }
    }

    pub async fn get(&self, query: &LrclibQuery) -> anyhow::Result<Option<LrclibItem>> {
        if !query.can_use_get() {
            return Ok(None);
        }
        self.throttle().await;
        let url = format!("{}/api/get" ,self.url);
        let request_builder = self.client.get(url).query(&query.to_get_query());
        let response = request_builder.send().await?;
        let status = response.status();
        if status.is_success() {
            let body = response.text().await?;
            match serde_json::from_str::<LrclibItem>(&body) {
                Ok(item) => Ok(Some(item)),
                Err(err) => {
                    anyhow::bail!("Error parsing lrclib response (did the api schema change?): {}", err);
                },
            }
        } else {
            if status == reqwest::StatusCode::NOT_FOUND {
                Ok(None)
            } else {
                let body = response.text().await.unwrap_or_default();
                let body = body.trim();
                if body.is_empty() {
                    Err(anyhow::anyhow!("Error getting lrclib item: {}", status))
                } else {
                    Err(anyhow::anyhow!("Error getting lrclib item: {} ({})", status, body))
                }
            }
        }
    }

    pub async fn search(&self, query: &LrclibQuery) -> anyhow::Result<Option<Vec<LrclibItem>>> {
        self.throttle().await;
        let url = format!("{}/api/search" ,self.url);
        let request_builder = self.client.get(url).query(&query.to_search_query());
        let response = request_builder.send().await?;
        let status = response.status();
        if status.is_success() {
            let body = response.text().await?;
            match serde_json::from_str::<Vec<LrclibItem>>(&body) {
                Ok(items) => Ok(Some(items)), 
                Err(err) => {
                    anyhow::bail!("Error parsing lrclib response (did the api schema change?): {}", err);
                },
            }
        } else {
            if status == reqwest::StatusCode::NOT_FOUND {
                Ok(None)
            } else {
                let body = response.text().await.unwrap_or_default();
                let body = body.trim();
                if body.is_empty() {
                    Err(anyhow::anyhow!("Error getting lrclib item: {}", status))
                } else {
                    Err(anyhow::anyhow!("Error getting lrclib item: {} ({})", status, body))
                }
            }
        }
    }
}

pub async fn write_lrc_for_file(entry: &DirEntry, synced_lyrics: &str) -> anyhow::Result<()> {
    let lrc_path = entry.path().with_extension("lrc");

    match File::create(lrc_path).await {
        Ok(mut file) => {
            match file.write_all(synced_lyrics.as_bytes()).await {
                Ok(_) => {
                    println!("Wrote synced lrc to {}", entry.path().display());
                },
                Err(err) => {
                    bail!("Writing file failed: {}", err);
                }
            }
        },
        Err(err) => {
            bail!("Creating file failed: {}", err);
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let config = CliConfig::parse();
    let mut client = LrcLibClient::new(&config.lrclib_url);
    client.set_url(&config.lrclib_url);
    client.set_delay_ms(config.delay);
    for result in WalkBuilder::new(".").hidden(config.hidden).add_custom_ignore_filename(".lrcsyncignore").build() {
        match result {
            Ok(entry) => {
                // check if media file
                let mut is_audio = false;
                for guess in mime_guess::from_path(entry.path()) {
                    if guess.type_() == mime_guess::mime::AUDIO {
                        is_audio = true;
                    }
                }
                if !is_audio {
                    continue;
                }
                let has_existing_lrc = entry.path().with_extension("lrc").exists();
                if has_existing_lrc && !config.force {
                    println!("Skipping {}: lrc file already exists", entry.path().display());
                    continue;
                }
                // read file
                match Probe::open(entry.path()).and_then(|p| p.read()) {
                    Ok(tagged_file) => {
                        let mut album_name: Option<String> = None;
                        let mut track_name = "".to_string();
                        let mut artist_name = "".to_string();
                        if let Some(tag) = tagged_file.primary_tag().or_else(|| tagged_file.first_tag()) {
                            if let Some(album) = tag.album() {
                                if !album.trim().is_empty() {
                                    album_name = Some(album.trim().to_string());
                                }
                            }
                            if let Some(title) = tag.title() {
                                track_name = title.trim().to_string();
                            }
                            let artists: Vec<&str> = tag.get_strings(ItemKey::TrackArtist).map(str::trim).filter(|s| !s.is_empty()).collect();
                            if !artists.is_empty() {
                                artist_name = artists.join(", ");
                            } else if let Some(artist) = tag.artist() {
                                artist_name = artist.trim().to_string();
                            }
                        }
                        if track_name.is_empty() {
                            println!("Skipping {}: missing track title metadata", entry.path().display());
                            continue;
                        }
                        let duration_secs = tagged_file.properties().duration().as_secs_f64();
                        let duration: Option<f32> = if duration_secs.is_finite() && duration_secs > 0.0 {
                            Some(duration_secs as f32)
                        } else {
                            None
                        };
                        let mut lrc_query = LrclibQuery {
                            track_name: track_name.clone(),
                            artist_name: artist_name.clone(),
                            album_name: album_name.clone(),
                            duration: duration,
                        };
                        if config.ignore.contains(&"duration".to_string()) {
                            lrc_query.remove_duration();
                        }
                        if config.ignore.contains(&"album_name".to_string()) || config.ignore.contains(&"album".to_string()) {
                            lrc_query.remove_album_name();
                        }
                        if config.search && (config.ignore.contains(&"artist_name".to_string()) || config.ignore.contains(&"artist".to_string())) {
                            lrc_query.artist_name.clear();
                        }
                        match client.get(&lrc_query).await {
                            Ok(Some(lrc_item)) => {
                                if let Some(synced_lyrics) = &lrc_item.synced_lyrics {
                                    println!("Found synced lrc for {}", entry.path().display());
                                    // write to file with extension changed to .lrc
                                    match write_lrc_for_file(&entry, synced_lyrics).await {
                                        Ok(_) => {},
                                        Err(err) => {
                                            println!("Error in saving lrc {}: {}",entry.path().display(), err);
                                        }
                                    }
                                }
                            },
                            Ok(None) => {
                                if config.search {
                                    // search fallback
                                    println!("Searching lrc for {}", entry.path().display());
                                    // hide artist hack
                                    if config.ignore.contains(&"artist_name".to_string()) || config.ignore.contains(&"artist".to_string()) {
                                        lrc_query.artist_name = "".to_string();
                                    }
                                    match client.search(&lrc_query).await {
                                        Ok(Some(lrc_items)) => {
                                            // weird order but it works and avoids too much nesting
                                            if lrc_items.len() == 0 {
                                                println!("Did not find lrc for (no results) {}",entry.path().display());
                                            } else {
                                                let mut canidates: Vec<LrclibItem> = lrc_items.into_iter().filter(|item| {
                                                    item.synced_lyrics.is_some() && !item.synced_lyrics.as_ref().unwrap().is_empty()
                                                }).collect();
                                                if let Some(duration) = &lrc_query.duration {
                                                    // sort by closed to target duration
                                                    canidates.sort_by(|a, b| {
                                                        let a_delta = a.duration.map(|d| (d - duration).abs()).unwrap_or(f32::INFINITY);
                                                        let b_delta = b.duration.map(|d| (d - duration).abs()).unwrap_or(f32::INFINITY);
                                                        a_delta.total_cmp(&b_delta)
                                                    }); 
                                                
                                                    if config.tolerance > 0.0 {
                                                        canidates = canidates.into_iter().filter(|item| {
                                                            item.duration.map(|d| (d - duration).abs() < config.tolerance).unwrap_or(false)
                                                        }).collect();
                                                    }
                                                }

                                                if canidates.len() > 0 {
                                                
                                                    println!("Searched lrc (found {}secs vs actual {}secs out of {} filtered results) for {}",canidates[0].duration.unwrap_or(-1.0),lrc_query.duration.unwrap_or(-1.0), canidates.len(), entry.path().display());  
                                                    // write to file with extension changed to .lrc
                                                    // TODO: manual duration tolerance?
                                                    match write_lrc_for_file(&entry, canidates[0].synced_lyrics.as_ref().unwrap()).await {
                                                        Ok(_) => {},
                                                        Err(err) => {
                                                            println!("Error in saving lrc {}: {}",entry.path().display(), err);
                                                        }
                                                    }
                                                } else {
                                                    println!("Did not find lrc for (no results after filtering) {}",entry.path().display());
                                                }
                                            }
                                        },
                                        Ok(None) => {
                                            println!("Did not find lrc for {}",entry.path().display()); 
                                        },
                                        Err(err) => {
                                            println!("Error searching lrc for {}: {}",entry.path().display(), err);
                                        }
                                    }
                                } else {
                                    println!("Did not find lrc for {}",entry.path().display()); 
                                }
                            }
                            Err(err) => {
                                println!("Error finding lrc for {}: {}",entry.path().display(), err);
                            }
                        }
                    },
                    Err(err) => {
                        println!("Error reading file metadata {}: {}",entry.path().display(), err);
                    }
                }
            }
            Err(e) => {
                println!("Error walking: {}", e);
            }
        }
    }
}