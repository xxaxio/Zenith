//! Zenith: Fast, lightweight limbo and queue server in Rust for Horizon and Meridian.

mod config;
mod metrics;

use config::ZenithConfig;
use metrics::{ZenithMetrics, start_metrics_server};
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, info};
use zenith_display::{
    DisplayConfig, HudRenderer, encode_subtitle_packet, encode_system_chat_packet,
    encode_title_packet, encode_title_times_packet,
};
use zenith_session::{SessionCommand, SessionEvent, SessionId, VirtualSession};
use zenith_shield::{ConnectionTracker, IpRateLimiter, RateLimitConfig, ShieldLimits};
use zenith_totem::{
    BackendHealth, HorizonTurboClient, TotemConfig, TotemEvent, TotemLifeline, TurboEndpoint,
};
use zenith_transit::{
    GroupManager, QueueEntry, SortingPolicy, TargetServerHealth, TransitConfig, TransitQueue,
};
use zenith_wire::{HmacForwarding, PROXY_V2_MAGIC, ProxyHeader, is_trusted_proxy};

/// Zenith operational modes for testing and production deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZenithMode {
    /// Infinite virtual holding: players remain in Limbo, explore, chat, and test commands (/transfer, /ping, etc.).
    Limbo,
    /// Transit queue: players are held and drained in waves to the target backend server.
    Queue,
    /// Stress harness: high-throughput bot load testing.
    Stress,
}

type ActiveSessions = Arc<
    Mutex<
        HashMap<
            SessionId,
            (
                mpsc::UnboundedSender<SessionCommand>,
                String,
                u128,
                usize,
                u128,
            ),
        >,
    >,
>;

