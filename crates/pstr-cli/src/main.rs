//! `pstr` — the command-line front end.
//!
//! Exists mainly to prove the pieces below it work without a window: add a
//! share, crawl it, look at what the catalog made of the names. The GUI is the
//! product; this is the thing that tells you *which layer* is broken when it
//! misbehaves.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use proton_sdk::ids::{LinkId, NodeUid, VolumeId};
use pstr_core::catalog::{Catalog, build_rows};
use pstr_core::config::AppDirs;
use pstr_core::{ShareStore, SharedLibrary};
use pstr_player::{EndReason, Player, PlayerConfig, PlayerEvent};
use pstr_stream::{DiskCacheConfig, LibraryOpener, StreamConfig, StreamSource};

#[derive(Parser)]
#[command(name = "pstr", version, about = "proton-stream command line")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum MetadataCommand {
    /// Show the current settings.
    Status,
    /// Turn enrichment on or off, and choose a provider.
    ///
    /// Off by default and deliberately: matching sends the titles in the
    /// catalog to a third party. See `pstr_core::metadata`.
    Set {
        /// Turn lookups on.
        #[arg(long, conflicts_with = "off")]
        on: bool,
        /// Turn lookups off, and forget every stored answer.
        #[arg(long)]
        off: bool,
        /// `anilist` or `tmdb`.
        #[arg(long)]
        provider: Option<String>,
        /// Store an API key for the chosen provider. Prompted for, never taken
        /// on the command line, where it would land in the shell history.
        #[arg(long)]
        api_key: bool,
    },
    /// Look up every title that has no fresh answer.
    Match {
        /// Ask about every title, including ones already matched.
        #[arg(long)]
        force: bool,
        /// Look up at most this many. Useful for a first look at how the
        /// matching is doing without spending a whole library on it.
        #[arg(long)]
        limit: Option<usize>,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Add a share by its full URL, including the `#password` fragment.
    Add {
        /// What to call it in the UI.
        #[arg(long)]
        name: String,
        /// The full share URL.
        url: String,
        /// Prompt for the link's custom password, if it has one.
        #[arg(long)]
        custom_password: bool,
    },
    /// List configured shares.
    Shares,
    /// Forget a share and its stored credentials.
    Remove {
        /// The share id, as printed by `shares`.
        id: String,
    },
    /// Crawl one share (or all of them) into the catalog.
    Crawl {
        /// Crawl only this share.
        #[arg(long)]
        share: Option<String>,
    },
    /// Print what the catalog holds.
    List {
        /// Restrict to one share.
        #[arg(long)]
        share: Option<String>,
    },
    /// Match the catalog against a metadata provider.
    ///
    /// Prints what matched and what did not, and stores the answers where the
    /// GUI reads them. This is the layer to reach for when the poster wall looks
    /// wrong: it shows the *matching* in isolation, with no network beyond the
    /// provider and no window in the way.
    Metadata {
        #[command(subcommand)]
        command: MetadataCommand,
    },
    /// Measure what playback would feel like, without a player.
    ///
    /// Reports the three numbers that decide whether this app is usable at all:
    /// how long a cold seek takes, how fast a sustained read runs, and how much
    /// memory the block ring actually holds.
    Bench {
        /// Substring of a file name or parsed title to read.
        #[arg(long, value_name = "TEXT")]
        file: String,
        /// Restrict the search to one share.
        #[arg(long)]
        share: Option<String>,
        /// Blocks to pull ahead of the reader; 0 disables read-ahead.
        #[arg(long, default_value_t = pstr_stream::DEFAULT_READAHEAD_BLOCKS)]
        readahead: usize,
        /// Resident block budget, in MiB.
        #[arg(long, default_value_t = 128)]
        ring_mib: u64,
        /// Also exercise the on-disk block cache.
        #[arg(long)]
        disk_cache: bool,
        /// How much to read for the throughput measurement, in MiB.
        #[arg(long, default_value_t = 32)]
        throughput_mib: u64,
        /// Cross-check a mid-file read against the SDK's own range download.
        #[arg(long)]
        verify: bool,
    },
    /// Play a catalogued file in an mpv window.
    Play {
        /// Substring of a file name or parsed title to play.
        #[arg(long, value_name = "TEXT")]
        file: String,
        /// Restrict the search to one share.
        #[arg(long)]
        share: Option<String>,
        /// Blocks to pull ahead of the player. Defaults to none: mpv's own
        /// demuxer cache is the read-ahead, and a second one under it only
        /// competes with the reads mpv is waiting on.
        #[arg(long, default_value_t = pstr_player::READAHEAD_BLOCKS)]
        readahead: usize,
        /// Resident block budget, in MiB.
        #[arg(long, default_value_t = 128)]
        ring_mib: u64,
        /// Keep decrypted blocks on disk, so a rewatch needs no network.
        #[arg(long)]
        disk_cache: bool,
        /// Start at this position, in seconds.
        #[arg(long, value_name = "SECONDS")]
        start: Option<f64>,
        /// Decode without a window or sound. For checking the pipeline where
        /// there is no display to open one on.
        #[arg(long)]
        headless: bool,
        /// Stop after this many seconds of wall-clock playback. Smoke tests
        /// only — it is how an unattended run ends without a window to close.
        #[arg(long, value_name = "SECONDS")]
        for_seconds: Option<f64>,
    },
}

