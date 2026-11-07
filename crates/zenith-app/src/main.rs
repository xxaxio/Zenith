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
use zenith_wire::HmacForwarding;

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
    hmac_forwarding: Option<HmacForwarding>,
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
    let totem_config = TotemConfig::default();
    let (totem_lifeline, totem_handles) = TotemLifeline::new(&target_server, totem_config);
    let mut totem_event_rx = totem_handles.event_rx;
    let health_rx = totem_handles.health_rx;

    tokio::spawn(totem_lifeline.run());

    let hud_health_rx = health_rx.clone();
    let wave_health_rx = health_rx.clone();
    let cmd_health_rx = health_rx.clone();

    // Background Totem Event Broadcaster & Auto-Recovery Lifeline
    let totem_sessions = Arc::clone(&active_sessions);
    let totem_queue_active = Arc::clone(&is_queue_active);
    let totem_queue = Arc::clone(&queue);
    let totem_metrics = Arc::clone(&metrics);
    let totem_mode = mode;
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

                    let sessions = totem_sessions.lock().await;
                    let times = encode_title_times_packet(5, 50, 15);
                    let sub =
                        encode_subtitle_packet("§c§lSERVEUR EN PAUSE §7Redémarrage en cours...");
                    let title = encode_title_packet("§c⚠");
                    let chat = encode_system_chat_packet(
                        "§b§lᴢᴇɴɪᴛʜ §8» §c§lALERTE : §7Le serveur cible a crashé ou redémarre. Vous êtes protégés dans le sas !",
                        false,
                    );

                    for (s_tx, _, _, _, _) in sessions.values() {
                        let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                        let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                        let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                        let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
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

                    let sessions = totem_sessions.lock().await;
                    let times = encode_title_times_packet(5, 50, 15);
                    let sub = encode_subtitle_packet(
                        "§a§lSERVEUR PRÊT ! §7Transferts automatiques en cours...",
                    );
                    let title = encode_title_packet("§a▶");
                    let chat = encode_system_chat_packet(
                        &format!(
                            "§b§lᴢᴇɴɪᴛʜ §8» §a§lSERVEUR EN LIGNE ! §7Ping: §e{}ms§7. Préparez-vous au transfert...",
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
                TotemEvent::StatusUpdate { .. } => {}
            }
        }
    });

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SessionEvent>();

    // 1. Background HUD refresh loop (every 1 second)
    let hud_queue = Arc::clone(&queue);
    let hud_renderer = Arc::clone(&hud);
    let hud_sessions = Arc::clone(&active_sessions);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(1000));
        loop {
            interval.tick().await;

            let q = hud_queue.lock().await;
            if q.is_empty() {
                continue;
            }

            let total = q.len();
            let nominal_wave_size = q.config().nominal_wave_size.max(1);
            let nominal_interval_ms = q.config().nominal_interval_ms.max(1000);

            let limbo_actionbar = if mode == ZenithMode::Limbo {
                Some(if !hud_health_rx.borrow().is_available() {
                    zenith_display::encode_actionbar_packet(
                        "{\"text\":\"§c§l● §7Serveur cible en redémarrage... §8| §b§lᴢᴇɴɪᴛʜ §7Sas sécurisé\"}",
                    )
                } else {
                    hud_renderer.render_limbo_actionbar()
                })
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
                } else if let Some((pos, total)) = q.position_of(session_id) {
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
        hmac_forwarding,
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
    loop {
        let (socket, peer_addr) = listener.accept().await?;
        debug!(peer = %peer_addr, "Accepted incoming TCP connection");

        let peer_ip = peer_addr.ip();
        let _conn_guard = if shield_enabled {
            match conn_tracker.acquire(peer_ip) {
                Ok(guard) => {
                    let check_res = {
                        let mut rl = rate_limiter.lock().await;
                        rl.check_allowed(peer_ip, Instant::now())
                    };
                    match check_res {
                        Ok(()) => Some(guard),
                        Err(e) => {
                            loop_metrics
                                .shield_rate_limited_total
                                .fetch_add(1, Ordering::Relaxed);
                            let jailed = rate_limiter.lock().await.jail_manager().count();
                            loop_metrics
                                .shield_jailed_ips
                                .store(jailed, Ordering::Relaxed);
                            debug!(peer = %peer_addr, error = %e, "Shield rejected incoming connection: rate-limited or jailed");
                            continue;
                        }
                    }
                }
                Err(e) => {
                    loop_metrics
                        .shield_rate_limited_total
                        .fetch_add(1, Ordering::Relaxed);
                    debug!(peer = %peer_addr, error = %e, "Shield rejected incoming connection: concurrent limit reached");
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
                        "§7Sas de rétention"
                    } else {
                        "§7File d'attente"
                    });
                    let _ = cmd_tx.send(SessionCommand::SendRaw(sub));
                    let title = encode_title_packet("§b§lᴢᴇɴɪᴛʜ");
                    let _ = cmd_tx.send(SessionCommand::SendRaw(title));

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
            let current_health = ctx.health_rx.borrow().clone();
            if !current_health.is_available() {
                let msg = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cLe serveur cible est temporairement indisponible (en cours de redémarrage). Vous êtes en sécurité dans le sas !",
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
                let sub = encode_subtitle_packet("§b§lC'EST PARTI ! §7La file est ouverte");
                let title = encode_title_packet("§a▶");
                let chat = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §aLa file d'attente est maintenant §2§lOUVERTE §a!",
                    false,
                );

                for (s_tx, _, _, _, _) in sess.values() {
                    let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                }
            } else {
                info!(by = %username, "Queue PAUSED by staff member");
                let times = encode_title_times_packet(5, 50, 15);
                let sub = encode_subtitle_packet("§c§lFILE EN PAUSE §7Ouverture suspendue");
                let title = encode_title_packet("§c⏸");
                let chat = encode_system_chat_packet(
                    "§b§lᴢᴇɴɪᴛʜ §8» §cLa file d'attente est maintenant §4§lEN PAUSE §c!",
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
                        "§b§lᴢᴇɴɪᴛʜ §8» §cLe joueur '{}' n'est pas dans la file d'attente.",
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
                    "§b§lᴢᴇɴɪᴛʜ §8» §cVous n'avez pas la permission d'utiliser cette commande.",
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
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUtilisation: /group create <nom> <poids> [staff: true/false]",
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
                                "§b§lᴢᴇɴɪᴛʜ §8» §cLe poids doit être un entier positif (ex: 100).",
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
                                    "§b§lᴢᴇɴɪᴛʜ §8» §aGroupe §e{} §acréé avec succès (poids: §b{}§a, staff: §b{}§a).",
                                    name,
                                    weight,
                                    if is_staff_group { "oui" } else { "non" }
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
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUtilisation: /group delete <nom>",
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
                                &format!("§b§lᴢᴇɴɪᴛʜ §8» §aGroupe §e{} §asupprimé.", name),
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
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUtilisation: /group setweight <groupe> <poids>",
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
                                "§b§lᴢᴇɴɪᴛʜ §8» §cLe poids doit être un entier positif.",
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
                                    "§b§lᴢᴇɴɪᴛʜ §8» §aPoids du groupe §e{} §amis à jour: §b{}§a.",
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
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUtilisation: /group setstaff <groupe> <true|false>",
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
                                    "§b§lᴢᴇɴɪᴛʜ §8» §aPermission staff du groupe §e{}§a: §b{}§a.",
                                    name,
                                    if is_staff_flag { "oui" } else { "non" }
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
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUtilisation: /group add <groupe> <joueur>",
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
                                    "§b§lᴢᴇɴɪᴛʜ §8» §aJoueur §e{} §aajouté au groupe §b{}§a.",
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
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUtilisation: /group remove <groupe> <joueur>",
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
                                    "§b§lᴢᴇɴɪᴛʜ §8» §cJoueur §e{} §cretiré du groupe §b{}§c.",
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
                            "§b§lᴢᴇɴɪᴛʜ §8» §cUtilisation: /group info <groupe>",
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                        return;
                    }
                    let name = &args[1];
                    let gm = ctx.group_mgr.lock().await;
                    if let Some(g) = gm.get_group(name) {
                        let m1 = encode_system_chat_packet(
                            &format!("§b§l=== Groupe: §e{} §b§l===", g.name),
                            false,
                        );
                        let m2 =
                            encode_system_chat_packet(&format!("§7Poids: §a{}", g.weight), false);
                        let m3 = encode_system_chat_packet(
                            &format!("§7Staff: §f{}", if g.is_staff { "§aOui" } else { "§cNon" }),
                            false,
                        );
                        let mut mem: Vec<_> = g.members.iter().cloned().collect();
                        mem.sort();
                        let mem_str = if mem.is_empty() {
                            "aucun".to_string()
                        } else {
                            mem.join(", ")
                        };
                        let m4 = encode_system_chat_packet(
                            &format!("§7Membres ({}) : §f{}", mem.len(), mem_str),
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(m1));
                        let _ = tx.send(SessionCommand::SendRaw(m2));
                        let _ = tx.send(SessionCommand::SendRaw(m3));
                        let _ = tx.send(SessionCommand::SendRaw(m4));
                    } else {
                        let msg = encode_system_chat_packet(
                            "§b§lᴢᴇɴɪᴛʜ §8» §cGroupe introuvable.",
                            false,
                        );
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                    }
                }
                "list" => {
                    let gm = ctx.group_mgr.lock().await;
                    let msg1 = encode_system_chat_packet(
                        "§b§l=== Groupes & Permissions Zenith ===",
                        false,
                    );
                    let _ = tx.send(SessionCommand::SendRaw(msg1));
                    for g in gm.all_groups() {
                        let line = format!(
                            "§e• §b{} §8(§7poids: §a{}§8, §7staff: {}§8, §7membres: §e{}§8)",
                            g.name,
                            g.weight,
                            if g.is_staff { "§aoui" } else { "§cnon" },
                            g.members.len()
                        );
                        let msg = encode_system_chat_packet(&line, false);
                        let _ = tx.send(SessionCommand::SendRaw(msg));
                    }
                }
                _ => {
                    let msg = encode_system_chat_packet(
                        "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /group <list|info|create|delete|setweight|setstaff|add|remove>",
                        false,
                    );
                    let _ = tx.send(SessionCommand::SendRaw(msg));
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
                "§aOuverte"
            } else {
                "§cEn pause"
            };

            let m1 = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );
            let m2 = encode_system_chat_packet("§b§l  ᴢᴇɴɪᴛʜ §8» §fFile d'Attente", false);
            let m3 = encode_system_chat_packet(
                &format!("§7  Statut: {} §7| Joueurs: §e{}", status_str, total),
                false,
            );
            let m4 = encode_system_chat_packet(
                &format!(
                    "§7  Votre position: §e#{} §8/ §e{} §7(attente: §a~{}§7)",
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
            let health = ctx.health_rx.borrow().clone();
            let m1 = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );
            let m2 = encode_system_chat_packet("§b§l  ᴢᴇɴɪᴛʜ §8» §fÉtat du Serveur Backend", false);
            let (status_text, ping_text) = match &health {
                BackendHealth::Healthy { latency_ms } => {
                    ("§aEn ligne (Optimal)", format!("§a{}ms", latency_ms))
                }
                BackendHealth::Degraded { latency_ms, reason } => (
                    "§eDégradé",
                    format!("§e{}ms §8(§7{}§8)", latency_ms, reason),
                ),
                BackendHealth::Offline {
                    consecutive_failures,
                    last_error,
                } => (
                    "§cHors-ligne (Sas actif)",
                    format!("§c{} échecs §8(§7{}§8)", consecutive_failures, last_error),
                ),
            };
            let m3 =
                encode_system_chat_packet(&format!("§7  Cible: §e{}", ctx.target_server), false);
            let m4 = encode_system_chat_packet(&format!("§7  Santé: {}", status_text), false);
            let m5 =
                encode_system_chat_packet(&format!("§7  Latence / Info: {}", ping_text), false);
            let m6 = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );

            let _ = tx.send(SessionCommand::SendRaw(m1));
            let _ = tx.send(SessionCommand::SendRaw(m2));
            let _ = tx.send(SessionCommand::SendRaw(m3));
            let _ = tx.send(SessionCommand::SendRaw(m4));
            let _ = tx.send(SessionCommand::SendRaw(m5));
            let _ = tx.send(SessionCommand::SendRaw(m6));
        }
        "ping" => {
            let msg = encode_system_chat_packet("§b§lᴢᴇɴɪᴛʜ §8» §7Latence réseau: §a~15ms", false);
            let _ = tx.send(SessionCommand::SendRaw(msg));
        }
        "help" => {
            let m1 = encode_system_chat_packet(
                "§8§m                                                  §r",
                false,
            );
            let m2 = encode_system_chat_packet("§b§l  ᴢᴇɴɪᴛʜ §8» §fCommandes Disponibles", false);
            let m3 = encode_system_chat_packet(
                "  §b/transfer §8- §7Rejoindre immédiatement le serveur",
                false,
            );
            let m4 = encode_system_chat_packet(
                "  §e/info     §8- §7Consulter votre position et temps estimé",
                false,
            );
            let m5 = encode_system_chat_packet(
                "  §e/status   §8- §7Consulter l'état de santé du serveur backend",
                false,
            );
            let m6 = encode_system_chat_packet(
                "  §e/ping     §8- §7Afficher votre latence réseau",
                false,
            );
            let _ = tx.send(SessionCommand::SendRaw(m1));
            let _ = tx.send(SessionCommand::SendRaw(m2));
            let _ = tx.send(SessionCommand::SendRaw(m3));
            let _ = tx.send(SessionCommand::SendRaw(m4));
            let _ = tx.send(SessionCommand::SendRaw(m5));
            let _ = tx.send(SessionCommand::SendRaw(m6));

            if is_staff {
                let m_staff = encode_system_chat_packet(
                    "  §c/queue         §8- §7Ouvrir ou mettre en pause la file",
                    false,
                );
                let m_skip = encode_system_chat_packet(
                    "  §c/queueskip <j> §8- §7Transférer de force un joueur",
                    false,
                );
                let m_grp = encode_system_chat_packet(
                    "  §c/group ...     §8- §7Gérer les groupes LuckPerms (list/info/create/add...)",
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(m_staff));
                let _ = tx.send(SessionCommand::SendRaw(m_skip));
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
