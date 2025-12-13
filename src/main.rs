use discord_rich_presence::{
    DiscordIpc, DiscordIpcClient,
    activity::{Activity, ActivityType, Assets, Button, Timestamps},
};
use std::{
    env, fs,
    io::Write,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
    vec,
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
            "Client ID file created at {}. Please edit it and add your client ID.",
            path.display()
        )
        .into());
    }

    let content = fs::read_to_string(&path)?;
    Ok(content)
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client_id = read_client_id()?;
    let mut client = DiscordIpcClient::new(&client_id.to_string());
    client.connect()?;

    let mut last_track_id = String::new();
    let mut cached_start: Option<i64> = None;
    let mut last_position: Option<u64> = None;

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
                } else if let Some(prev_pos) = last_position
                    && position.abs_diff(prev_pos) > 3
                {
                    cached_start = Some(now - position as i64);
                }

                last_position = Some(position);

                if is_music() {
                    let mut activity = Activity::new()
                        .activity_type(ActivityType::Listening)
                        .details(&artist)
                        .state(&title)
                        .assets(Assets::new().large_image("logo"))
                        .status_display_type(
                            discord_rich_presence::activity::StatusDisplayType::Details,
                        )
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