/// Prompt for a secret, or read one line of it from stdin when there is no
/// terminal to prompt on.
///
/// `rpassword` opens `/dev/tty` and fails outright without one, which makes the
/// command unusable from a script or a pipe. Neither path echoes the secret, and
/// neither puts it in `argv` where every other process on the machine could read
/// it — which is why there is no `--password <value>` flag.
fn read_password(prompt: &str) -> Result<String> {
    use std::io::{BufRead, IsTerminal};

    if std::io::stdin().is_terminal() {
        return Ok(rpassword::prompt_password(prompt)?);
    }

    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("read password from stdin")?;
    let password = line.trim_end_matches(['\r', '\n']).to_string();
    if password.is_empty() {
        anyhow::bail!("no password on stdin, and no terminal to prompt on");
    }
    Ok(password)
}

/// The runtime is built by hand rather than by `#[tokio::main]` because
/// `pstr-player` needs an `Arc<Runtime>`, not a `Handle`: mpv's demuxer thread
/// blocks on it from outside tokio, so it has to be kept alive explicitly.
fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pstr=info,pstr_core=info".into()),
        )
        .init();

    let runtime = std::sync::Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("build the tokio runtime")?,
    );

    runtime.clone().block_on(run(runtime))
}

async fn run(runtime: std::sync::Arc<tokio::runtime::Runtime>) -> Result<()> {
    let cli = Cli::parse();
    let dirs = AppDirs::ensure().context("resolve app directories")?;
    let store = ShareStore::new(dirs.clone());

    match cli.command {
        Command::Add {
            name,
            url,
            custom_password,
        } => {
            let password = if custom_password {
                Some(read_password("Custom link password: ")?)
            } else {
                None
            };
            let share = store
                .add(&name, &url, password.as_deref())
                .context("add share")?;
            println!("added {} ({})", share.name, share.id);
        }

        Command::Shares => {
            let shares = store.list().context("list shares")?;
            if shares.is_empty() {
                println!("no shares configured — add one with `pstr add --name <name> <url>`");
            }
            for share in shares {
                let custom = if share.has_custom_password {
                    " [custom password]"
                } else {
                    ""
                };
                println!("{}\t{}{}", share.id, share.name, custom);
            }
        }

        Command::Remove { id } => {
            store.remove(&id).context("remove share")?;
            println!("removed {id}");
        }

        Command::Crawl { share } => {
            let (library, failures) = SharedLibrary::open_all(&store)
                .await
                .context("open shares")?;
            for (share, error) in &failures {
                eprintln!("[warn] share {} did not open: {error}", share.id);
            }

            let mut catalog = Catalog::open(&dirs.catalog_db()).context("open catalog")?;

            let targets: Vec<String> = match &share {
                Some(id) => vec![id.clone()],
                None => library.share_ids().map(str::to_string).collect(),
            };

            for share_id in targets {
                let started = std::time::Instant::now();
                let nodes = library
                    .crawl(&share_id)
                    .await
                    .with_context(|| format!("crawl {share_id}"))?;

                let rows = build_rows(&share_id, &nodes);

                catalog
                    .replace_share(&share_id, &rows)
                    .with_context(|| format!("store {share_id}"))?;

                println!(
                    "{share_id}: {} nodes crawled, {} playable, in {:.1}s",
                    nodes.len(),
                    rows.len(),
                    started.elapsed().as_secs_f64()
                );
            }
        }

        Command::List { share } => {
            let catalog = Catalog::open(&dirs.catalog_db()).context("open catalog")?;
            let files = match &share {
                Some(id) => catalog.files(id)?,
                None => catalog.all_files()?,
            };

            if files.is_empty() {
                println!("catalog is empty — run `pstr crawl` first");
            }
            for file in files {
                let numbering = match (file.parsed.season, file.parsed.episode) {
                    (Some(s), Some(e)) => format!("S{s:02}E{e:02}"),
                    (None, Some(e)) => format!("#{e:03}"),
                    (Some(s), None) => format!("S{s:02}"),
                    (None, None) => file
                        .parsed
                        .year
                        .map(|y| format!("({y})"))
                        .unwrap_or_else(|| "-".into()),
                };
                let episode_title = file.parsed.episode_title.as_deref().unwrap_or("-");
                println!(
                    "{:<40} {:<8} {}",
                    file.parsed.title, numbering, episode_title
                );
            }
        }

        Command::Metadata { command } => metadata(&dirs, command).await?,

        Command::Bench {
            file,
            share,
            readahead,
            ring_mib,
            disk_cache,
            throughput_mib,
            verify,
        } => {
            bench(
                &dirs,
                &store,
                BenchArgs {
                    file,
                    share,
                    readahead,
                    ring_mib,
                    disk_cache,
                    throughput_mib,
                    verify,
                },
            )
            .await?;
        }

        Command::Play {
            file,
            share,
            readahead,
            ring_mib,
            disk_cache,
            start,
            headless,
            for_seconds,
        } => {
            play(
                &dirs,
                &store,
                runtime,
                PlayArgs {
                    file,
                    share,
                    readahead,
                    ring_mib,
                    disk_cache,
                    start,
                    headless,
                    for_seconds,
                },
            )
            .await?;
        }
    }

    Ok(())
}

