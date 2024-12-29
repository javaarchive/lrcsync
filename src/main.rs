
use anyhow::bail;
use audiotags::Tag;
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
    #[arg(short = 'i', long = "ignore", value_parser, num_args = 1, help = "ignore the follow properties when searching lrclib by not sending them, comma seperated")]
    pub ignore: Vec<String>,
    #[arg(short = 's', long = "search", default_value_t = false, help = "use searching on lrclib as a fallback")]
    pub search: bool,
    #[arg(short = 't', long = "tolerance", default_value_t = 5.0, help = "tolerance in seconds for searching lrclib")]
    pub tolerance: f32,
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

    pub fn to_get_query(&self) -> Vec<(String, String)> {
        let mut query = self.to_search_query();
        if let Some(duration) = &self.duration {
            query.push(("duration".to_string(), duration.to_string()));
        }
        query
    }

    pub fn to_search_query(&self) -> Vec<(String, String)> {
        let mut query = Vec::new();
        query.push(("track_name".to_string(), self.track_name.clone()));
        if self.artist_name.len() > 0 {
            query.push(("artist_name".to_string(), self.artist_name.clone()));
        }
        if let Some(album_name) = &self.album_name {
            query.push(("album_name".to_string(), album_name.clone()));
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
pub struct LrclibItem {
    pub id: u32,
    pub trackName: String,
    pub artistName: String,
    pub albumName: String,
    pub duration: f32,
    pub instrumental: bool,
    pub plainLyrics: Option<String>,
    pub syncedLyrics: Option<String>,
}

impl LrcLibClient {
    pub fn new(url: &str) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Lrclib-Client", DEFAULT_USER_AGENT.parse().unwrap());
        Self {
            url: "https://lrclib.net".to_string(),
            client: reqwest::Client::builder().default_headers(headers).user_agent(DEFAULT_USER_AGENT).build().expect("Failed to create reqwest client"),
        }
    }

    pub fn set_url(&mut self, url: &str) {
        self.url = url.to_string();
    }

    pub async fn get(&self, query: &LrclibQuery) -> anyhow::Result<Option<LrclibItem>> {
        let url = format!("{}/api/get" ,self.url);
        let request_builder = self.client.get(url).query(&query.to_get_query());
        let response = request_builder.send().await?;
        if response.status().is_success() {
            let body = response.text().await?;
            match serde_json::from_str::<LrclibItem>(&body) {
                Ok(item) => Ok(Some(item)),
                Err(err) => {
                    anyhow::bail!("Error parsing lrclib response (did the api schema change?): {}", err);
                },
            }
        } else {
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                Ok(None)
            } else {
                Err(anyhow::anyhow!("Error getting lrclib item: {}", response.status()))
            }
        }
    }

    pub async fn search(&self, query: &LrclibQuery) -> anyhow::Result<Option<Vec<LrclibItem>>> {
        let url = format!("{}/api/search" ,self.url);
        let request_builder = self.client.get(url).query(&query.to_search_query());
        let response = request_builder.send().await?;
        if response.status().is_success() {
            let body = response.text().await?;
            match serde_json::from_str::<Vec<LrclibItem>>(&body) {
                Ok(items) => Ok(Some(items)), 
                Err(err) => {
                    anyhow::bail!("Error parsing lrclib response (did the api schema change?): {}", err);
                },
            }
        } else {
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                Ok(None)
            } else {
                Err(anyhow::anyhow!("Error getting lrclib item: {}", response.status()))
            }
        }
    }
}

pub async fn write_lrc_for_file(entry: &DirEntry, synced_lyrics: &str, config: &CliConfig) -> anyhow::Result<()> {
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
                match Tag::new().read_from_path(entry.path()) {
                    Ok(tag) => {
                        let mut album_name: Option<String> = None;
                        let mut track_name = "".to_string();
                        let mut artist_name = "".to_string();
                        if let Some(album) = tag.album() {
                            album_name = Some(album.title.to_string());
                        }
                        if let Some(title) = tag.title() {
                            track_name = title.to_string();
                        }
                        if let Some(artists) = tag.artists() {
                            artist_name = artists.join(", ");
                        }
                        let duration: Option<f32> = match tag.duration() {
                            Some(duration) => Some(duration as f32),
                            None => None,
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
                        match client.get(&lrc_query).await {
                            Ok(Some(lrc_item)) => {
                                if let Some(synced_lyrics) = &lrc_item.syncedLyrics {
                                    println!("Found synced lrc for {}", entry.path().display());
                                    // write to file with extension changed to .lrc
                                    match write_lrc_for_file(&entry, synced_lyrics, &config).await {
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
                                                let mut canidates = lrc_items;
                                                if let Some(duration) = &lrc_query.duration {
                                                    // sort by closed to target duration
                                                    canidates.sort_by(|a, b| {
                                                        let a_duration = a.duration as f32;
                                                        let b_duration = b.duration as f32;
                                                        let a_delta = a_duration - duration;
                                                        let b_delta = b_duration - duration;
                                                        return a_delta.abs().partial_cmp(&b_delta.abs()).unwrap();
                                                    }); 
                                                
                                                    if config.tolerance > 0.0 {
                                                        canidates = canidates.into_iter().filter(|item| {
                                                            let item_duration = item.duration as f32;
                                                            let delta = item_duration - duration;
                                                            return delta.abs() < config.tolerance;
                                                        }).collect();
                                                    }
                                                }
                                                
                                                println!("Searched lrc (found {}secs vs actual {}secs out of {} filtered results) for {}",canidates[0].duration,lrc_query.duration.unwrap_or(-1.0), canidates.len(), entry.path().display());  
                                                // write to file with extension changed to .lrc
                                                // TODO: manual duration tolerance?
                                                match write_lrc_for_file(&entry, &canidates[0].syncedLyrics.as_ref().unwrap(), &config).await {
                                                    Ok(_) => {},
                                                    Err(err) => {
                                                        println!("Error in saving lrc {}: {}",entry.path().display(), err);
                                                    }
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