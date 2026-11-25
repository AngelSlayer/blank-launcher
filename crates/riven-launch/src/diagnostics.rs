use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};

use crate::instances::Instances;

const CHANNEL: &str = "https://angelslayer.github.io/Blank/channels/stable.json";
const ENDPOINT: &str = "https://blank-reports.blank-reports-worker.workers.dev/v1/reports";
const MAX_SECONDS: u64 = 604_800;
static SEND_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Serialize, Deserialize)]
struct Active {
    id: String,
    version: String,
    started: u64,
}

#[derive(Serialize)]
struct Event {
    code: &'static str,
    level: &'static str,
    count: u64,
    #[serde(rename = "firstSecond")]
    first_second: u64,
    #[serde(rename = "lastSecond")]
    last_second: u64,
}

#[derive(Serialize)]
struct Report {
    format: u32,
    id: String,
    version: String,
    #[serde(rename = "durationSeconds")]
    duration_seconds: u64,
    result: &'static str,
    events: Vec<Event>,
}

fn root() -> Option<PathBuf> {
    riven_sync::data_dir().map(|dir| dir.join("blank-diagnostics"))
}

fn enabled() -> bool {
    crate::load_settings().is_ok_and(|settings| settings.blank_diagnostics == Some(true))
}

fn epoch(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |time| time.as_secs())
}

fn modified(path: &Path) -> Option<u64> {
    fs::metadata(path).ok()?.modified().ok().map(epoch)
}

fn active_path(store: &Instances, id: &str) -> PathBuf {
    store.dir(id).join("blank-diagnostics-active.json")
}

pub fn is_blank(store: &Instances, id: &str) -> bool {
    riven_sync::install::load_state(&store.game_dir(id))
        .ok()
        .flatten()
        .is_some_and(|state| state.source == CHANNEL)
}

pub(crate) fn begin(store: &Instances, id: &str, started: SystemTime) {
    if !enabled() {
        return;
    }
    let Ok(Some(state)) = riven_sync::install::load_state(&store.game_dir(id)) else { return };
    if state.source != CHANNEL {
        return;
    }
    let mut random = [0; 16];
    if getrandom::fill(&mut random).is_err() {
        return;
    }
    let active = Active { id: hex::encode(random), version: state.version, started: epoch(started) };
    if let Err(error) = write_json(&active_path(store, id), &active) {
        tracing::warn!("cannot save the Blank diagnostic session: {error}");
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)
}

#[derive(Default)]
struct LogSummary {
    start: Option<u64>,
    latest: u64,
    day: u64,
    normal_stop: bool,
    events: BTreeMap<(&'static str, &'static str), Event>,
}

fn log_clock(timestamp: &str) -> Option<u64> {
    let time = timestamp.split_whitespace().last()?.split('.').next()?;
    let mut fields = time.split(':').map(str::parse::<u64>);
    let hour = fields.next()?.ok()?;
    let minute = fields.next()?.ok()?;
    let second = fields.next()?.ok()?;
    (hour < 24 && minute < 60 && second < 60 && fields.next().is_none())
        .then_some(hour * 3600 + minute * 60 + second)
}

fn event_code(message: &str) -> Option<&'static str> {
    if message.contains("error during loading of villager trades") {
        Some("villager_trades")
    } else if message.contains("recipe") && ["error loading", "failed to parse", "failed to load", "could not load"].iter().any(|part| message.contains(part)) {
        Some("recipe_load")
    } else if ["error reading optional pack metadata", "failed to load resource", "failed to reload resource"].iter().any(|part| message.contains(part)) {
        Some("resource_load")
    } else if ["error loading mods", "failed to load mods", "mod loading failed", "mod loading has failed"].iter().any(|part| message.contains(part)) {
        Some("mod_load")
    } else {
        None
    }
}

impl LogSummary {
    fn read(&mut self, input: impl BufRead) {
        for line in input.lines().map_while(Result::ok) {
            let Some((timestamp, rest)) = line.strip_prefix('[').and_then(|line| line.split_once("] [")) else { continue };
            let Some(clock) = log_clock(timestamp) else { continue };
            let Some((header, message)) = rest.split_once(']') else { continue };
            let mut observed = clock + self.day;
            if self.latest.saturating_sub(observed) > 43_200 {
                self.day += 86_400;
                observed += 86_400;
            }
            let start = *self.start.get_or_insert(observed);
            self.latest = self.latest.max(observed);
            let Some((_, level)) = header.rsplit_once('/') else { continue };
            let message = message.to_ascii_lowercase();
            if level == "INFO" && (message.contains("clearing modloader") || message.trim().ends_with("stopping!")) {
                self.normal_stop = true;
            }
            if !matches!(level, "WARN" | "ERROR" | "FATAL") {
                continue;
            }
            let level = if level == "WARN" { "warning" } else { "error" };
            let Some(code) = event_code(&message).or((level == "error").then_some("other_error")) else { continue };
            let elapsed = observed.saturating_sub(start).min(MAX_SECONDS);
            let event = self.events.entry((code, level)).or_insert(Event {
                code, level, count: 0, first_second: elapsed, last_second: elapsed,
            });
            event.count = (event.count + 1).min(1_000_000);
            event.first_second = event.first_second.min(elapsed);
            event.last_second = event.last_second.max(elapsed);
        }
    }
}