/// `pstr metadata …` — the enrichment settings, and matching the catalog.
///
/// Nothing here touches Proton: it reads titles out of the local catalog and
/// talks to the provider. That separation is the point — when the poster wall is
/// wrong, this says whether the matching or the crawl is to blame.
async fn metadata(dirs: &AppDirs, command: MetadataCommand) -> Result<()> {
    use pstr_core::library::Library;
    use pstr_core::metadata::ProviderId;

    let mut settings = pstr_meta::settings::load(dirs).context("read metadata settings")?;

    match command {
        MetadataCommand::Status => {
            println!("lookups:  {}", if settings.enabled { "on" } else { "off" });
            println!(
                "provider: {} — {}",
                settings.provider.label(),
                settings.provider.description()
            );
            if settings.provider.needs_api_key() {
                println!(
                    "api key:  {}",
                    match pstr_meta::settings::api_key(settings.provider) {
                        Some(_) => "stored in the credential store",
                        None => "not set — run `pstr metadata set --api-key`",
                    }
                );
            }

            let catalog = Catalog::open(&dirs.catalog_db()).context("open catalog")?;
            let stored = catalog.all_metadata()?;
            let matched = stored
                .values()
                .filter(|record| record.metadata.is_some())
                .count();
            println!(
                "stored:   {matched} matched, {} with no match",
                stored.len() - matched
            );
        }

        MetadataCommand::Set {
            on,
            off,
            provider,
            api_key,
        } => {
            if let Some(name) = provider {
                settings.provider = ProviderId::parse(&name).ok_or_else(|| {
                    anyhow::anyhow!(
                        "unknown provider {name:?}; try one of: {}",
                        ProviderId::ALL.map(|provider| provider.as_str()).join(", ")
                    )
                })?;
            }
            if on {
                settings.enabled = true;
            }
            if off {
                settings.enabled = false;
            }

            if api_key {
                let key = rpassword::prompt_password(format!(
                    "{} API key (empty to forget it): ",
                    settings.provider.label()
                ))
                .context("read the API key")?;
                pstr_meta::settings::set_api_key(settings.provider, &key)
                    .context("store the API key")?;
            }

            pstr_meta::settings::save(dirs, &settings).context("save metadata settings")?;

            // Turning it off, or switching provider, drops the stored answers —
            // the same thing the GUI does, for the same reason. See
            // `Engine::set_metadata_config`.
            if off || settings.enabled {
                let catalog = Catalog::open(&dirs.catalog_db()).context("open catalog")?;
                let stale = catalog
                    .all_metadata()?
                    .values()
                    .any(|record| record.provider != settings.provider);
                if off || stale {
                    catalog.clear_metadata()?;
                    println!("cleared the stored answers");
                }
            }
            println!(
                "lookups {} · provider {}",
                if settings.enabled { "on" } else { "off" },
                settings.provider.label()
            );
        }

        MetadataCommand::Match { force, limit } => {
            if !settings.enabled {
                anyhow::bail!(
                    "lookups are off — run `pstr metadata set --on` first.\n\
                     Matching sends the titles in your catalog to {}.",
                    settings.provider.label()
                );
            }

            let service = pstr_meta::MetadataService::new(
                &settings,
                pstr_meta::settings::api_key(settings.provider),
            )
            .context("build the metadata provider")?;

            let catalog = Catalog::open(&dirs.catalog_db()).context("open catalog")?;
            let library = Library::build(catalog.all_files()?, &catalog.all_watch_states()?);
            let stored = catalog.all_metadata()?;

            let mut pending: Vec<_> = library
                .titles
                .iter()
                .filter(|title| {
                    force
                        || !pstr_meta::service::is_usable(stored.get(&title.key), settings.provider)
                })
                .collect();
            if let Some(limit) = limit {
                pending.truncate(limit);
            }

            if pending.is_empty() {
                println!("nothing to do — every title already has a fresh answer");
                return Ok(());
            }
            println!(
                "matching {} titles against {}…",
                pending.len(),
                settings.provider.label()
            );

            // Serial, not concurrent: both providers rate-limit by the minute,
            // and the GUI already has the parallel path. What this command is
            // for is reading the output.
            let (mut matched, mut failed) = (0usize, 0usize);
            for title in pending {
                match service.record(title).await {
                    Ok(record) => {
                        match &record.metadata {
                            Some(found) => {
                                matched += 1;
                                println!(
                                    "  {:<40} → {} ({})",
                                    title.name,
                                    found.name,
                                    found
                                        .year
                                        .map(|year| year.to_string())
                                        .unwrap_or_else(|| "-".into())
                                );
                            }
                            None => println!("  {:<40} → no match", title.name),
                        }
                        catalog.set_metadata(&record)?;
                    }
                    Err(error) => {
                        failed += 1;
                        // Not stored: a failure to ask is not an answer, and the
                        // title has to stay askable. See `pstr_meta::service`.
                        println!("  {:<40} ! {error}", title.name);
                    }
                }
            }
            println!("{matched} matched, {failed} could not be looked up");
        }
    }

    Ok(())
}