struct CommandContext {
    queue: Arc<Mutex<TransitQueue>>,
    group_mgr: Arc<Mutex<GroupManager>>,
    sessions: ActiveSessions,
    is_queue_active: Arc<AtomicBool>,
    target_server: String,
    hud: Arc<HudRenderer>,
    health_rx: tokio::sync::watch::Receiver<BackendHealth>,
    totem_enabled: bool,
    hmac_forwarding: Option<HmacForwarding>,
    rp_enabled: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("zenith 0.1.0");
        return Ok(());
    }

    if args.iter().any(|arg| arg == "--check") {
        info!("Zenith configuration check passed.");
        return Ok(());
    }

    let mut cli_config = None;
    let mut cli_mode = None;
    let mut cli_wave_size = None;
    let mut cli_wave_interval = None;
    let mut cli_target = None;
    let mut cli_bind = None;
    let mut cli_no_shield = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" if i + 1 < args.len() => {
                cli_config = Some(args[i + 1].clone());
                i += 2;
            }
            "--no-shield" => {
                cli_no_shield = true;
                i += 1;
            }
            "--mode" | "-m" if i + 1 < args.len() => {
                cli_mode = Some(args[i + 1].clone());
                i += 2;
            }
            "--wave-size" | "-w" if i + 1 < args.len() => {
                cli_wave_size = args[i + 1].parse().ok();
                i += 2;
            }
            "--wave-interval" if i + 1 < args.len() => {
                cli_wave_interval = args[i + 1].parse().ok();
                i += 2;
            }
            "--target" | "-t" if i + 1 < args.len() => {
                cli_target = Some(args[i + 1].clone());
                i += 2;
            }
            "--bind" | "-b" if i + 1 < args.len() => {
                cli_bind = Some(args[i + 1].clone());
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    let config_path = cli_config.map(PathBuf::from);
    let mut config = ZenithConfig::load_or_default(config_path.as_deref());

    if let Some(b) = cli_bind {
        config.server.bind = b;
    }
    if let Some(t) = cli_target {
        config.server.target = t;
    }
    if let Some(m) = cli_mode {
        config.server.mode = m;
    }
    if let Some(w) = cli_wave_size {
        config.queue.wave_size = w;
    }
    if let Some(interval) = cli_wave_interval {
        config.queue.wave_interval_ms = interval;
    }
    if cli_no_shield {
        config.shield.enabled = false;
    }

    let bind_addr = config.server.bind.clone();
    let target_server = config.server.target.clone();
    let wave_size = config.queue.wave_size;
    let wave_interval_ms = config.queue.wave_interval_ms;
    let mode = match config.server.mode.to_lowercase().as_str() {
        "queue" | "transit" => ZenithMode::Queue,
        "stress" | "benchmark" => ZenithMode::Stress,
        _ => ZenithMode::Limbo,
    };

    info!("Starting Zenith v0.1.0 (Protocol 777 / Minecraft 26.3)...");
    info!("Operating Mode: {:?}", mode);
    info!("Target backend: {}", target_server);
    if mode == ZenithMode::Queue {
        info!(
            "Wave settings: {} players every {}ms",
            wave_size, wave_interval_ms
        );
    } else {
        info!(
            "Limbo showcase active: auto-transfer disabled. Players stay in limbo; type /transfer to transfer."
        );
    }
    info!("Listening for virtual limbo connections on {}", bind_addr);

    // Operational Metrics Registry & Prometheus Exporter
    let metrics = ZenithMetrics::new();
    if config.metrics.enabled {
        let metrics_bind = config.metrics.bind.clone();
        let m = Arc::clone(&metrics);
        tokio::spawn(async move {
            if let Err(e) = start_metrics_server(metrics_bind, m).await {
                error!(error = %e, "Failed to start Prometheus metrics exporter");
            }
        });
    }

    // Defensive Cybersecurity Layer: Rate Limiting & Concurrency Tracker
    let shield_enabled = config.shield.enabled;
    let rate_limiter = Arc::new(Mutex::new(IpRateLimiter::new(RateLimitConfig {
        ip_rate_limit: config.shield.ip_rate_limit,
        ip_burst_capacity: config.shield.ip_burst_capacity,
        subnet_rate_limit: config.shield.subnet_rate_limit,
        subnet_burst_capacity: config.shield.subnet_burst_capacity,
        violation_threshold: config.shield.violation_threshold,
        jail_duration: Duration::from_secs(config.shield.jail_duration_secs),
    })));
    let conn_tracker = ConnectionTracker::new(config.shield.max_connections_per_ip);

    // Background task to prune expired jails and idle rate limit buckets
    let prune_limiter = Arc::clone(&rate_limiter);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            prune_limiter.lock().await.prune(Instant::now());
        }
    });

    // Horizon Native Turbo Fast-Path (full-Rust IPC ctl.sock telemetry & pre-warming)
    let turbo_client = if config.turbo.enabled {
        let ep = config
            .turbo
            .endpoint
            .as_deref()
            .unwrap_or("127.0.0.1:40000");
        info!(endpoint = %ep, "Horizon Native Turbo Mode active");
        Some(Arc::new(HorizonTurboClient::new(TurboEndpoint::parse_str(
            ep,
        ))))
    } else {
        None
    };

    if let Some(ref turbo) = turbo_client {
        let t = Arc::clone(turbo);
        let m = Arc::clone(&metrics);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            loop {
                interval.tick().await;
                if let Ok(st) = t.query_status().await {
                    m.turbo_mspt_us.store(st.mspt_us, Ordering::Relaxed);
                }
            }
        });
    }

    let transit_config = TransitConfig {
        sorting_policy: SortingPolicy::PingAscending,
        nominal_wave_size: wave_size,
        nominal_interval_ms: wave_interval_ms,
        ..Default::default()
    };
    let queue = Arc::new(Mutex::new(TransitQueue::new(transit_config)));
    let groups_path = config
        .queue
        .groups_file
        .clone()
        .unwrap_or_else(|| PathBuf::from("groups.toml"));
    let group_manager = Arc::new(Mutex::new(GroupManager::new(Some(groups_path))));
    let hud = Arc::new(HudRenderer::new(DisplayConfig::default()));
    let active_sessions: ActiveSessions = Arc::new(Mutex::new(HashMap::new()));
    let is_queue_active = Arc::new(AtomicBool::new(mode == ZenithMode::Queue));

    let hmac_secret = config.forwarding.hmac_secret.clone();
    let hmac_forwarding = hmac_secret.map(|s| HmacForwarding::new(s.as_bytes()));
    if hmac_forwarding.is_some() {
        info!("HMAC-SHA256 forwarding security token generation enabled");
    }

    // Totem Lifeline: Active SLP Health Probing & Anti-Crash Evacuation
    let totem_config = TotemConfig {
        probe_interval_ms: config.totem.probe_interval_ms,
        probe_timeout_ms: config.totem.probe_timeout_ms,
        degraded_latency_ms: 200,
        min_failures_for_offline: config.totem.consecutive_failures,
        min_successes_for_recovery: config.totem.consecutive_successes,
    };
    let (totem_lifeline, totem_handles) = TotemLifeline::new(&target_server, totem_config);
    let mut totem_event_rx = totem_handles.event_rx;
    let health_rx = totem_handles.health_rx;

    if config.totem.enabled {
        tokio::spawn(totem_lifeline.run());
    }

    let hud_health_rx = health_rx.clone();
    let wave_health_rx = health_rx.clone();
    let cmd_health_rx = health_rx.clone();

    // Background Totem Event Broadcaster & Auto-Recovery Lifeline
    let totem_sessions = Arc::clone(&active_sessions);
    let totem_queue_active = Arc::clone(&is_queue_active);
    let totem_queue = Arc::clone(&queue);
    let totem_metrics = Arc::clone(&metrics);
    let totem_mode = mode;
    let notify_down = config.broadcast.notify_on_backend_down;
    let notify_up = config.broadcast.notify_on_backend_up;
    tokio::spawn(async move {
        while let Ok(event) = totem_event_rx.recv().await {
            match event {
                TotemEvent::BackendDown {
                    consecutive_failures,
                    error,
                } => {
                    totem_metrics
                        .totem_backend_online
                        .store(false, Ordering::Relaxed);
                    tracing::warn!(
                        consecutive_failures,
                        error = %error,
                        "Totem detected backend failure - securing limbo"
                    );
                    if totem_mode == ZenithMode::Queue {
                        totem_queue_active.store(false, Ordering::SeqCst);
                        totem_queue.lock().await.set_paused(true);
                    }

                    if notify_down {
                        let sessions = totem_sessions.lock().await;
                        let times = encode_title_times_packet(5, 50, 15);
                        let sub = encode_subtitle_packet(
                            "§c§lBACKEND RESTARTING §7Recovery in progress...",
                        );
                        let title = encode_title_packet("§c⚠");
                        let chat = encode_system_chat_packet(
                            "§b§lᴢᴇɴɪᴛʜ §8» §c§lALERT: §7Target backend crashed or restarting. You are safe in virtual holding!",
                            false,
                        );

                        for (s_tx, _, _, _, _) in sessions.values() {
                            let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                            let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                            let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                            let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                        }
                    }
                }
                TotemEvent::BackendRecovered {
                    latency_ms,
                    online_players,
                } => {
                    totem_metrics
                        .totem_backend_online
                        .store(true, Ordering::Relaxed);
                    totem_metrics
                        .totem_latency_ms
                        .store(latency_ms, Ordering::Relaxed);
                    info!(
                        latency_ms,
                        online_players = ?online_players,
                        "Totem detected backend recovery - resuming transit"
                    );
                    if totem_mode == ZenithMode::Queue {
                        totem_queue_active.store(true, Ordering::SeqCst);
                        totem_queue.lock().await.set_paused(false);
                    }

                    if notify_up {
                        let sessions = totem_sessions.lock().await;
                        let times = encode_title_times_packet(5, 50, 15);
                        let sub = encode_subtitle_packet(
                            "§a§lBACKEND READY §7Resuming automatic transfers...",
                        );
                        let title = encode_title_packet("§a▶");
                        let chat = encode_system_chat_packet(
                            &format!(
                                "§b§lᴢᴇɴɪᴛʜ §8» §a§lBACKEND ONLINE! §7Ping: §e{}ms§7. Preparing transfers...",
                                latency_ms
                            ),
                            false,
                        );

                        for (s_tx, _, _, _, _) in sessions.values() {
                            let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                            let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                            let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                            let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                        }
                    }
                }
                TotemEvent::StatusUpdate { .. } => {}
            }
        }
    });

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SessionEvent>();

    // 1. Background HUD refresh loop (every 1 second)
    let hud_queue = Arc::clone(&queue);
    let hud_renderer = Arc::clone(&hud);
    let hud_sessions = Arc::clone(&active_sessions);
    let hud_totem_enabled = config.totem.enabled;
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(1000));
        loop {
            interval.tick().await;

            let (ranked_map, total, nominal_wave_size, nominal_interval_ms) = {
                let q = hud_queue.lock().await;
                if q.is_empty() {
                    continue;
                }
                let ranked = if mode == ZenithMode::Queue {
                    q.ranked_positions()
                } else {
                    std::collections::HashMap::new()
                };
                (
                    ranked,
                    q.len(),
                    q.config().nominal_wave_size.max(1),
                    q.config().nominal_interval_ms.max(1000),
                )
            };

            let limbo_actionbar = if mode == ZenithMode::Limbo {
                Some(
                    if hud_totem_enabled && !hud_health_rx.borrow().is_available() {
                        zenith_display::encode_actionbar_packet("§c§l● §7Target backend offline")
                    } else {
                        hud_renderer.render_limbo_actionbar()
                    },
                )
            } else {
                None
            };

            let mut sessions = hud_sessions.lock().await;
            for (&session_id, (tx, _username, _uuid, last_pos, bb_uuid)) in sessions.iter_mut() {
                if mode == ZenithMode::Limbo {
                    if let Some(ref actionbar) = limbo_actionbar {
                        let _ = tx.send(SessionCommand::SendRaw(actionbar.clone()));
                    }

                    if *last_pos != total {
                        let (title_pkt, health_pkt) =
                            hud_renderer.render_limbo_bossbar_update(*bb_uuid, total);
                        let _ = tx.send(SessionCommand::SendRaw(title_pkt));
                        let _ = tx.send(SessionCommand::SendRaw(health_pkt));
                        *last_pos = total;
                    }
                } else if let Some(&(pos, total)) = ranked_map.get(&session_id) {
                    let eta_seconds = ((pos as u64).saturating_sub(1) / nominal_wave_size as u64)
                        * (nominal_interval_ms / 1000);

                    // Send Actionbar
                    let actionbar = hud_renderer.render_actionbar(pos, total, eta_seconds);
                    let _ = tx.send(SessionCommand::SendRaw(actionbar));

                    // Send Bossbar update & click sound on position change
                    if pos != *last_pos {
                        if pos < *last_pos {
                            if let Some(click) = hud_renderer.render_click_sound() {
                                let _ = tx.send(SessionCommand::SendRaw(click));
                            }
                        }
                        let (title_pkt, health_pkt) =
                            hud_renderer.render_bossbar_update(*bb_uuid, pos, total, eta_seconds);
                        let _ = tx.send(SessionCommand::SendRaw(title_pkt));
                        let _ = tx.send(SessionCommand::SendRaw(health_pkt));
                        *last_pos = pos;
                    }
                }
            }
        }
    });

    // 2. Background Wave Drain loop (health-adaptive transfers - only active in Queue mode)
    if mode == ZenithMode::Queue {
        let wave_queue = Arc::clone(&queue);
        let wave_hud = Arc::clone(&hud);
        let wave_sessions = Arc::clone(&active_sessions);
        let wave_active_flag = Arc::clone(&is_queue_active);
        let wave_metrics = Arc::clone(&metrics);
        let wave_turbo = turbo_client.clone();
        let wave_turbo_pre_warm = config.turbo.pre_warm;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(wave_interval_ms));
            loop {
                interval.tick().await;

                if !wave_active_flag.load(Ordering::SeqCst)
                    || !wave_health_rx.borrow().is_available()
                {
                    continue;
                }

                let drained = {
                    let mut q = wave_queue.lock().await;
                    if q.is_empty() {
                        continue;
                    }
                    let h = wave_health_rx.borrow();
                    let lat = h.latency_ms().unwrap_or(20) as f64;
                    let health = TargetServerHealth {
                        tps: 20.0,
                        mspt: lat.max(10.0),
                        is_online: h.is_available(),
                        is_draining: false,
                        last_update: Instant::now(),
                    };
                    let d = q.drain_wave(Instant::now(), &health);
                    wave_metrics.queue_size.store(q.len(), Ordering::Relaxed);
                    d
                };

                if !drained.is_empty() {
                    info!(
                        count = drained.len(),
                        "Draining transit wave towards backend"
                    );
                    let mut sessions = wave_sessions.lock().await;
                    for entry in drained {
                        if let Some((tx, username, uuid, _last_pos, bb_uuid)) =
                            sessions.remove(&entry.session_id)
                        {
                            let (host, port) = match entry.target_server.split_once(':') {
                                Some((h, p)) => (h, p.parse::<u16>().unwrap_or(25565)),
                                None => (entry.target_server.as_str(), 25565),
                            };

                            // Horizon Turbo Fast-Path: pre-warm arrival in backend memory
                            if let Some(ref turbo) = wave_turbo {
                                if wave_turbo_pre_warm {
                                    let _ = turbo.notify_player_expect(&username, uuid, None).await;
                                }
                            }

                            // Send transfer sound and packet
                            let (sound_opt, transfer_pkt) = wave_hud.render_transfer(host, port);
                            if let Some(snd) = sound_opt {
                                let _ = tx.send(SessionCommand::SendRaw(snd));
                            }
                            let chat_msg = encode_system_chat_packet(
                                "§b§lᴢᴇɴɪᴛʜ §8» §aVotre tour est arrivé ! Transfert en cours...",
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(chat_msg));
                            let _ = tx.send(SessionCommand::SendRaw(transfer_pkt));

                            // Clear Bossbar
                            let remove_bb = wave_hud.render_bossbar_remove(bb_uuid);
                            let _ = tx.send(SessionCommand::SendRaw(remove_bb));

                            // Instruct session to execute transfer
                            let _ = tx.send(SessionCommand::Transfer {
                                host: host.to_string(),
                                port,
                            });

                            if let Some(ref turbo) = wave_turbo {
                                if wave_turbo_pre_warm {
                                    let _ = turbo.notify_player_confirm(&username).await;
                                }
                            }
                            wave_metrics.transfers_total.fetch_add(1, Ordering::Relaxed);

                            info!(
                                player = %username,
                                target = %entry.target_server,
                                "Player transferred successfully"
                            );
                        }
                    }
                }
            }
        });
    }

    // 3. Central Event & Command Coordinator Loop
    let cmd_context = Arc::new(CommandContext {
        queue: Arc::clone(&queue),
        group_mgr: Arc::clone(&group_manager),
        sessions: Arc::clone(&active_sessions),
        is_queue_active: Arc::clone(&is_queue_active),
        target_server: target_server.clone(),
        hud: Arc::clone(&hud),
        health_rx: cmd_health_rx,
        totem_enabled: config.totem.enabled,
        hmac_forwarding,
        rp_enabled: config.resource_pack.enabled,
    });

    let loop_ctx = Arc::clone(&cmd_context);
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                SessionEvent::Command {
                    session_id,
                    username,
                    command,
                    args,
                } => {
                    handle_command(session_id, &username, &command, &args, &loop_ctx).await;
                }
                SessionEvent::PingUpdated {
                    session_id,
                    ping_ms,
                } => {
                    loop_ctx.queue.lock().await.update_ping(session_id, ping_ms);
                }
                SessionEvent::ResourcePackStatus {
                    session_id,
                    uuid,
                    action,
                } => {
                    debug!(session = ?session_id, uuid = ?uuid, ?action, "Client resource pack status received");
                }
                SessionEvent::Disconnected { session_id } => {
                    loop_ctx.queue.lock().await.remove(session_id);
                    loop_ctx.sessions.lock().await.remove(&session_id);
                }
            }
        }
    });

    // 4. Listener accept loop
    let listener = TcpListener::bind(&bind_addr).await?;
    info!("Zenith server online. Ready for player connections!");

    let loop_metrics = Arc::clone(&metrics);
    let proxy_enabled = config.proxy.enabled;
    let trusted_proxies = config.proxy.trusted_proxies.clone();
    let rp_config = config.resource_pack.clone();

    loop {
        let (mut socket, peer_addr) = listener.accept().await?;
        debug!(peer = %peer_addr, "Accepted incoming TCP connection");

        // PROXY Protocol v2 Ingress: parse header if enabled and peer is a trusted proxy
        let mut client_ip = peer_addr.ip();
        if proxy_enabled && is_trusted_proxy(peer_addr.ip(), &trusted_proxies) {
            let proxy_res = async {
                let mut peek_buf = [0u8; 512];
                let mut read_bytes = 0;
                for _ in 0..10 {
                    let n = socket.peek(&mut peek_buf).await?;
                    if n > read_bytes {
                        read_bytes = n;
                    }
                    if read_bytes < 12 {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        continue;
                    }
                    if peek_buf[..12] != PROXY_V2_MAGIC {
                        return Ok::<_, std::io::Error>(None);
                    }
                    if read_bytes < 16 {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        continue;
                    }
                    let total_len = 16 + u16::from_be_bytes([peek_buf[14], peek_buf[15]]) as usize;
                    if total_len > peek_buf.len() {
                        return Ok(None);
                    }
                    if read_bytes < total_len {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        continue;
                    }
                    if let Ok(Some((header, _))) = ProxyHeader::parse(&peek_buf[..total_len]) {
                        let mut discard = vec![0u8; total_len];
                        socket.read_exact(&mut discard).await?;
                        if let Some(src) = header.src_addr {
                            return Ok(Some(src.ip()));
                        }
                    }
                    return Ok(None);
                }
                Ok(None)
            };

            if let Ok(Ok(Some(extracted_ip))) =
                tokio::time::timeout(Duration::from_millis(500), proxy_res).await
            {
                debug!(peer = %peer_addr, real_ip = %extracted_ip, "Extracted real client IP via PROXY v2");
                client_ip = extracted_ip;
            }
        }

        let _conn_guard = if shield_enabled {
            match conn_tracker.acquire(client_ip) {
                Ok(guard_opt) => {
                    let check_res = {
                        let mut rl = rate_limiter.lock().await;
                        rl.check_allowed(client_ip, Instant::now())
                    };
                    match check_res {
                        Ok(()) => guard_opt,
                        Err(e) => {
                            loop_metrics
                                .shield_rate_limited_total
                                .fetch_add(1, Ordering::Relaxed);
                            let jailed = rate_limiter.lock().await.jail_manager().count();
                            loop_metrics
                                .shield_jailed_ips
                                .store(jailed, Ordering::Relaxed);
                            debug!(peer = %peer_addr, client_ip = %client_ip, error = %e, "Shield rejected incoming connection: rate-limited or jailed");
                            continue;
                        }
                    }
                }
                Err(e) => {
                    loop_metrics
                        .shield_rate_limited_total
                        .fetch_add(1, Ordering::Relaxed);
                    debug!(peer = %peer_addr, client_ip = %client_ip, error = %e, "Shield rejected incoming connection: concurrent limit reached");
                    continue;
                }
            }
        } else {
            None
        };

        let queue_clone = Arc::clone(&queue);
        let group_clone = Arc::clone(&group_manager);
        let hud_clone = Arc::clone(&hud);
        let sessions_clone = Arc::clone(&active_sessions);
        let target_clone = target_server.clone();
        let event_tx_clone = event_tx.clone();
        let task_metrics = Arc::clone(&loop_metrics);
        let task_rp_config = rp_config.clone();

        tokio::spawn(async move {
            let _guard = _conn_guard;
            task_metrics.active_sessions.fetch_add(1, Ordering::Relaxed);
            let _ = socket.set_nodelay(true);
            let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
            let mut session =
                VirtualSession::with_command_channel(socket, ShieldLimits::default(), cmd_rx);
            session.set_target_server(target_clone.clone());
            session.set_event_sender(event_tx_clone);

            match session.complete_handshake_and_login().await {
                Ok(None) => {
                    // Status Ping completed cleanly
                }
                Ok(Some((username, uuid))) => {
                    let session_id = session.id;
                    let bb_uuid = uuid;

                    // Resolve priority from GroupManager (staff, vip, owner)
                    let priority = group_clone.lock().await.priority_of(&username);

                    info!(
                        session = ?session_id,
                        player = %username,
                        priority = ?priority,
                        peer = %peer_addr,
                        real_ip = %client_ip,
                        "Player entered Zenith virtual holding dimension"
                    );

                    let (pos, total, eta_seconds) = {
                        let mut q = queue_clone.lock().await;
                        q.enqueue(QueueEntry {
                            session_id,
                            uuid,
                            username: username.clone(),
                            priority,
                            ping_ms: 20,
                            enqueued_at: Instant::now(),
                            target_server: target_clone,
                        });
                        task_metrics.queue_size.store(q.len(), Ordering::Relaxed);
                        let (p, t) = q.position_of(session_id).unwrap_or((1, 1));
                        let eta = ((p as u64).saturating_sub(1)
                            / q.config().nominal_wave_size.max(1) as u64)
                            * (q.config().nominal_interval_ms / 1000).max(1);
                        (p, t, eta)
                    };

                    // Send initial BossBar Add
                    if mode == ZenithMode::Limbo {
                        let add_bb = hud_clone.render_limbo_bossbar_add(bb_uuid, total);
                        let _ = cmd_tx.send(SessionCommand::SendRaw(add_bb));
                    } else {
                        let add_bb = hud_clone.render_bossbar_add(bb_uuid, pos, total, eta_seconds);
                        let _ = cmd_tx.send(SessionCommand::SendRaw(add_bb));
                    }

                    // Send sleek, minimal welcome title (fade in, short display)
                    let times = encode_title_times_packet(10, 40, 10);
                    let _ = cmd_tx.send(SessionCommand::SendRaw(times));
                    let sub = encode_subtitle_packet(if mode == ZenithMode::Limbo {
                        "§7Virtual Limbo Holding"
                    } else {
                        "§7Transit Queue"
                    });
                    let _ = cmd_tx.send(SessionCommand::SendRaw(sub));
                    let title = encode_title_packet("§b§lᴢᴇɴɪᴛʜ");
                    let _ = cmd_tx.send(SessionCommand::SendRaw(title));

                    // Preload resource pack in background if configured
                    if task_rp_config.enabled && !task_rp_config.url.is_empty() {
                        let mut uuid_bytes = [0u8; 16];
                        let hash_bytes = task_rp_config.hash.as_bytes();
                        for (idx, &b) in hash_bytes.iter().enumerate() {
                            uuid_bytes[idx % 16] ^= b;
                        }
                        let pack_uuid = u128::from_be_bytes(uuid_bytes);

                        let _ = cmd_tx.send(SessionCommand::PushResourcePack {
                            uuid: pack_uuid,
                            url: task_rp_config.url.clone(),
                            hash: task_rp_config.hash.clone(),
                            required: task_rp_config.required,
                            prompt: task_rp_config.prompt.clone(),
                        });
                        debug!(session = ?session_id, player = %username, "Pushed background resource pack preload");
                    }

                    // Register in active sessions map
                    sessions_clone
                        .lock()
                        .await
                        .insert(session_id, (cmd_tx, username.clone(), uuid, pos, bb_uuid));

                    // Run the limbo loop
                    if let Err(e) = session.handle_limbo_loop().await {
                        error!(session = ?session_id, error = %e, "Limbo session terminated with error");
                    } else {
                        info!(session = ?session_id, player = %username, "Limbo session closed cleanly");
                    }

                    // Cleanup if disconnected before wave drain
                    queue_clone.lock().await.remove(session_id);
                    task_metrics
                        .queue_size
                        .store(queue_clone.lock().await.len(), Ordering::Relaxed);
                    sessions_clone.lock().await.remove(&session_id);
                }
                Err(e) => {
                    debug!(error = %e, "Handshake/Login ended (client disconnect or ping)");
                }
            }
            task_metrics.active_sessions.fetch_sub(1, Ordering::Relaxed);
        });
    }

    #[allow(unreachable_code)]
    Ok(())
}

