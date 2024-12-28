
use audiotags::Tag;
use clap::Parser;
use ignore::WalkBuilder;
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
    pub ignore: Vec<String>
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
    pub fn to_query_string(&self) -> String {
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

    pub fn to_query(&self) -> Vec<(String, String)> {
        let mut query = Vec::new();
        query.push(("track_name".to_string(), self.track_name.clone()));
        query.push(("artist_name".to_string(), self.artist_name.clone()));
        if let Some(album_name) = &self.album_name {
            query.push(("album_name".to_string(), album_name.clone()));
        }
        if let Some(duration) = &self.duration {
            query.push(("duration".to_string(), duration.to_string()));
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
        let request_builder = self.client.get(url).query(&query.to_query());
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
                        if config.ignore.contains(&"album_name".to_string()) {
                            lrc_query.remove_album_name();
                        }
                        match client.get(&lrc_query).await {
                            Ok(Some(lrc_item)) => {
                                if let Some(synced_lyrics) = &lrc_item.syncedLyrics {
                                    println!("Found synced lrc for {}", entry.path().display());
                                    // write to file with extension changed to .lrc
                                    match File::create(entry.path().with_extension("lrc")).await {
                                        Ok(mut file) => {
                                            match file.write_all(synced_lyrics.as_bytes()).await {
                                                Ok(_) => {
                                                    println!("Wrote synced lrc to {}", entry.path().display());
                                                },
                                                Err(err) => {
                                                    println!("Error writing file {}: {}",entry.path().display(), err);
                                                }
                                            }
                                        },
                                        Err(err) => {
                                            println!("Error creating file {}: {}",entry.path().display(), err);
                                        }
                                    }
                                }
                            },
                            Ok(None) => {
                                println!("Did not find lrc for {}",entry.path().display()); 
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