struct BenchArgs {
    file: String,
    share: Option<String>,
    readahead: usize,
    ring_mib: u64,
    disk_cache: bool,
    throughput_mib: u64,
    verify: bool,
}

const MIB: u64 = 1024 * 1024;

/// Read chunk size. Small on purpose: mpv's demuxer reads in tens of kilobytes,
/// so a benchmark using 4 MiB reads would flatter the block layer by accident.
const CHUNK: u64 = 64 * 1024;

/// Everything `bench` and `play` both need before they can read a byte.
struct Opened {
    name: String,
    share_id: String,
    /// Held so the shares stay open for the life of the stream.
    library: std::sync::Arc<SharedLibrary>,
    source: StreamSource,
    uid: NodeUid,
    stream: pstr_stream::VideoStream,
    /// How long the cold open took: link details, the ancestor-key unlock, the
    /// revision listing. Everything before the first byte.
    open_ms: f64,
}

/// Resolve a name fragment to a catalogued file and open a stream on it.
async fn open_target(
    dirs: &AppDirs,
    store: &ShareStore,
    file: &str,
    share: Option<&str>,
    config: StreamConfig,
) -> Result<Opened> {
    let catalog = Catalog::open(&dirs.catalog_db()).context("open catalog")?;
    let files = match share {
        Some(id) => catalog.files(id)?,
        None => catalog.all_files()?,
    };

    let needle = file.to_lowercase();
    let target = files
        .into_iter()
        .find(|node| {
            node.name.to_lowercase().contains(&needle)
                || node.parsed.title.to_lowercase().contains(&needle)
        })
        .ok_or_else(|| anyhow::anyhow!("no catalogued file matches {file:?}"))?;

    println!("file:   {} ({})", target.name, target.share_id);

    let (library, failures) = SharedLibrary::open_all(store)
        .await
        .context("open shares")?;
    for (share, error) in &failures {
        eprintln!("[warn] share {} did not open: {error}", share.id);
    }

    let library = std::sync::Arc::new(library);
    let opener = std::sync::Arc::new(LibraryOpener::new(std::sync::Arc::clone(&library)));
    let source = StreamSource::new(opener, config).await?;

    let uid = NodeUid::new(
        VolumeId::new(target.volume_id.clone()),
        LinkId::new(target.link_id.clone()),
    );

    let started = std::time::Instant::now();
    let stream = source.open(&target.share_id, &uid).await?;
    let open_ms = started.elapsed().as_secs_f64() * 1000.0;

    Ok(Opened {
        name: target.name,
        share_id: target.share_id,
        library,
        source,
        uid,
        stream,
        open_ms,
    })
}