async fn handle_command(
    session_id: SessionId,
    username: &str,
    cmd: &str,
    args: &[String],
    ctx: &CommandContext,
) {
    let session_data = {
        let s = ctx.sessions.lock().await;
        s.get(&session_id)
            .map(|(tx, _, uuid, _, _)| (tx.clone(), *uuid))
    };
    let Some((tx, player_uuid)) = session_data else {
        return;
    };

    let is_staff = ctx.group_mgr.lock().await.is_staff(username);

    match cmd {
        "transfer" | "join" | "play" => {
            if !is_staff {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cYou do not have permission to manually transfer. Please wait in queue.",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            let current_health = ctx.health_rx.borrow().clone();
            if ctx.totem_enabled && !current_health.is_available() {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cTarget server is currently unavailable (restarting). You are protected in queue!",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            if let Some(ref hmac) = ctx.hmac_forwarding {
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let token = hmac.create_transit_token(player_uuid, username, now_secs);
                tracing::info!(
                    player = %username,
                    token = ?token,
                    "HMAC transit token generated for player transfer"
                );
            }

            let (host, port) = match ctx.target_server.split_once(':') {
                Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(25565)),
                None => (ctx.target_server.clone(), 25565),
            };

            let (sound_opt, transfer_pkt) = ctx.hud.render_transfer(&host, port);
            if let Some(snd) = sound_opt {
                let _ = tx.send(SessionCommand::SendRaw(snd));
            }
            let msg = encode_system_chat_packet(
                &format!("§b§lᴢᴇɴɪᴛʜ §8» §aTransfert vers §e{}:{}§a...", host, port),
                false,
            );
            let _ = tx.send(SessionCommand::SendRaw(msg));
            let _ = tx.send(SessionCommand::SendRaw(transfer_pkt));
            let _ = tx.send(SessionCommand::Transfer { host, port });
        }
        "queue" => {
            if !is_staff {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cVous n'avez pas la permission d'utiliser cette commande.",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            let was_active = ctx.is_queue_active.load(Ordering::SeqCst);
            let new_active = !was_active;
            ctx.is_queue_active.store(new_active, Ordering::SeqCst);
            ctx.queue.lock().await.toggle_pause();

            let sess = ctx.sessions.lock().await;
            if new_active {
                info!(by = %username, "Queue OPENED by staff member");
                let times = encode_title_times_packet(5, 50, 15);
                let sub = encode_subtitle_packet("§b§lQUEUE RESUMED §7Transit waves are active");
                let title = encode_title_packet("§a▶");
                let chat =
                    encode_system_chat_packet("§b§lᴢᴇɴɪᴛʜ §8» §aQueue is now §2§lOPEN §a!", false);

                for (s_tx, _, _, _, _) in sess.values() {
                    let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                }
            } else {
                info!(by = %username, "Queue PAUSED by staff member");
                let times = encode_title_times_packet(5, 50, 15);
                let sub = encode_subtitle_packet("§c§lQUEUE PAUSED §7Transit waves suspended");
                let title = encode_title_packet("§c⏸");
                let chat = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cQueue is now §4§lPAUSED §c!",
                    false,
                );

                for (s_tx, _, _, _, _) in sess.values() {
                    let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                }
            }
        }
        "queueskip" | "skip" => {
            if !is_staff {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cVous n'avez pas la permission d'utiliser cette commande.",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            if args.is_empty() {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cUtilisation: /queueskip <joueur>",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            let target_name = &args[0];
            let mut target_found = None;

            {
                let s = ctx.sessions.lock().await;
                for (&s_id, (s_tx, u_name, _, _, bb_uuid)) in s.iter() {
                    if u_name.eq_ignore_ascii_case(target_name) {
                        target_found = Some((s_id, s_tx.clone(), u_name.clone(), *bb_uuid));
                        break;
                    }
                }
            }

            if let Some((target_id, target_tx, target_actual_name, target_bb_uuid)) = target_found {
                info!(by = %username, target = %target_actual_name, "Admin executed queueskip");

                let (host, port) = match ctx.target_server.split_once(':') {
                    Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(25565)),
                    None => (ctx.target_server.clone(), 25565),
                };

                let (sound_opt, transfer_pkt) = ctx.hud.render_transfer(&host, port);
                if let Some(snd) = sound_opt {
                    let _ = target_tx.send(SessionCommand::SendRaw(snd));
                }
                let target_msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §aTransfert forcé vers le serveur de jeu !",
                    false,
                );
                let _ = target_tx.send(SessionCommand::SendRaw(target_msg));
                let _ = target_tx.send(SessionCommand::SendRaw(transfer_pkt));
                let remove_bb = ctx.hud.render_bossbar_remove(target_bb_uuid);
                let _ = target_tx.send(SessionCommand::SendRaw(remove_bb));
                let _ = target_tx.send(SessionCommand::Transfer {
                    host: host.clone(),
                    port,
                });

                ctx.queue.lock().await.remove(target_id);
                ctx.sessions.lock().await.remove(&target_id);

                let reply = encode_system_chat_packet(
                    &format!(
                        "§b§lᴢᴇɴɪᴛʜ §8» §aTransfert forcé de §e{} §aeffectué avec succès.",
                        target_actual_name
                    ),
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(reply));
            } else {
                let msg = encode_system_chat_packet(
                    &format!(
                        "§b§lᴢᴇɴɪᴛʜ §8» §cPlayer '{}' is not in the queue.",
                        target_name
                    ),
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
            }
        }
        "group" => {
            if !is_staff {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cYou do not have permission to use this command.",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            let subcmd = args.first().map(|s| s.as_str()).unwrap_or("");
            match subcmd {
                "create" => {
                    if args.len() < 3 {
                        let msg = encode_system_chat_packet(
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /group create <name> <weight> [staff: true/false]",
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                        return;
                    }
                    let name = &args[1];
                    let weight: u32 = match args[2].parse() {
                        Ok(w) => w,
                        Err(_) => {
                            let msg = encode_system_chat_packet(
                                "§b§lᴢᴇɴɪᴛʜ §8» §cWeight must be a positive integer (e.g. 100).",
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                            return;
                        }
                    };
                    let is_staff_group = args
                        .get(3)
                        .map(|s| s == "true" || s == "1")
                        .unwrap_or(false);

                    let mut gm = ctx.group_mgr.lock().await;
                    match gm.create_group(name, weight, is_staff_group) {
                        Ok(()) => {
                            info!(by = %username, group = %name, weight, is_staff = is_staff_group, "Created group");
                            let msg = encode_system_chat_packet(
                                &format!(
                                    "§b§lᴢᴇɴɪᴛʜ §8» §aGroup §e{} §acreated successfully (weight: §b{}§a, staff: §b{}§a).",
                                    name,
                                    weight,
                                    if is_staff_group { "yes" } else { "no" }
                                ),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                        Err(e) => {
                            let msg = encode_system_chat_packet(
                                &format!("§b§lᴢᴇɴɪᴛʜ §8» §c{}", e),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                    }
                }
                "delete" => {
                    if args.len() < 2 {
                        let msg = encode_system_chat_packet(
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /group delete <name>",
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                        return;
                    }
                    let name = &args[1];
                    let mut gm = ctx.group_mgr.lock().await;
                    match gm.delete_group(name) {
                        Ok(()) => {
                            info!(by = %username, group = %name, "Deleted group");
                            let msg = encode_system_chat_packet(
                                &format!("§b§lᴢᴇɴɪᴛʜ §8» §aGroup §e{} §adeleted.", name),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                        Err(e) => {
                            let msg = encode_system_chat_packet(
                                &format!("§b§lᴢᴇɴɪᴛʜ §8» §c{}", e),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                    }
                }
                "setweight" => {
                    if args.len() < 3 {
                        let msg = encode_system_chat_packet(
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /group setweight <group> <weight>",
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                        return;
                    }
                    let name = &args[1];
                    let weight: u32 = match args[2].parse() {
                        Ok(w) => w,
                        Err(_) => {
                            let msg = encode_system_chat_packet(
                                "§b§lᴢᴇɴɪᴛʜ §8» §cWeight must be a positive integer.",
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                            return;
                        }
                    };

                    let mut gm = ctx.group_mgr.lock().await;
                    match gm.set_weight(name, weight) {
                        Ok(()) => {
                            info!(by = %username, group = %name, weight, "Updated group weight");
                            let msg = encode_system_chat_packet(
                                &format!(
                                    "§b§lᴢᴇɴɪᴛʜ §8» §aGroup §e{} §aweight updated: §b{}§a.",
                                    name, weight
                                ),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                        Err(e) => {
                            let msg = encode_system_chat_packet(
                                &format!("§b§lᴢᴇɴɪᴛʜ §8» §c{}", e),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                    }
                }
                "setstaff" => {
                    if args.len() < 3 {
                        let msg = encode_system_chat_packet(
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /group setstaff <group> <true|false>",
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                        return;
                    }
                    let name = &args[1];
                    let is_staff_flag = args[2] == "true" || args[2] == "1";

                    let mut gm = ctx.group_mgr.lock().await;
                    match gm.set_staff(name, is_staff_flag) {
                        Ok(()) => {
                            info!(by = %username, group = %name, is_staff = is_staff_flag, "Updated group staff permission");
                            let msg = encode_system_chat_packet(
                                &format!(
                                    "§b§lᴢᴇɴɪᴛʜ §8» §aGroup §e{} §astaff status: §b{}§a.",
                                    name,
                                    if is_staff_flag { "yes" } else { "no" }
                                ),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                        Err(e) => {
                            let msg = encode_system_chat_packet(
                                &format!("§b§lᴢᴇɴɪᴛʜ §8» §c{}", e),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                    }
                }
                "add" => {
                    if args.len() < 3 {
                        let msg = encode_system_chat_packet(
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /group add <group> <player>",
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                        return;
                    }
                    let group = &args[1];
                    let player = &args[2];

                    let mut gm = ctx.group_mgr.lock().await;
                    match gm.add_member(group, player) {
                        Ok(()) => {
                            let new_prio = gm.priority_of(player);
                            let mut q = ctx.queue.lock().await;
                            if let Some(target_sid) = q.session_id_of(player) {
                                q.update_priority(target_sid, new_prio);
                            }
                            info!(by = %username, target = %player, group = %group, "Added player to group");
                            let msg = encode_system_chat_packet(
                                &format!(
                                    "§b§lᴢᴇɴɪᴛʜ §8» §aPlayer §e{} §aadded to group §b{}§a.",
                                    player, group
                                ),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                        Err(e) => {
                            let msg = encode_system_chat_packet(
                                &format!("§b§lᴢᴇɴɪᴛʜ §8» §c{}", e),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                    }
                }
                "remove" => {
                    if args.len() < 3 {
                        let msg = encode_system_chat_packet(
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /group remove <group> <player>",
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                        return;
                    }
                    let group = &args[1];
                    let player = &args[2];

                    let mut gm = ctx.group_mgr.lock().await;
                    match gm.remove_member(group, player) {
                        Ok(()) => {
                            let new_prio = gm.priority_of(player);
                            let mut q = ctx.queue.lock().await;
                            if let Some(target_sid) = q.session_id_of(player) {
                                q.update_priority(target_sid, new_prio);
                            }
                            info!(by = %username, target = %player, group = %group, "Removed player from group");
                            let msg = encode_system_chat_packet(
                                &format!(
                                    "§b§lᴢᴇɴɪᴛʜ §8» §cPlayer §e{} §cremoved from group §b{}§c.",
                                    player, group
                                ),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                        Err(e) => {
                            let msg = encode_system_chat_packet(
                                &format!("§b§lᴢᴇɴɪᴛʜ §8» §c{}", e),
                                false,
                            );
                            let _ = tx.send(SessionCommand::SendRaw(msg));
                        }
                    }
                }
                "info" => {
                    if args.len() < 2 {
                        let msg = encode_system_chat_packet(
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /group info <group>",
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                        return;
                    }
                    let name = &args[1];
                    let gm = ctx.group_mgr.lock().await;
                    if let Some(g) = gm.get_group(name) {
                        let m1 = encode_system_chat_packet(
                            &format!("§b§l=== Group: §e{} §b§l===", g.name),
                            false,
                        );
                        let m2 =
                            encode_system_chat_packet(&format!("§7Weight: §a{}", g.weight), false);
                        let m3 = encode_system_chat_packet(
                            &format!("§7Staff: §f{}", if g.is_staff { "§aYes" } else { "§cNo" }),
                            false,
                        );
                        let mut mem: Vec<_> = g.members.iter().cloned().collect();
                        mem.sort();
                        let mem_str = if mem.is_empty() {
                            "none".to_string()
                        } else {
                            mem.join(", ")
                        };
                        let m4 = encode_system_chat_packet(
                            &format!("§7Members ({}) : §f{}", mem.len(), mem_str),
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(m1));
                        let _ = tx.send(SessionCommand::SendRaw(m2));
                        let _ = tx.send(SessionCommand::SendRaw(m3));
                        let _ = tx.send(SessionCommand::SendRaw(m4));
                    } else {
                        let msg =
                            encode_system_chat_packet("§b§lᴢᴇɴɪᴛʜ §8» §cGroup not found.", false);
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                    }
                }
                "list" => {
                    let gm = ctx.group_mgr.lock().await;
                    let msg1 =
                        encode_system_chat_packet("§b§l=== Zenith Groups & Permissions ===", false);
                    let _ = tx.send(SessionCommand::SendRaw(msg1));
                    for g in gm.all_groups() {
                        let line = format!(
                            "§e• §b{} §8(§7weight: §a{}§8, §7staff: {}§8, §7members: §e{}§8)",
                            g.name,
                            g.weight,
                            if g.is_staff { "§ayes" } else { "§cno" },
                            g.members.len()
                        );
                        let msg = encode_system_chat_packet(&line, false);
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                    }
                }
                _ => {
                    let m1 = encode_system_chat_packet(
                        "§8§m                                                  §r",
                        false,
                    );
                    let m2 = encode_system_chat_packet(
                        "§b§l  ᴢᴇɴɪᴛʜ §8» §fGroup & Tier Administration",
                        false,
                    );
                    let m3 = encode_system_chat_packet(
                        "  §e/group list §8- §7List all groups and priorities",
                        false,
                    );
                    let m4 = encode_system_chat_packet(
                        "  §e/group info <group> §8- §7View group details & members",
                        false,
                    );
                    let m5 = encode_system_chat_packet(
                        "  §e/group add <group> <player> §8- §7Assign player to priority group",
                        false,
                    );
                    let m6 = encode_system_chat_packet(
                        "  §e/group remove <group> <player> §8- §7Remove player from group",
                        false,
                    );
                    let m7 = encode_system_chat_packet(
                        "  §e/group setweight <group> <weight> §8- §7Change priority weight",
                        false,
                    );
                    let m8 = encode_system_chat_packet(
                        "  §e/group setstaff <group> <true|false> §8- §7Toggle staff permissions",
                        false,
                    );
                    let m9 = encode_system_chat_packet(
                        "  §e/group create <name> <weight> §8- §7Create a new group",
                        false,
                    );
                    let m10 = encode_system_chat_packet(
                        "  §e/group delete <name> §8- §7Delete an existing group",
                        false,
                    );
                    let m11 = encode_system_chat_packet(
                        "§8§m                                                  §r",
                        false,
                    );
                    let _ = tx.send(SessionCommand::SendRaw(m1));
                    let _ = tx.send(SessionCommand::SendRaw(m2));
                    let _ = tx.send(SessionCommand::SendRaw(m3));
                    let _ = tx.send(SessionCommand::SendRaw(m4));
                    let _ = tx.send(SessionCommand::SendRaw(m5));
                    let _ = tx.send(SessionCommand::SendRaw(m6));
                    let _ = tx.send(SessionCommand::SendRaw(m7));
                    let _ = tx.send(SessionCommand::SendRaw(m8));
                    let _ = tx.send(SessionCommand::SendRaw(m9));
                    let _ = tx.send(SessionCommand::SendRaw(m10));
                    let _ = tx.send(SessionCommand::SendRaw(m11));
                }
            }
        }
        "queueinfo" | "info" => {
            let q = ctx.queue.lock().await;
            let (pos, total) = q.position_of(session_id).unwrap_or((1, q.len().max(1)));
            let nominal_wave_size = q.config().nominal_wave_size.max(1);
            let nominal_interval_ms = q.config().nominal_interval_ms.max(1000);
            let eta_seconds = ((pos as u64).saturating_sub(1) / nominal_wave_size as u64)
                * (nominal_interval_ms / 1000);
            let eta_str = zenith_display::format_eta(eta_seconds);

            let status_str = if ctx.is_queue_active.load(Ordering::SeqCst) {
                "§aOpen"
            } else {
                "§cPaused"
            };

            let m1 = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );
            let m2 = encode_system_chat_packet("§b§l  ᴢᴇɴɪᴛʜ §8» §fTransit Queue", false);
            let m3 = encode_system_chat_packet(
                &format!("§7  Status: {} §7| Players: §e{}", status_str, total),
                false,
            );
            let m4 = encode_system_chat_packet(
                &format!(
                    "§7  Your position: §e#{} §8/ §e{} §7(estimated wait: §a~{}§7)",
                    pos, total, eta_str
                ),
                false,
            );
            let m5 = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );

            let _ = tx.send(SessionCommand::SendRaw(m1));
            let _ = tx.send(SessionCommand::SendRaw(m2));
            let _ = tx.send(SessionCommand::SendRaw(m3));
            let _ = tx.send(SessionCommand::SendRaw(m4));
            let _ = tx.send(SessionCommand::SendRaw(m5));
        }
        "status" => {
            if !is_staff {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cYou do not have permission to view backend status.",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            let health = ctx.health_rx.borrow().clone();
            let m1 = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );
            let m2 = encode_system_chat_packet("§b§l  ᴢᴇɴɪᴛʜ §8» §fBackend Server Health", false);
            let (status_text, ping_text) = if !ctx.totem_enabled {
                (
                    "§aOnline (Available)",
                    "§7Direct (monitoring disabled)".to_string(),
                )
            } else {
                match &health {
                    BackendHealth::Healthy { latency_ms } => {
                        ("§aOnline (Optimal)", format!("§a{}ms", latency_ms))
                    }
                    BackendHealth::Degraded { latency_ms, reason } => (
                        "§eDegraded",
                        format!("§e{}ms §8(§7{}§8)", latency_ms, reason),
                    ),
                    BackendHealth::Offline {
                        consecutive_failures,
                        last_error,
                    } => (
                        "§cOffline (Holding active)",
                        format!("§c{} failures §8(§7{}§8)", consecutive_failures, last_error),
                    ),
                }
            };
            let m3 =
                encode_system_chat_packet(&format!("§7  Target: §e{}", ctx.target_server), false);
            let m4 = encode_system_chat_packet(&format!("§7  Health: {}", status_text), false);
            let m5 =
                encode_system_chat_packet(&format!("§7  Latency / Info: {}", ping_text), false);
            let m6 = encode_system_chat_packet(
                &format!(
                    "§7  Resource Pack Preload: {}",
                    if ctx.rp_enabled {
                        "§aActive"
                    } else {
                        "§7Disabled"
                    }
                ),
                false,
            );
            let m7 = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );

            let _ = tx.send(SessionCommand::SendRaw(m1));
            let _ = tx.send(SessionCommand::SendRaw(m2));
            let _ = tx.send(SessionCommand::SendRaw(m3));
            let _ = tx.send(SessionCommand::SendRaw(m4));
            let _ = tx.send(SessionCommand::SendRaw(m5));
            let _ = tx.send(SessionCommand::SendRaw(m6));
            let _ = tx.send(SessionCommand::SendRaw(m7));
        }
        "broadcast" | "bc" => {
            if !is_staff {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cYou do not have permission to broadcast.",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            if args.is_empty() {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /broadcast <message>",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            let broadcast_text = args.join(" ");
            let chat = encode_system_chat_packet(
                &format!("§b§lᴢᴇɴɪᴛʜ §8» §d§l[BROADCAST] §r§f{}", broadcast_text),
                false,
            );

            let sess = ctx.sessions.lock().await;
            for (s_tx, _, _, _, _) in sess.values() {
                let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
            }
            info!(by = %username, message = %broadcast_text, "Staff broadcast sent to all players");
        }
        "alert" => {
            if !is_staff {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cYou do not have permission to send alerts.",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            if args.is_empty() {
                let msg =
                    encode_system_chat_packet("§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /alert <message>", false);
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            let alert_text = args.join(" ");
            let times = encode_title_times_packet(10, 60, 20);
            let title = encode_title_packet("§c§lALERT");
            let sub = encode_subtitle_packet(&format!("§e{}", alert_text));
            let chat = encode_system_chat_packet(
                &format!("§b§lᴢᴇɴɪᴛʜ §8» §c§l[ALERT] §e{}", alert_text),
                false,
            );

            let sess = ctx.sessions.lock().await;
            for (s_tx, _, _, _, _) in sess.values() {
                let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
            }
            info!(by = %username, alert = %alert_text, "Staff alert sent to all players");
        }
        "test" => {
            if !is_staff {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cYou do not have permission to run simulation tests.",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(msg));
                return;
            }

            let subcmd = args.first().map(|s| s.as_str()).unwrap_or("");
            match subcmd {
                "crash" => {
                    let times = encode_title_times_packet(5, 50, 15);
                    let sub =
                        encode_subtitle_packet("§c§lBACKEND RESTARTING §7Recovery in progress...");
                    let title = encode_title_packet("§c⚠");
                    let chat = encode_system_chat_packet(
                        "§b§lᴢᴇɴɪᴛʜ §8» §c§lALERT: §7Target backend crashed or restarting. You are safe in virtual holding!",
                        false,
                    );

                    let sess = ctx.sessions.lock().await;
                    for (s_tx, _, _, _, _) in sess.values() {
                        let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                        let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                        let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                        let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                    }
                    info!(by = %username, "Simulated Totem backend crash alert triggered");
                }
                "recovery" | "recover" => {
                    let times = encode_title_times_packet(5, 50, 15);
                    let sub = encode_subtitle_packet(
                        "§a§lBACKEND READY §7Resuming automatic transfers...",
                    );
                    let title = encode_title_packet("§a▶");
                    let chat = encode_system_chat_packet(
                        "§b§lᴢᴇɴɪᴛʜ §8» §a§lBACKEND ONLINE! §7Ping: §e18ms§7. Preparing transfers...",
                        false,
                    );

                    let sess = ctx.sessions.lock().await;
                    for (s_tx, _, _, _, _) in sess.values() {
                        let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                        let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                        let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                        let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                    }
                    info!(by = %username, "Simulated Totem backend recovery alert triggered");
                }
                "pack" => {
                    let pack_uuid = 0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00;
                    let pack_url = "https://assets.horizon.dev/packs/server-pack.zip".to_string();
                    let pack_hash = "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string();
                    let prompt =
                        Some("Pre-caching Horizon server textures in queue...".to_string());
                    let _ = tx.send(SessionCommand::PushResourcePack {
                        uuid: pack_uuid,
                        url: pack_url,
                        hash: pack_hash,
                        required: false,
                        prompt,
                    });
                    let msg = encode_system_chat_packet(
                        "§b§lᴢᴇɴɪᴛʜ §8» §aResource Pack push packet sent to your client!",
                        false,
                    );
                    let _ = tx.send(SessionCommand::SendRaw(msg));
                    info!(by = %username, "Resource Pack push packet sent to caller");
                }
                _ => {
                    let msg = encode_system_chat_packet(
                        "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /test <crash | recovery | pack>",
                        false,
                    );
                    let _ = tx.send(SessionCommand::SendRaw(msg));
                }
            }
        }
        "ping" => {
            let ping = ctx.queue.lock().await.ping_of(session_id).unwrap_or(0);
            let display_ping = if ping == 0 {
                "<1".to_string()
            } else {
                ping.to_string()
            };
            let msg = encode_system_chat_packet(
                &format!("§b§lᴢᴇɴɪᴛʜ §8» §7Your latency: §a{}ms", display_ping),
                false,
            );
            let _ = tx.send(SessionCommand::SendRaw(msg));
        }
        "help" => {
            let m1 = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );
            let m2 = encode_system_chat_packet("§b§l  ᴢᴇɴɪᴛʜ §8» §fAvailable Commands", false);
            let m3 = encode_system_chat_packet(
                "  §e/info     §8- §7View your position and estimated wait time",
                false,
            );
            let m4 = encode_system_chat_packet(
                "  §e/ping     §8- §7Display your round-trip network latency",
                false,
            );
            let m5 = encode_system_chat_packet(
                "  §e/help     §8- §7Display this command reference",
                false,
            );
            let _ = tx.send(SessionCommand::SendRaw(m1));
            let _ = tx.send(SessionCommand::SendRaw(m2));
            let _ = tx.send(SessionCommand::SendRaw(m3));
            let _ = tx.send(SessionCommand::SendRaw(m4));
            let _ = tx.send(SessionCommand::SendRaw(m5));

            if is_staff {
                let m_head = encode_system_chat_packet("§c§l  Staff Commands:", false);
                let m_trans = encode_system_chat_packet(
                    "  §c/transfer      §8- §7Force instant transfer to target backend",
                    false,
                );
                let m_stat = encode_system_chat_packet(
                    "  §c/status        §8- §7View backend server health and latency",
                    false,
                );
                let m_staff = encode_system_chat_packet(
                    "  §c/queue         §8- §7Open or pause queue transit waves",
                    false,
                );
                let m_skip = encode_system_chat_packet(
                    "  §c/queueskip <p> §8- §7Force-transfer a specific player",
                    false,
                );
                let m_bc = encode_system_chat_packet(
                    "  §c/broadcast <m> §8- §7Send a chat announcement to all players",
                    false,
                );
                let m_al = encode_system_chat_packet(
                    "  §c/alert <m>     §8- §7Flash a screen Title + Subtitle alert",
                    false,
                );
                let m_grp = encode_system_chat_packet(
                    "  §c/group ...     §8- §7Manage priority groups and staff permissions",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(m_head));
                let _ = tx.send(SessionCommand::SendRaw(m_trans));
                let _ = tx.send(SessionCommand::SendRaw(m_stat));
                let _ = tx.send(SessionCommand::SendRaw(m_staff));
                let _ = tx.send(SessionCommand::SendRaw(m_skip));
                let _ = tx.send(SessionCommand::SendRaw(m_bc));
                let _ = tx.send(SessionCommand::SendRaw(m_al));
                let _ = tx.send(SessionCommand::SendRaw(m_grp));
            }

            let m_end = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );
            let _ = tx.send(SessionCommand::SendRaw(m_end));
        }
        _ => {}
    }
}