fn summarize(game: &Path, started: u64) -> LogSummary {
    let logs = game.join("logs");
    let mut summary = LogSummary::default();
    let mut archives: Vec<_> = fs::read_dir(&logs).into_iter().flatten().flatten()
        .map(|entry| entry.path())
        .filter(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(".log.gz")))
        .filter_map(|path| {
            let time = modified(&path)?;
            // Log4j can archive the previous launch as the new JVM starts.
            (time >= started.saturating_add(30)).then_some((time, path))
        }).collect();
    archives.sort_unstable();
    for (_, path) in archives {
        if let Ok(file) = File::open(path) {
            summary.read(BufReader::new(GzDecoder::new(file)));
        }
    }
    let latest = logs.join("latest.log");
    if modified(&latest).is_some_and(|time| time >= started) && let Ok(file) = File::open(latest) {
        summary.read(BufReader::new(file));
    }
    summary
}

fn recent_crash(game: &Path, started: u64) -> bool {
    fs::read_dir(game.join("crash-reports")).into_iter().flatten().flatten().any(|entry| {
        let path = entry.path();
        path.extension().is_some_and(|ext| ext == "txt")
            && modified(&path).is_some_and(|time| time >= started)
    })
}

fn finish(marker: &Path, game: &Path, played: Option<u64>, code: Option<i32>) -> io::Result<()> {
    let bytes = match fs::read(marker) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let active: Active = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if active.id.len() != 32 || !active.id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid report identifier"));
    }
    if !enabled() {
        return fs::remove_file(marker);
    }
    let Some(root) = root() else { return Ok(()) };
    let queue = root.join("pending");
    let sent = root.join("sent");
    let name = format!("{}.json", active.id);
    if sent.join(&name).exists() {
        return fs::remove_file(marker);
    }
    let summary = summarize(game, active.started);
    let elapsed_log = summary.start.map_or(0, |start| summary.latest.saturating_sub(start));
    let duration_seconds = elapsed_log.max(played.unwrap_or(elapsed_log)).min(MAX_SECONDS);
    let result = if crate::game::crashed(code) || recent_crash(game, active.started) {
        "crash"
    } else if code == Some(0) || summary.normal_stop {
        "normal"
    } else {
        "abnormal"
    };
    let report = Report {
        format: 1,
        id: active.id,
        version: active.version,
        duration_seconds,
        result,
        events: summary.events.into_values().collect(),
    };
    fs::create_dir_all(&queue)?;
    write_json(&queue.join(name), &report)?;
    fs::remove_file(marker)
}

/// Records only a sanitized summary, then tries pending reports in the background.
pub async fn finished(store: &Instances, id: &str, played: Option<u64>, code: Option<i32>) {
    let marker = active_path(store, id);
    if !marker.is_file() {
        return;
    }
    let game = store.game_dir(id);
    match tokio::task::spawn_blocking(move || finish(&marker, &game, played, code)).await {
        Ok(Ok(())) => {}
        error => tracing::warn!("could not finish Blank diagnostics: {error:?}"),
    }
    tokio::spawn(flush());
}

/// Retry a small batch at a time. An unsuccessful request leaves the report on disk.
pub async fn flush() {
    let _guard = SEND_LOCK.lock().await;
    if !enabled() {
        return;
    }
    let Some(root) = root() else { return };
    let Ok(entries) = fs::read_dir(root.join("pending")) else { return };
    let sent = root.join("sent");
    if fs::create_dir_all(&sent).is_err() {
        return;
    }
    let mut queue: Vec<_> = entries.flatten().map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json")).collect();
    queue.sort_by_key(|path| modified(path));
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build() else { return };
    for path in queue.into_iter().take(4) {
        if !enabled() {
            break;
        }
        let Ok(report) = fs::read(&path) else { continue };
        let Ok(response) = client.post(ENDPOINT)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(report)
            .send().await else { break };
        if !response.status().is_success() {
            break;
        }
        if let Some(name) = path.file_name() && let Err(error) = fs::rename(&path, sent.join(name)) {
            tracing::warn!("could not archive the delivered Blank report: {error}");
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_warnings_from_both_log_formats_across_midnight() {
        let log = "[23:00:00] [main/INFO]: Started\n\
                   [23:12:00] [Render thread/WARN]: Error during loading of villager trades\n\
                   [25нояб.2026 08:58:00.000] [Render thread/WARN] [jeresources/]: Error during loading of villager trades\n\
                   [09:00:00] [Render thread/INFO]: Clearing ModLoader\n";
        let mut summary = LogSummary::default();
        summary.read(log.as_bytes());
        let event = &summary.events[&("villager_trades", "warning")];
        assert_eq!((event.count, event.first_second, event.last_second), (2, 720, 35_880));
        assert_eq!(summary.latest - summary.start.unwrap(), 36_000);
        assert!(summary.normal_stop);
    }
}