/// Build a stream config from the flags `bench` and `play` share.
fn stream_config(
    dirs: &AppDirs,
    readahead: usize,
    ring_mib: u64,
    disk_cache: bool,
) -> StreamConfig {
    let config = StreamConfig::default()
        .with_readahead(readahead)
        .with_ring_bytes(ring_mib * MIB);
    if disk_cache {
        config.with_disk_cache(DiskCacheConfig::new(dirs.block_cache()))
    } else {
        config
    }
}

async fn bench(dirs: &AppDirs, store: &ShareStore, args: BenchArgs) -> Result<()> {
    let config = stream_config(dirs, args.readahead, args.ring_mib, args.disk_cache);
    // 1. Cold open.
    let opened = open_target(dirs, store, &args.file, args.share.as_deref(), config).await?;
    let Opened {
        library,
        source,
        uid,
        stream,
        open_ms,
        share_id,
        ..
    } = opened;

    let size = stream.size();
    println!(
        "size:   {:.1} MiB in {} blocks",
        size as f64 / MIB as f64,
        stream.block_sizes().len()
    );
    println!("open:   {open_ms:.0} ms");

    if size == 0 {
        anyhow::bail!("that revision has no content to read");
    }

    // 2. Time to first byte, from the position a player actually starts at.
    let started = std::time::Instant::now();
    let head = stream.read_range(0, CHUNK).await?;
    println!(
        "first byte: {:.0} ms ({} bytes)",
        started.elapsed().as_secs_f64() * 1000.0,
        head.len()
    );

    // 3. Cold seeks. Distinct fractions of the file, so each lands in a block
    //    nothing has touched — this is the seek-bar drag.
    println!("cold seeks:");
    let mut worst = 0.0_f64;
    for percent in [10, 25, 50, 75, 90] {
        let offset = size * percent / 100;
        let started = std::time::Instant::now();
        let bytes = stream.read_range(offset, CHUNK).await?;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        worst = worst.max(ms);
        println!("  {percent:>3}%  {ms:>7.0} ms  {} bytes", bytes.len());
    }
    println!("  worst {worst:.0} ms");

    // 4. Sustained sequential throughput, in player-sized reads. Started past
    //    everything the seeks warmed.
    let start_at = (size * 60 / 100).min(size.saturating_sub(1));
    let want = (args.throughput_mib * MIB).min(size - start_at);
    let started = std::time::Instant::now();
    let mut read = 0_u64;
    let mut peak_resident = 0_u64;
    let mut buf = vec![0_u8; CHUNK as usize];

    while read < want {
        let got = stream.read_at(start_at + read, &mut buf).await? as u64;
        if got == 0 {
            break;
        }
        read += got;
        peak_resident = peak_resident.max(source.ring_stats().resident_bytes);
    }
    let seconds = started.elapsed().as_secs_f64();
    println!(
        "throughput: {:.1} MiB/s ({:.1} MiB in {seconds:.1}s)",
        read as f64 / MIB as f64 / seconds,
        read as f64 / MIB as f64
    );

    // 5. What it cost to hold.
    let stats = stream.stats();
    println!(
        "memory: {:.0} MiB peak resident, {} blocks, budget {} MiB",
        peak_resident as f64 / MIB as f64,
        stats.ring.blocks,
        args.ring_mib
    );
    println!(
        "blocks: {} fetched ({:.1} MiB), ring {} hit / {} miss, {} evicted",
        stats.blocks_fetched,
        stats.bytes_fetched as f64 / MIB as f64,
        stats.ring.hits,
        stats.ring.misses,
        stats.ring.evictions
    );
    println!(
        "seeks:  {} detected, {} read-ahead started, {} cancelled",
        stats.seeks, stats.readahead_blocks, stats.readahead_cancelled
    );
    if let Some(disk) = stats.disk {
        println!(
            "disk:   {} hit / {} miss, {} rejected, {:.0} MiB stored in {} entries",
            disk.hits,
            disk.misses,
            disk.rejected,
            disk.stored_bytes as f64 / MIB as f64,
            disk.entries
        );
    }

    // 6. Correctness, not speed: the same range straight from the SDK.
    if args.verify {
        let client = library
            .client(&share_id)
            .ok_or_else(|| anyhow::anyhow!("share {} is not open", share_id))?;

        let offset = size / 2;
        let length = CHUNK.min(size - offset);
        let expected = client.download_range(&uid, offset, length).await?;
        let actual = stream.read_range(offset, length).await?;

        anyhow::ensure!(
            expected == actual,
            "stream returned {} bytes that differ from the SDK's own range read at {offset}",
            actual.len()
        );
        println!("verify: {length} bytes at {offset} match the SDK's range download");
    }

    Ok(())
}

