use discord_rich_presence::{
    DiscordIpc, DiscordIpcClient,
    activity::{Activity, ActivityType, Assets, Button, StatusDisplayType, Timestamps},
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    env, fs,
    io::Write,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn is_music() -> bool {
    let artist = get_metadata("xesam:artist");
    let album = get_metadata("xesam:album");
    let length = get_length();

    matches!(
        (artist, album, length),
        (Some(a), Some(b), Some(l))
            if !a.is_empty() && !b.is_empty() && l > 60
    )
}

fn read_client_id() -> Result<String, Box<dyn std::error::Error>> {
    let path: PathBuf = env::home_dir()
        .ok_or("Couldn't get home directory path")?
        .join(".config/music-discord-rpc/client_id.txt");

    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = fs::File::create(&path)?;
        writeln!(file, "0000000000000000000")?;

        return Err(format!(
            "Client ID file created at {}. Please edit it and add your Discord client ID.",
            path.display()
        )
        .into());
    }

    let content = fs::read_to_string(&path)?;
    Ok(content.trim().to_string())
}

fn get_position() -> Option<u64> {
    let output = Command::new("playerctl").args(["position"]).output().ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()
        .map(|v| v as u64)
}

fn get_length() -> Option<u64> {
    let output = Command::new("playerctl")
        .args(["metadata", "mpris:length"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .ok()
        .map(|v| v / 1000000)
}

fn get_metadata(field: &str) -> Option<String> {
    let output = Command::new("playerctl")
        .args(["metadata", field])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

#[derive(Deserialize)]
struct ITunesResponse {
    #[serde(rename = "resultCount")]
    result_count: i32,
    results: Vec<ITunesResult>,
}

#[derive(Deserialize)]
struct ITunesResult {
    #[serde(rename = "artworkUrl100")]
    artwork_url_100: Option<String>,
}

fn fetch_album_art_itunes(artist: &str, album: &str) -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;

    let search_urls = vec![
        format!(
            "https://itunes.apple.com/search?term={} {}&entity=album&limit=1",
            urlencoding::encode(artist),
            urlencoding::encode(album)
        ),
        format!(
            "https://itunes.apple.com/search?term={}&entity=album&limit=1",
            urlencoding::encode(artist)
        ),
        format!(
            "https://itunes.apple.com/search?term={}&entity=album&limit=1",
            urlencoding::encode(album)
        ),
    ];

    for search_url in search_urls {
        let response: ITunesResponse = match client.get(&search_url).send() {
            Ok(resp) => match resp.json() {
                Ok(data) => data,
                Err(_) => continue,
            },
            Err(_) => continue,
        };

        if response.result_count > 0
            && let Some(result) = response.results.first()
            && let Some(url) = &result.artwork_url_100
        {
            let large_url = url.replace("100x100", "600x600");
            return Some(large_url);
        }
    }

    None
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client_id = read_client_id()?;
    let mut client = DiscordIpcClient::new(&client_id);
    client.connect()?;

    let mut last_track_id = String::new();
    let mut cached_start: Option<i64> = None;
    let mut last_position: Option<u64> = None;

    let mut art_cache: HashMap<String, String> = HashMap::new();

    loop {
        if let (Some(title), Some(artist)) =
            (get_metadata("xesam:title"), get_metadata("xesam:artist"))
        {
            let album = get_metadata("xesam:album").unwrap_or("Unknown album".to_string());

            let track_id = format!("{}-{}-{}", artist, title, album);

            if let Some(position) = get_position() {
                let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

                let track_changed = track_id != last_track_id;

                if track_changed {
                    cached_start = Some(now);
                    last_track_id = track_id.clone();

                    let cache_key = format!("{}-{}", artist, album);
                    if !art_cache.contains_key(&cache_key) {
                        println!("Fetching album art for: {} - {}", artist, album);
                        if let Some(art_url) = fetch_album_art_itunes(&artist, &album) {
                            println!("Found album art: {}", art_url);
                            art_cache.insert(cache_key.clone(), art_url);
                        } else {
                            println!("No album art found, using default");
                            art_cache.insert(cache_key.clone(), "logo".to_string());
                        }
                    }
                } else if let Some(prev_pos) = last_position
                    && position.abs_diff(prev_pos) > 3
                {
                    cached_start = Some(now - position as i64);
                }

                last_position = Some(position);

                if is_music() {
                    let cache_key = format!("{}-{}", artist, album);
                    let art_image = art_cache
                        .get(&cache_key)
                        .map(|s| s.as_str())
                        .unwrap_or("logo");

                    let mut activity = Activity::new()
                        .activity_type(ActivityType::Listening)
                        .status_display_type(StatusDisplayType::State)
                        .details(&title)
                        .state(&artist)
                        .assets(Assets::new().large_image(art_image).large_text(&album))
                        .buttons(vec![Button::new(
                            "Listen on Apple Music",
                            "https://notimplemented.yet",
                        )]);

                    if let Some(start) = cached_start {
                        activity = activity.timestamps(Timestamps::new().start(start));
                    }

                    if let Err(e) = client.set_activity(activity) {
                        eprintln!("Failed to set activity - {:?}. Reconnecting...", e);
                        client.connect()?;
                    }
                }
            }
        } else {
            last_track_id.clear();
            cached_start = None;
            last_position = None;

            if let Err(e) = client.clear_activity() {
                eprintln!("Failed to clear activity - {:?}. Reconnecting...", e);
                client.connect()?;
            }
        }

        thread::sleep(Duration::from_secs(1));
    }
}