struct PlayArgs {
    file: String,
    share: Option<String>,
    readahead: usize,
    ring_mib: u64,
    disk_cache: bool,
    start: Option<f64>,
    headless: bool,
    for_seconds: Option<f64>,
}

/// Play a catalogued file through mpv.
///
/// This is the whole stack end to end: catalog → share client → block layer →
/// `pstr://` → mpv's demuxer → decode. Nothing is downloaded first; mpv reads
/// the file where it lies.
async fn play(
    dirs: &AppDirs,
    store: &ShareStore,
    runtime: std::sync::Arc<tokio::runtime::Runtime>,
    args: PlayArgs,
) -> Result<()> {
    let config = stream_config(dirs, args.readahead, args.ring_mib, args.disk_cache);
    let opened = open_target(dirs, store, &args.file, args.share.as_deref(), config).await?;

    let size = opened.stream.size();
    if size == 0 {
        anyhow::bail!("that revision has no content to play");
    }
    println!(
        "size:   {:.1} MiB in {} blocks",
        size as f64 / MIB as f64,
        opened.stream.block_sizes().len()
    );
    println!("open:   {:.0} ms", opened.open_ms);

    let mut player_config = PlayerConfig {
        window_title: format!("proton-stream — {}", opened.name),
        ..PlayerConfig::default()
    };
    if args.headless {
        // Still demuxes and decodes — the whole chain runs, the frames and
        // samples are just discarded. That is the point: it exercises playback
        // somewhere there is no display to open a window on.
        for option in [("vo", "null"), ("ao", "null"), ("force-window", "no")] {
            player_config
                .options
                .push((option.0.into(), option.1.into()));
        }
    }

    let player = Player::new(runtime, player_config).context("start mpv")?;
    let _handle = player.play(opened.stream.clone()).context("load stream")?;

    let mut seeked = args.start.is_none();
    let mut last_line = std::time::Instant::now();
    let started = std::time::Instant::now();
    let mut position = 0.0_f64;
    let mut duration = 0.0_f64;
    // Seek issued → picture back. The number the seek bar is judged on, and the
    // one worth watching while dragging it in the window.
    let mut seek_started: Option<std::time::Instant> = None;
    let mut first_frame: Option<f64> = None;

    loop {
        if let Some(limit) = args.for_seconds
            && started.elapsed().as_secs_f64() >= limit
        {
            println!("\nstopped after {limit:.0}s at {position:.0}s");
            break;
        }

        let Some(event) = player.poll_event(0.1) else {
            continue;
        };
        match event {
            PlayerEvent::FileLoaded => {
                duration = player.duration().unwrap_or(0.0);
                println!("loaded: {duration:.0}s");
                // Seek only once demuxing has succeeded — before that mpv has
                // no timeline to seek within.
                if let (false, Some(start)) = (seeked, args.start) {
                    player.seek_to(start).context("seek to --start")?;
                    seeked = true;
                }
            }
            PlayerEvent::Duration(value) => duration = value,
            PlayerEvent::Seek => seek_started = Some(std::time::Instant::now()),
            PlayerEvent::PlaybackRestart => {
                if let Some(at) = seek_started.take() {
                    println!("\nseek:   resumed in {:.0} ms", at.elapsed().as_millis());
                }
            }
            PlayerEvent::Position(value) => {
                if first_frame.is_none() {
                    first_frame = Some(started.elapsed().as_secs_f64());
                    println!("first frame: {:.1}s after launch", first_frame.unwrap());
                }
                position = value;
                // Overwrite one line rather than scrolling: this runs at
                // whatever rate mpv reports, which is often.
                if last_line.elapsed().as_secs_f64() >= 1.0 {
                    last_line = std::time::Instant::now();
                    print!("\r  {position:.0}s / {duration:.0}s          ");
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
            }
            PlayerEvent::EndFile(reason) => {
                println!("\nended:  {reason:?} at {position:.0}s");
                if reason != EndReason::Quit {
                    break;
                }
            }
            PlayerEvent::Shutdown => {
                println!("\nclosed at {position:.0}s");
                break;
            }
            _ => {}
        }
    }

    let stats = opened.stream.stats();
    println!(
        "blocks: {} fetched ({:.1} MiB), ring {} hit / {} miss, {} evicted",
        stats.blocks_fetched,
        stats.bytes_fetched as f64 / MIB as f64,
        stats.ring.hits,
        stats.ring.misses,
        stats.ring.evictions
    );
    println!(
        "seeks:  {} detected, {} read-ahead started, {} cancelled",
        stats.seeks, stats.readahead_blocks, stats.readahead_cancelled
    );
    if let Some(disk) = stats.disk {
        println!(
            "disk:   {} hit / {} miss, {} rejected, {:.0} MiB stored in {} entries",
            disk.hits,
            disk.misses,
            disk.rejected,
            disk.stored_bytes as f64 / MIB as f64,
            disk.entries
        );
    }

    opened.source.close(&opened.share_id, &opened.uid);
    drop(opened.library);
    Ok(())
}
